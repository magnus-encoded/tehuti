//! Audio sources: what Tehuti can take audio from for speech, sorted by where the sound comes from.

/// The lifecycle shared by everything Tehuti listens to or reads from (audio sources, pads).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Presence {
    /// Seen, not yet usable: a relay waits for its phone, a device waits for its grab.
    Connecting,
    /// Usable now.
    #[default]
    Present,
    /// Vanished or never listed; known only from saved settings.
    Gone,
}

impl Presence {
    /// The word shown under a tile; empty while present.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Connecting => "CONNECTING",
            Self::Present => "",
            Self::Gone => "GONE",
        }
    }
}

/// Where the sound comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A local device that picks up sound in the room.
    Microphone,
    /// Audio that originates elsewhere.
    Stream(StreamOrigin),
}

/// What a Stream carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamOrigin {
    /// A phone sending through the relay player.
    Relay,
    /// A network stream such as RTP.
    Network,
    /// A monitor of the machine's own output.
    Loopback,
    /// A fixed clip, used by tests and recorded commands.
    Clip,
}

impl SourceKind {
    /// The word shown on the tile.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Microphone => "MIC",
            Self::Stream(_) => "STREAM",
        }
    }

    /// Whether listening would hear Tehuti's own machine, so speech could trigger itself.
    #[must_use]
    pub const fn loops_back(self) -> bool {
        matches!(self, Self::Stream(StreamOrigin::Loopback))
    }
}

/// One audio source as the Input page and speech name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSource {
    /// The stable key: the `PipeWire` node name today.
    pub key: String,
    pub description: String,
    pub kind: SourceKind,
    pub presence: Presence,
}

impl AudioSource {
    /// Sorts a `PipeWire` node by what it carries, not by how `PipeWire` lists it.
    #[must_use]
    pub fn classify(key: &str, description: &str, relay: Option<Presence>) -> Self {
        let lowered = format!("{key} {description}").to_lowercase();
        let has = |words: &[&str]| words.iter().any(|word| lowered.contains(word));
        let (kind, presence) = if let Some(presence) = relay {
            (SourceKind::Stream(StreamOrigin::Relay), presence)
        } else if key.ends_with(".monitor") || has(&["monitor of", "loopback"]) {
            (SourceKind::Stream(StreamOrigin::Loopback), Presence::Present)
        } else if has(&["rtp", "network", "tunnel", "stream", "raop"]) {
            (SourceKind::Stream(StreamOrigin::Network), Presence::Present)
        } else {
            (SourceKind::Microphone, Presence::Present)
        };
        Self {
            key: key.to_owned(),
            description: description.to_owned(),
            kind,
            presence,
        }
    }

    /// A source known only from saved settings.
    #[must_use]
    pub fn gone(key: &str) -> Self {
        Self {
            presence: Presence::Gone,
            ..Self::classify(key, key, None)
        }
    }
}

/// A fixed clip as a Stream: tests drive voice commands through it without a microphone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipSource {
    samples: Vec<i16>,
    position: usize,
}

impl ClipSource {
    #[must_use]
    pub const fn new(samples: Vec<i16>) -> Self {
        Self { samples, position: 0 }
    }

    /// The source entry for this clip.
    #[must_use]
    pub fn source(key: &str) -> AudioSource {
        AudioSource {
            key: key.to_owned(),
            description: key.to_owned(),
            kind: SourceKind::Stream(StreamOrigin::Clip),
            presence: Presence::Present,
        }
    }

    /// The next up to `len` samples; empty once the clip has played out.
    pub fn next_chunk(&mut self, len: usize) -> &[i16] {
        let start = self.position.min(self.samples.len());
        let end = start.saturating_add(len).min(self.samples.len());
        self.position = end;
        self.samples.get(start..end).unwrap_or_default()
    }

    /// Plays the clip again from its start.
    pub const fn rewind(&mut self) {
        self.position = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn chunks_of_any_size_replay_the_whole_clip(
            samples in prop::collection::vec(any::<i16>(), 0..200),
            sizes in prop::collection::vec(1_usize..40, 1..30),
        ) {
            let mut clip = ClipSource::new(samples.clone());
            let mut heard = Vec::new();
            for size in sizes.iter().cycle().take(400) {
                let chunk = clip.next_chunk(*size);
                prop_assert!(chunk.len() <= *size);
                heard.extend_from_slice(chunk);
            }
            prop_assert_eq!(heard, samples);
        }

        #[test]
        fn a_monitor_node_is_always_a_looping_stream(name in "[a-z_.]{0,20}", description in "[A-Za-z ]{0,20}") {
            let source = AudioSource::classify(&format!("{name}.monitor"), &description, None);
            prop_assert!(source.kind.loops_back());
        }

        #[test]
        fn a_relay_is_a_stream_with_the_presence_given(connected in any::<bool>()) {
            let presence = if connected { Presence::Present } else { Presence::Connecting };
            let source = AudioSource::classify("relay", "Relay", Some(presence));
            prop_assert_eq!(source.kind, SourceKind::Stream(StreamOrigin::Relay));
            prop_assert_eq!(source.presence, presence);
        }
    }

    #[test]
    fn a_usb_device_is_a_microphone_and_a_network_feed_a_stream() {
        assert_eq!(AudioSource::classify("alsa_input.usb", "USB Mic", None).kind, SourceKind::Microphone);
        assert_eq!(
            AudioSource::classify("rtp_source", "RTP", None).kind,
            SourceKind::Stream(StreamOrigin::Network)
        );
        assert!(!AudioSource::classify("rtp_source", "RTP", None).kind.loops_back());
    }

    #[test]
    fn a_saved_source_that_is_not_listed_is_gone() {
        assert_eq!(AudioSource::gone("old").presence, Presence::Gone);
    }

    #[test]
    fn the_clip_stream_is_a_present_stream() {
        let source = ClipSource::source("clip");
        assert_eq!(source.kind, SourceKind::Stream(StreamOrigin::Clip));
        assert_eq!(source.presence, Presence::Present);
    }
}
