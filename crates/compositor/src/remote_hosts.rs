//! Hosts that could send a remote microphone, read from Tailscale when it runs.
//!
//! Tailscale is only read: Tehuti never installs or starts it. When it is
//! missing, stopped or slow, the list stays empty and manual entry still works.

use core::time::Duration;
use std::io;

/// The online peers in `tailscale status --json`: the `MagicDNS` name without its
/// trailing dot, or the host name when there is none. The peer's own machine is not listed.
#[must_use]
pub fn tailnet_peers(status: &serde_json::Value) -> Vec<String> {
    let Some(peers) = status.get("Peer").and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };
    let mut hosts: Vec<String> = Vec::new();
    for peer in peers.values() {
        if peer.get("Online").and_then(serde_json::Value::as_bool) != Some(true) {
            continue;
        }
        let text = |key: &str| {
            peer.get(key)
                .and_then(serde_json::Value::as_str)
                .map(|value| value.trim_end_matches('.'))
                .filter(|value| !value.is_empty())
        };
        if let Some(host) = text("DNSName").or_else(|| text("HostName")) {
            if !hosts.iter().any(|known| known == host) {
                hosts.push(host.to_owned());
            }
        }
    }
    hosts.sort();
    hosts
}

/// Lists tailnet peers off the event loop; an empty list when Tailscale is missing, stopped or hangs.
#[must_use]
pub fn list_remote_hosts() -> std::sync::mpsc::Receiver<Vec<String>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("remote-hosts".into())
        .spawn(move || {
            let hosts = run_status().unwrap_or_else(|error| {
                tracing::info!(%error, "no tailnet suggestions (install tailscale to get them)");
                Vec::new()
            });
            let _sent = sender.send(hosts);
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "remote host listing thread failed");
    }
    receiver
}

fn run_status() -> io::Result<Vec<String>> {
    let mut child = crate::child_process::command("tailscale")
        .args(["status", "--json"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        if let Some(mut stdout) = stdout {
            let _read = io::Read::read_to_end(&mut stdout, &mut output);
        }
        output
    });
    let started = std::time::Instant::now();
    while child.try_wait()?.is_none() {
        if started.elapsed() >= Duration::from_secs(3) {
            let _killed = child.kill();
            let _reaped = child.wait();
            return Err(io::Error::new(io::ErrorKind::TimedOut, "tailscale timed out"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = reader
        .join()
        .map_err(|_panic| io::Error::other("tailscale reader panicked"))?;
    let status = serde_json::from_slice(&output).map_err(io::Error::other)?;
    Ok(tailnet_peers(&status))
}

#[cfg(test)]
mod tests {
    use super::tailnet_peers;

    #[test]
    fn only_online_peers_are_suggested_by_magicdns_name() {
        let status = serde_json::json!({
            "Self": {"HostName": "me", "DNSName": "me.tail1.ts.net.", "Online": true},
            "Peer": {
                "k1": {"HostName": "laptop", "DNSName": "laptop.tail1.ts.net.", "Online": true},
                "k2": {"HostName": "desk", "DNSName": "", "Online": true},
                "k3": {"HostName": "asleep", "DNSName": "asleep.tail1.ts.net.", "Online": false}
            }
        });
        assert_eq!(tailnet_peers(&status), vec!["desk", "laptop.tail1.ts.net"]);
    }

    #[test]
    fn a_stopped_tailscale_gives_no_suggestions() {
        assert!(tailnet_peers(&serde_json::json!({"BackendState": "Stopped", "Peer": null})).is_empty());
        assert!(tailnet_peers(&serde_json::json!({})).is_empty());
    }
}
