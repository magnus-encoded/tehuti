//! Resolving a transcript that matched no command, from a model's answer.
//!
//! The model's answer arrives as an input; this module decides by confidence
//! band whether to run the command, offer a few for confirmation, or dictate.

/// How many commands an offer shows at most.
pub const MAX_OFFERED: usize = 3;

/// The window after a run in which an undo counts as a cancellation.
pub const CANCEL_WINDOW: core::time::Duration = core::time::Duration::from_secs(5);

/// One command and the probability thousandths the model gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ranked {
    pub command: String,
    pub probability: u16,
}

/// What the model answered: its chosen command, its confidence in thousandths,
/// and every command's probability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub command: String,
    pub confidence: u16,
    pub ranked: Vec<Ranked>,
}

/// The confidence thresholds, in thousandths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bands {
    /// At or above this the command runs.
    pub run_from: u16,
    /// At or above this, and below `run_from`, the top commands are offered.
    pub offer_from: u16,
}

impl Default for Bands {
    fn default() -> Self {
        Self {
            run_from: 800,
            offer_from: 500,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    Run(String),
    /// Two or three commands, the model's choice first.
    Offer(Vec<String>),
    /// Today's behaviour for unmatched speech.
    Dictate,
}

/// Decides what an unmatched transcript becomes.
///
/// Anything the model cannot back with a command of the focused context
/// dictates: no answer, an empty transcript, a command outside `commands`,
/// confidence below the offer band, or fewer than two known commands to offer.
#[must_use]
pub fn resolve(transcript: &str, commands: &[String], answer: Option<&Answer>, bands: Bands) -> Resolution {
    let Some(answer) = answer else {
        return Resolution::Dictate;
    };
    if transcript.trim().is_empty() || !commands.contains(&answer.command) {
        return Resolution::Dictate;
    }
    if answer.confidence >= bands.run_from {
        return Resolution::Run(answer.command.clone());
    }
    if answer.confidence < bands.offer_from {
        return Resolution::Dictate;
    }
    let mut others: Vec<&Ranked> = answer
        .ranked
        .iter()
        .filter(|ranked| ranked.command != answer.command && commands.contains(&ranked.command))
        .collect();
    others.sort_by(|left, right| right.probability.cmp(&left.probability));
    let offered: Vec<String> = core::iter::once(answer.command.clone())
        .chain(others.into_iter().map(|ranked| ranked.command.clone()))
        .fold(Vec::new(), |mut unique, command| {
            if !unique.contains(&command) {
                unique.push(command);
            }
            unique
        })
        .into_iter()
        .take(MAX_OFFERED)
        .collect();
    if offered.len() < 2 {
        Resolution::Dictate
    } else {
        Resolution::Offer(offered)
    }
}

/// What ran for a turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ran(String),
    Offered(Vec<String>),
    Dictated,
}

impl From<&Resolution> for Outcome {
    fn from(resolution: &Resolution) -> Self {
        match resolution {
            Resolution::Run(command) => Self::Ran(command.clone()),
            Resolution::Offer(commands) => Self::Offered(commands.clone()),
            Resolution::Dictate => Self::Dictated,
        }
    }
}

/// One line of the per-turn log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub heard: String,
    /// The model's choice and its confidence in thousandths, when it answered.
    pub chosen: Option<(String, u16)>,
    pub outcome: Outcome,
}

impl Turn {
    #[must_use]
    pub fn new(heard: &str, answer: Option<&Answer>, resolution: &Resolution) -> Self {
        Self {
            heard: heard.to_owned(),
            chosen: answer.map(|answer| (answer.command.clone(), answer.confidence)),
            outcome: Outcome::from(resolution),
        }
    }
}

/// Whether an undo `after` a run counts as cancelling it.
#[must_use]
pub fn cancelled_in_time(after: core::time::Duration) -> bool {
    after <= CANCEL_WINDOW
}
