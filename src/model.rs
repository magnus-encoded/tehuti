// SPDX-License-Identifier: MPL-2.0
use crate::spec::{CommandSpec, Move, ValueType, raw_move};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Draft {
    pub(super) stages: Vec<Stage>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Stage {
    pub program: Option<String>,
    pub items: Vec<Item>,
    pub nested: Option<Box<Self>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Item {
    pub definition: Move,
    pub value: Option<String>,
}

/// A hole-free invocation. Construction is only possible through `Draft::ready`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadyInvocation {
    draft: Draft,
}

impl ReadyInvocation {
    #[must_use]
    pub fn shell_text(&self) -> String {
        self.draft.preview()
    }

    #[must_use]
    pub fn argv(&self) -> Vec<Vec<String>> {
        self.draft.stages.iter().map(Stage::argv).collect()
    }

    #[must_use]
    pub const fn draft(&self) -> &Draft {
        &self.draft
    }
}

impl Draft {
    #[must_use]
    pub fn ready(&self) -> Option<ReadyInvocation> {
        (!self.stages.is_empty() && self.stages.iter().all(Stage::ready))
            .then(|| ReadyInvocation { draft: self.clone() })
    }

    #[must_use]
    pub fn preview(&self) -> String {
        self.stages
            .iter()
            .map(|stage| {
                stage
                    .argv()
                    .iter()
                    .enumerate()
                    .map(|(index, word)| {
                        if index == 0 && word.starts_with('-') {
                            format!("'{}'", word.replace('\'', "'\\''"))
                        } else {
                            quote(word)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }

    pub(super) fn stage(&self, address: &Address) -> Option<&Stage> {
        let mut stage = self.stages.get(address.top)?;
        for _ in 0..address.depth {
            stage = stage.nested.as_deref()?;
        }
        Some(stage)
    }

    pub(super) fn stage_mut(&mut self, address: &Address) -> Option<&mut Stage> {
        let mut stage = self.stages.get_mut(address.top)?;
        for _ in 0..address.depth {
            stage = stage.nested.as_deref_mut()?;
        }
        Some(stage)
    }
}

impl Item {
    fn argv(&self) -> Vec<String> {
        let mut words = Vec::new();
        if !self.definition.operand {
            words.push(self.definition.key.clone());
        }
        if self.definition.takes.is_some() {
            words.push(
                self.value
                    .clone()
                    .unwrap_or_else(|| format!("‹{}›", self.definition.key)),
            );
        }
        words
    }
}

impl Stage {
    pub(super) fn ready(&self) -> bool {
        self.program
            .as_ref()
            .is_some_and(|name| !name.is_empty() && !name.contains('\0'))
            && self.items.iter().all(|item| {
                !item.definition.key.contains('\0')
                    && (item.definition.takes.is_none()
                        || item
                            .value
                            .as_ref()
                            .is_some_and(|value| crate::machine::valid_value(&item.definition, value)))
            })
            && self.nested.as_ref().is_none_or(|nested| nested.ready())
    }

    pub(super) fn argv(&self) -> Vec<String> {
        let mut words = vec![self.program.clone().unwrap_or_else(|| "‹program›".into())];
        for item in self
            .items
            .iter()
            .filter(|item| self.nested.is_none() || item.definition.group != "raw")
        {
            words.extend(item.argv());
        }
        if let Some(nested) = &self.nested {
            words.extend(nested.argv());
            for item in self.items.iter().filter(|item| item.definition.group == "raw") {
                words.extend(item.argv());
            }
        }
        words
    }

    pub(super) fn set_program(&mut self, program: String, specs: &[CommandSpec]) {
        self.items.clear();
        self.nested = None;
        if let Some(spec) = specs.iter().find(|spec| spec.program == program) {
            self.items.extend(
                spec.moves
                    .iter()
                    .filter(|entry| entry.required)
                    .map(|entry| Item {
                        definition: entry.clone(),
                        value: None,
                    }),
            );
        }
        self.program = Some(program);
    }

    pub(super) fn moves(&self, specs: &[CommandSpec]) -> Vec<Move> {
        let mut moves = specs
            .iter()
            .find(|spec| Some(&spec.program) == self.program.as_ref())
            .map_or_else(Vec::new, |spec| spec.moves.clone());
        moves.retain(|entry| {
            (entry.repeat || !self.items.iter().any(|item| item.definition.key == entry.key))
                && (entry.takes != Some(ValueType::Command) || self.nested.is_none())
        });
        moves.push(raw_move());
        moves
    }

    pub(super) fn insert(&mut self, definition: Move, specs: &[CommandSpec]) -> usize {
        let order = specs
            .iter()
            .find(|spec| Some(&spec.program) == self.program.as_ref())
            .map(|spec| &spec.order);
        let rank = |group: &str| {
            order
                .and_then(|groups| groups.iter().position(|entry| entry == group))
                .unwrap_or(usize::MAX)
        };
        let index = self
            .items
            .iter()
            .position(|item| rank(&item.definition.group) > rank(&definition.group))
            .unwrap_or(self.items.len());
        self.items.insert(
            index,
            Item {
                definition,
                value: None,
            },
        );
        index
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Address {
    pub top: usize,
    pub depth: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Segment {
    Cwd,
    Pipe(usize),
    Program(Address),
    Item(Address, usize),
    End(Address),
}

pub(crate) fn segments(draft: &Draft) -> Vec<Segment> {
    let mut result = Vec::new();
    if !draft.stages.is_empty() {
        result.push(Segment::Cwd);
    }
    for (top, stage) in draft.stages.iter().enumerate() {
        if top > 0 {
            result.push(Segment::Pipe(top));
        }
        stage_segments(stage, &Address { top, depth: 0 }, &mut result);
        result.push(Segment::End(Address { top, depth: 0 }));
    }
    result
}

fn stage_segments(stage: &Stage, address: &Address, result: &mut Vec<Segment>) {
    result.push(Segment::Program(address.clone()));
    for (index, _) in stage
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| stage.nested.is_none() || item.definition.group != "raw")
    {
        result.push(Segment::Item(address.clone(), index));
    }
    if let Some(nested) = &stage.nested {
        stage_segments(
            nested,
            &Address {
                top: address.top,
                depth: address.depth.saturating_add(1),
            },
            result,
        );
        for (index, _) in stage
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.definition.group == "raw")
        {
            result.push(Segment::Item(address.clone(), index));
        }
    }
}

pub(crate) fn quote(word: &str) -> String {
    if !word.is_empty()
        && !word.contains('=')
        && !matches!(
            word,
            "case"
                | "do"
                | "done"
                | "elif"
                | "else"
                | "esac"
                | "fi"
                | "for"
                | "if"
                | "in"
                | "then"
                | "until"
                | "while"
                | "time"
        )
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte))
    {
        word.into()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::process::Command;

    proptest! {
        #[test]
        fn shell_preserves_every_argument(words in prop::collection::vec("[^\u{0}]{0,50}", 1..20), nested in any::<bool>(), piped in any::<bool>()) {
            let arguments = core::iter::once("%s\\0".to_owned()).chain(words.clone()).map(|word| Item {definition: raw_move(), value: Some(word)}).collect();
            let stage = Stage {program: Some("printf".into()), items: arguments, nested: None};
            let stage = if nested { Stage {program: Some("env".into()), items: Vec::new(), nested: Some(Box::new(stage))} } else {stage};
            let mut stages = vec![stage];
            if piped { stages.push(Stage {program: Some("cat".into()), ..Stage::default()}); }
            let draft = Draft {stages};
            let ready = draft.ready().ok_or_else(|| TestCaseError::fail("complete tree must be ready"))?;
            let result = Command::new("sh").args(["-c", &ready.shell_text()]).output()?;
            let expected: Vec<u8> = words.iter().flat_map(|word| word.bytes().chain(core::iter::once(0))).collect();
            prop_assert!(result.status.success());
            prop_assert_eq!(result.stdout, expected);
            let argv = ready.argv();
            prop_assert_eq!(argv.first().and_then(|argv| argv.first()).map(String::as_str), Some(if nested {"env"} else {"printf"}));
        }
    }
    #[test]
    fn program_names_cannot_be_shell_keywords_or_assignments() -> Result<(), Box<dyn core::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::TempDir::new()?;
        for program in ["if", "for", "then", "a=b", "two words", "-name"] {
            let path = directory.path().join(program);
            std::fs::write(&path, "#!/bin/sh\nprintf '%s\\0' \"$@\"\n")?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
            let stage = Stage {
                program: Some(program.into()),
                items: vec![Item {
                    definition: raw_move(),
                    value: Some("one word".into()),
                }],
                nested: None,
            };
            let draft = Draft { stages: vec![stage] };
            let result = Command::new("/bin/sh")
                .args(["-c", &draft.preview()])
                .env("PATH", directory.path())
                .output()?;
            assert!(
                result.status.success(),
                "program name {program} executes literally"
            );
            assert_eq!(
                result.stdout, b"one word\0",
                "the program receives the exact argument"
            );
        }
        Ok(())
    }
}
