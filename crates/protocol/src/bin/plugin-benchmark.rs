//! Headless plugin stdio measurements, including its reference plugin.

use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::Instant;
#[path = "../benchmark.rs"]
pub mod benchmark;
use benchmark::{Sample, exchange};
use tehuti_protocol::{
    ButtonName, ButtonState, Call, Initialize, Initialized, Message, Method, Response, ResultValue,
    RpcVersion, Subscription, parse_message,
};

type Error = Box<dyn core::error::Error>;

#[derive(Debug)]
struct Reference(Child);

impl Drop for Reference {
    fn drop(&mut self) {
        let _killed = self.0.kill();
        let _reaped = self.0.wait();
    }
}

fn reference() -> Result<(), Error> {
    let mut output = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let Message::Call(call) = parse_message(line?.as_bytes())? else {
            return Err("reference expects a call".into());
        };
        let reply = match call.method {
            Method::Initialize(_) => Message::Response(Response {
                jsonrpc: RpcVersion::Two,
                request_id: call.request_id.ok_or("initialize needs an id")?,
                result: ResultValue::Initialized(Initialized {
                    protocol_version: 0,
                    encoding: "json".into(),
                    events: vec![Subscription::Button],
                }),
            }),
            Method::Button { .. } => Message::Response(Response {
                jsonrpc: RpcVersion::Two,
                request_id: call.request_id.ok_or("button needs an id")?,
                result: ResultValue::Actions(Vec::new()),
            }),
            Method::Adjuster { .. } => Message::Call(Call {
                jsonrpc: RpcVersion::Two,
                request_id: None,
                method: Method::Actions { actions: Vec::new() },
            }),
            Method::Speech { .. }
            | Method::Verb { .. }
            | Method::Verbs { .. }
            | Method::SpeechHints { .. }
            | Method::SpeechLanguage { .. }
            | Method::InvokeWords { .. }
            | Method::Focus { .. }
            | Method::Actions { .. }
            | Method::SubmitActions { .. }
            | Method::PadOverrides { .. }
            | Method::Notice { .. }
            | Method::Status { .. }
            | Method::Toggle { .. }
            | Method::Toggles { .. }
            | Method::PadInput { .. } => return Err("unexpected reference workload".into()),
        };
        output.write_all(&serde_json::to_vec(&reply)?)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}

fn workload_call(name: &str, sequence: u64) -> Call {
    Call {
        jsonrpc: RpcVersion::Two,
        request_id: (name == "button").then_some(sequence),
        method: if name == "button" {
            Method::Button {
                name: ButtonName::East,
                state: ButtonState::Pressed,
            }
        } else {
            Method::Adjuster {
                name: "axis".into(),
                value: if sequence.is_multiple_of(2) {
                    16_384
                } else {
                    -16_384
                },
                finished: false,
            }
        },
    }
}

fn benchmark(samples: u64) -> Result<(), Error> {
    let mut child = Reference(
        Command::new(std::env::current_exe()?)
            .arg("--reference")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?,
    );
    let mut input = child.0.stdin.take().ok_or("missing reference stdin")?;
    let mut output = BufReader::new(child.0.stdout.take().ok_or("missing reference stdout")?);
    let epoch = Instant::now();
    let mut clock = || epoch.elapsed();
    let _handshake = exchange(
        &Call {
            jsonrpc: RpcVersion::Two,
            request_id: Some(0),
            method: Method::Initialize(Initialize {
                protocol_version: 0,
                encoding: "json".into(),
                core_version: env!("CARGO_PKG_VERSION").into(),
                options: None,
            }),
        },
        &mut input,
        &mut output,
        &mut clock,
    )?;
    let mut results = std::collections::BTreeMap::new();
    for name in ["button", "axis"] {
        let mut timings: Vec<Sample> = Vec::new();
        for sequence in 1..=samples.saturating_add(100) {
            let call = workload_call(name, sequence);
            let timing = exchange(&call, &mut input, &mut output, &mut clock)?;
            if sequence > 100 {
                timings.push(timing);
            }
        }
        let _previous = results.insert(name, timings);
    }
    drop(input);
    if !child.0.wait()?.success() {
        return Err("reference plugin failed".into());
    }
    serde_json::to_writer(io::stdout().lock(), &results)?;
    Ok(())
}

fn main() -> Result<(), Error> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.as_slice() {
        [] => benchmark(10_000),
        [flag] if flag == "--reference" => reference(),
        [flag, count] if flag == "--samples" => {
            let samples = count.parse()?;
            if samples == 0 || samples > 1_000_000 {
                return Err("samples must be between 1 and 1000000".into());
            }
            benchmark(samples)
        }
        _ => Err("usage: plugin-benchmark [--samples N]".into()),
    }
}
