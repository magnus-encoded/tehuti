use crate::machine::State;
use crate::model::{Segment, segments};
use crate::{Candidate, HistoryRow, Move, ValueType};
use core::fmt::Write;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenSegment {
    pub label: String,
    pub focused: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    pub history: Vec<HistoryRow>,
    pub history_focus: usize,
    pub preview: String,
    pub segments: Vec<ScreenSegment>,
    pub description: String,
    pub list: Vec<Candidate>,
    pub list_focus: usize,
    pub ready: bool,
    pub notice: String,
}

impl State {
    #[must_use]
    pub fn screen(&self) -> Screen {
        let mut description = String::new();
        let segments = segments(&self.draft).iter().enumerate().map(|(index, segment)| {
            let (label, detail) = match segment {
                Segment::Cwd => (self.cwd.clone(), format!("Working folder {}.", self.cwd)),
                Segment::Pipe(_) => ("┃".into(), "Pipe: output on the left feeds the stage on the right.".into()),
                Segment::End(_) => ("＋".into(), "Add to this stage: say an option, its purpose or an operand; DOWN browses. RB adds a pipe.".into()),
                Segment::Program(address) => {
                    let program = self.draft.stage(address).and_then(|stage| stage.program.clone());
                    let summary = self.specs.iter().find(|spec| Some(&spec.program) == program.as_ref()).map_or("no spec; raw arguments only", |spec| spec.summary.as_str());
                    (program.clone().unwrap_or_else(|| "‹program›".into()), program.map_or_else(|| "Program: say a whole command, e.g. find src dash name star dot r s. A accepts the tree; A again runs it. Or say its name; DOWN browses.".into(), |program| format!("Program {program}: {summary}.")))
                }
                Segment::Item(address, item) => self.draft.stage(address).and_then(|stage| stage.items.get(*item)).map_or_else(|| (String::new(), String::new()), |item| {
                    let value = item.value.clone().unwrap_or_else(|| format!("‹{}›", item.definition.key));
                    let label = if item.definition.operand {value} else if item.definition.takes.is_some() {format!("{} {value}", item.definition.key)} else {item.definition.key.clone()};
                    (label, describe(&item.definition))
                }),
            };
            let focused = index == self.focus && self.row == 0;
            if focused { description = detail; }
            ScreenSegment {label, focused}
        }).collect();
        Screen {
            history: self.history.clone(),
            history_focus: self.row,
            preview: self.draft.preview(),
            segments,
            description,
            list: self.list.as_ref().map_or_else(Vec::new, |list| {
                list.entries.iter().map(|entry| entry.candidate.clone()).collect()
            }),
            list_focus: self.list.as_ref().map_or(0, |list| list.selected),
            ready: self.draft.ready().is_some(),
            notice: self.notice.clone(),
        }
    }
}

impl core::fmt::Display for Screen {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut text = String::from("HISTORY\n");
        self.write_history(&mut text)?;
        writeln!(text, "PREVIEW  $ {}", self.preview)?;
        writeln!(
            text,
            "BUILDER{}",
            if self.history_focus > 0 {
                "  (focus is on a history row)"
            } else {
                ""
            }
        )?;
        writeln!(
            text,
            "> {}",
            if self.segments.is_empty() {
                "(no command: press Y)".into()
            } else {
                self.segments
                    .iter()
                    .map(|segment| {
                        if segment.focused {
                            format!("[{}]", segment.label)
                        } else {
                            segment.label.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ▶ ")
            }
        )?;
        writeln!(text, "FOCUS  {}", self.description)?;
        writeln!(text, "LIST: UP/DOWN choose, A accept, B close")?;
        for (index, candidate) in self.list.iter().enumerate() {
            writeln!(
                text,
                "  {} {:14} {:?}  {}",
                if index == self.list_focus { ">" } else { " " },
                candidate.value,
                candidate.confidence,
                candidate.note
            )?;
        }
        writeln!(
            text,
            "STATUS {}",
            if self.ready {
                "ready: A runs it"
            } else {
                "not ready: empty slot(s)"
            }
        )?;
        writeln!(text, "NOTICE {}", self.notice)?;
        formatter.write_str(&text)
    }
}

impl Screen {
    fn write_history(&self, text: &mut String) -> core::fmt::Result {
        if self.history.is_empty() {
            writeln!(text, "  (empty)")?;
        }
        let start = self.history.len().saturating_sub(5);
        for (index, row) in self.history.iter().enumerate().skip(start) {
            let focused = self.history_focus == self.history.len().saturating_sub(index);
            writeln!(
                text,
                "{} {}    {} {}",
                if focused { ">" } else { " " },
                row.text,
                if row.exit == 0 { "✓" } else { "✗" },
                row.exit
            )?;
            for line in row.output.lines().take(12) {
                writeln!(text, "      {line}")?;
            }
        }
        Ok(())
    }
}

fn describe(definition: &Move) -> String {
    let prompt = match definition.takes {
        Some(ValueType::Path) => "A path: say its name or DOWN to browse.",
        Some(ValueType::Glob) => "A shell pattern: spell symbols (star, dot, slash), or type it.",
        Some(ValueType::Regex) => "A search pattern: say or type it.",
        Some(ValueType::Integer) => "A whole number: say or type it.",
        Some(ValueType::Enum) => "Choose a value with speech or DOWN to browse.",
        Some(ValueType::String) => "Text: say or type it.",
        Some(ValueType::Command) => "A nested command: say its program.",
        None => "A flag; it takes no value.",
    };
    format!("{}: {}. {prompt}", definition.key, definition.summary)
}
