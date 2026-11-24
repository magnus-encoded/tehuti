//! Timing the host JSON codec around a real stdio exchange.

use crate::{Call, Initialized, Message, Method, ResultValue, parse_message};
use core::time::Duration;
use std::io::{BufRead, Write};

/// One exchange, timed only on the host.
#[derive(Debug, serde::Serialize, PartialEq, Eq)]
pub struct Sample {
    /// Encoding through reply decoding, including pipe I/O.
    pub round_trip_ns: u128,
    /// Host encoding plus host reply decoding.
    pub json_ns: u128,
}

/// Sends one request and waits for its reference-plugin reply.
///
/// # Errors
/// I/O failure, invalid JSON or a reply that does not match the request.
pub fn exchange(
    call: &Call,
    input: &mut impl Write,
    output: &mut impl BufRead,
    clock: &mut impl FnMut() -> Duration,
) -> Result<Sample, Box<dyn core::error::Error>> {
    let start = clock();
    let bytes = serde_json::to_vec(call)?;
    let encoded = clock();
    input.write_all(&bytes)?;
    input.write_all(b"\n")?;
    input.flush()?;
    let mut line = String::new();
    if output.read_line(&mut line)? == 0 {
        return Err("reference plugin closed stdout".into());
    }
    let decoding = clock();
    let message = parse_message(line.as_bytes())?;
    let end = clock();
    let valid = match (&call.method, message) {
        (Method::Initialize(_), Message::Response(response)) => {
            response.request_id == call.request_id.unwrap_or_default()
                && matches!(response.result, ResultValue::Initialized(Initialized {
                    protocol_version: 0, ref encoding, ..
                }) if encoding == "json")
        }
        (Method::Button { .. }, Message::Response(response)) => {
            response.request_id == call.request_id.unwrap_or_default()
                && response.result == ResultValue::Actions(Vec::new())
        }
        (
            Method::Adjuster { .. },
            Message::Call(Call {
                method: Method::Actions { actions },
                ..
            }),
        ) => actions.is_empty(),
        _ => false,
    };
    if !valid {
        return Err("unexpected reference plugin reply".into());
    }
    Ok(Sample {
        round_trip_ns: end.saturating_sub(start).as_nanos(),
        json_ns: encoded
            .saturating_sub(start)
            .saturating_add(end.saturating_sub(decoding))
            .as_nanos(),
    })
}
