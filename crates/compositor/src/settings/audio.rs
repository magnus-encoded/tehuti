use std::{io, process::Command, sync::mpsc, thread};
use tehuti_core::dispatch::{Sink, SinkKind};

#[derive(Debug, Default)]
pub(crate) struct Control {
    worker: Option<Worker>,
    unavailable: bool,
    pending: usize,
}

#[derive(Debug)]
struct Worker {
    requests: mpsc::Sender<Option<u32>>,
    replies: mpsc::Receiver<io::Result<Vec<Sink>>>,
}

impl Control {
    pub(crate) fn request(&mut self, sink: Option<u32>) {
        if self.worker.is_none() {
            let (requests, receiver) = mpsc::channel();
            let (sender, replies) = mpsc::channel();
            match thread::Builder::new()
                .name("audio-control".into())
                .spawn(move || {
                    while let Ok(requested_sink) = receiver.recv() {
                        let result = update(requested_sink, &mut run);
                        if let Err(error) = &result {
                            tracing::warn!(%error, "audio control unavailable");
                        }
                        if sender.send(result).is_err() {
                            break;
                        }
                    }
                }) {
                Ok(_worker) => self.worker = Some(Worker { requests, replies }),
                Err(error) => {
                    tracing::warn!(%error, "audio worker unavailable");
                    self.unavailable = true;
                }
            }
        }
        if let Some(worker) = &self.worker {
            self.unavailable = worker.requests.send(sink).is_err();
            if !self.unavailable {
                self.pending = self.pending.saturating_add(1);
            }
        }
    }

    pub(crate) fn poll(&mut self) -> Option<io::Result<Vec<Sink>>> {
        if core::mem::take(&mut self.unavailable) {
            return Some(Err(io::Error::other("audio worker unavailable")));
        }
        let worker = self.worker.as_ref()?;
        let mut latest = None;
        for reply in worker.replies.try_iter() {
            self.pending = self.pending.saturating_sub(1);
            if self.pending == 0 {
                latest = Some(reply);
            }
        }
        latest
    }
}

fn run(arguments: &[String]) -> io::Result<String> {
    let output = Command::new("wpctl")
        .env("LC_ALL", "C")
        .args(arguments)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("wpctl failed"));
    }
    String::from_utf8(output.stdout).map_err(io::Error::other)
}

pub(crate) fn update(
    sink: Option<u32>,
    command: &mut impl FnMut(&[String]) -> io::Result<String>,
) -> io::Result<Vec<Sink>> {
    if let Some(sink_id) = sink {
        let _output = command(&["set-default".into(), sink_id.to_string()])?;
    }
    parse_status(&command(&["status".into()])?)
}

fn kind(description: &str) -> SinkKind {
    let description = description.to_lowercase();
    if ["hdmi", "displayport", "display port"]
        .iter()
        .any(|word| description.contains(word))
    {
        SinkKind::Hdmi
    } else if ["headset", "headphone", "usb", "bluetooth"]
        .iter()
        .any(|word| description.contains(word))
    {
        SinkKind::Headset
    } else if ["analog", "built-in", "speaker"]
        .iter()
        .any(|word| description.contains(word))
    {
        SinkKind::Speakers
    } else {
        SinkKind::Other
    }
}

pub(crate) fn parse_status(output: &str) -> io::Result<Vec<Sink>> {
    let mut audio = false;
    let mut sinks = false;
    let mut found = false;
    let mut result = Vec::new();
    for line in output.lines() {
        let heading =
            line.trim_matches(|character: char| character.is_whitespace() || "│├└─".contains(character));
        if heading == "Audio" {
            audio = true;
            continue;
        }
        if !line.starts_with(char::is_whitespace) && !heading.is_empty() {
            audio = false;
        }
        if !audio {
            continue;
        }
        if heading == "Sinks:" {
            sinks = true;
            found = true;
            continue;
        }
        if heading.ends_with(':') {
            sinks = false;
        }
        if !sinks || heading.is_empty() {
            continue;
        }
        let is_default = heading.starts_with('*');
        let entry = heading.trim_start_matches('*').trim();
        let Some((sink_id, description)) = entry.split_once('.') else {
            continue;
        };
        let sink_id = sink_id.trim().parse().map_err(io::Error::other)?;
        let description = description.split(" [").next().unwrap_or(description).trim();
        if description.is_empty() {
            return Err(io::Error::other("sink description missing"));
        }
        result.push(Sink {
            sink_id,
            description: description.into(),
            kind: kind(description),
            is_default,
        });
    }
    if !found {
        return Err(io::Error::other("audio sinks missing"));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = include_str!("fixtures/wpctl-status.txt");

    #[test]
    fn status_lists_only_audio_sinks_and_marks_default() -> io::Result<()> {
        assert_eq!(
            parse_status(STATUS)?,
            vec![
                Sink {
                    sink_id: 52,
                    description: "USB Headset Analog Stereo".into(),
                    kind: SinkKind::Headset,
                    is_default: true
                },
                Sink {
                    sink_id: 61,
                    description: "HDMI / DisplayPort 2".into(),
                    kind: SinkKind::Hdmi,
                    is_default: false
                },
                Sink {
                    sink_id: 45,
                    description: "Built-in Audio Analog Stereo".into(),
                    kind: SinkKind::Speakers,
                    is_default: false
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn device_kinds_cover_output_transports() {
        for (description, expected) in [
            ("USB DAC", SinkKind::Headset),
            ("Headphones", SinkKind::Headset),
            ("DisplayPort", SinkKind::Hdmi),
            ("Speakers", SinkKind::Speakers),
            ("Network audio", SinkKind::Other),
        ] {
            assert_eq!(kind(description), expected);
        }
    }

    #[test]
    fn selection_sets_only_the_sink_default() -> io::Result<()> {
        let mut commands = Vec::new();
        let sinks = update(Some(61), &mut |arguments| {
            commands.push(arguments.to_vec());
            Ok(STATUS.into())
        })?;
        assert_eq!(commands, vec![vec!["set-default", "61"], vec!["status"]]);
        assert_eq!(sinks.len(), 3);
        Ok(())
    }

    #[test]
    fn missing_failed_and_invalid_control_are_unavailable() {
        for error in [io::ErrorKind::NotFound, io::ErrorKind::Other] {
            assert!(update(None, &mut |_| Err(io::Error::from(error))).is_err());
        }
        assert!(parse_status("invalid output").is_err());
        assert_eq!(
            parse_status("Audio\n ├─ Sinks:\n ├─ Sources:\n").ok(),
            Some(Vec::new())
        );
    }
}
