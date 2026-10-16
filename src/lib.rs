pub mod machine;
pub mod model;
pub mod readers;
pub mod screen;
pub mod spec;

pub use machine::{Button, Effect, HistoryRow, Input, State, step};
pub use model::{Draft, ReadyInvocation};
pub use readers::{
    Candidate, Confidence, Environment, parse_words, read_value, spelled, spoken_words, strip_transcript,
};
pub use screen::{Screen, ScreenSegment};
pub use spec::{CommandSpec, Move, ValueType, specs};

#[cfg(test)]
mod tests;
