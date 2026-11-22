#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::io;
    use tehuti_protocol::benchmark::{Sample, exchange};
    use tehuti_protocol::{ButtonName, ButtonState, Call, Message, Method, RpcVersion, parse_message};

    #[test]
    fn measures_host_json_and_round_trip_with_an_injected_clock() -> Result<(), Box<dyn core::error::Error>> {
        let call = Call {
            jsonrpc: RpcVersion::Two,
            request_id: Some(7),
            method: Method::Button {
                name: ButtonName::East,
                state: ButtonState::Pressed,
            },
        };
        let mut wire = Vec::new();
        let mut response = io::Cursor::new(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":[]}\n");
        let mut ticks = [100, 120, 260, 300].into_iter();
        let sample = exchange(&call, &mut wire, &mut response, &mut || {
            Duration::from_nanos(ticks.next().unwrap_or_default())
        })?;
        assert_eq!(
            sample,
            Sample {
                round_trip_ns: 200,
                json_ns: 60
            },
            "only host codec time belongs in JSON share"
        );
        assert_eq!(
            parse_message(&wire)?,
            Message::Call(call),
            "the request crosses the JSON wire"
        );
        Ok(())
    }

    #[test]
    fn button_and_axis_workloads_cross_headless_child_stdio() -> Result<(), Box<dyn core::error::Error>> {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_plugin-benchmark"))
            .args(["--samples", "2"])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XDG_RUNTIME_DIR")
            .output()?;
        assert!(output.status.success(), "benchmark must run without a session");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        for name in ["button", "axis"] {
            let samples = report
                .get(name)
                .and_then(serde_json::Value::as_array)
                .ok_or("missing workload samples")?;
            assert_eq!(samples.len(), 2, "warmup samples are excluded");
            for sample in samples {
                let total = sample
                    .get("round_trip_ns")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or("missing round trip")?;
                let codec = sample
                    .get("json_ns")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or("missing JSON timing")?;
                assert!(
                    total > 0 && codec <= total,
                    "host codec time is part of the round trip"
                );
            }
        }
        Ok(())
    }
}
