use crate::model::{Address, Draft, ReadyInvocation, Segment, Stage, segments};
use crate::readers::{
    Candidate, Environment, parse_words, ranked, read_value, similarity, spelled, spoken_words,
    strip_transcript,
};
use crate::spec::{CommandSpec, Move, ValueType, raw_move};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Button {
    #[serde(rename = "A")]
    Accept,
    #[serde(rename = "B")]
    Back,
    #[serde(rename = "X")]
    Delete,
    #[serde(rename = "Y")]
    New,
    #[serde(rename = "LB")]
    PreviousStage,
    #[serde(rename = "RB")]
    NextStage,
    #[serde(rename = "UP")]
    ListUp,
    Down,
    Left,
    Right,
    LsUp,
    LsDown,
    LsLeft,
    LsRight,
}

impl Button {
    /// # Errors
    /// Returns an error for an unknown controller button.
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_value(serde_json::Value::String(text.to_uppercase()))
    }
}

#[derive(Clone, Debug)]
pub enum Input {
    Press(Button),
    Say(String),
    Type(String),
    Environment(Environment),
    Completed {
        invocation: ReadyInvocation,
        exit: i32,
        output: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Run(ReadyInvocation),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct HistoryRow {
    pub tree: Draft,
    pub text: String,
    pub exit: i32,
    pub output: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum Choice {
    Program(Address, String),
    Whole(Address, Stage),
    Move(Address, Move),
    Value(Address, usize, String),
    Operand(Address, Move, String),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Entry {
    pub candidate: Candidate,
    pub choice: Choice,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct OpenList {
    pub entries: Vec<Entry>,
    pub selected: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct State {
    pub(super) cwd: String,
    pub(super) draft: Draft,
    pub(super) focus: usize,
    pub(super) row: usize,
    pub(super) history: Vec<HistoryRow>,
    pub(super) undo: Vec<(Draft, usize)>,
    pub(super) list: Option<OpenList>,
    pub(super) notice: String,
    pub(super) env: Environment,
    pub(super) specs: Vec<CommandSpec>,
}

impl State {
    #[must_use]
    pub fn new(cwd: String, specs: Vec<CommandSpec>) -> Self {
        Self {
            cwd,
            specs,
            draft: Draft::default(),
            focus: 0,
            row: 0,
            history: Vec::new(),
            undo: Vec::new(),
            list: None,
            notice: "Press Y for a new command. Then say a whole command; A accepts it, A again runs it."
                .into(),
            env: Environment::default(),
        }
    }

    #[must_use]
    pub const fn draft(&self) -> &Draft {
        &self.draft
    }

    #[must_use]
    pub fn history(&self) -> &[HistoryRow] {
        &self.history
    }

    /// Validate persisted state at the host boundary.
    #[must_use]
    pub fn valid(&self) -> bool {
        fn stage_valid(stage: &Stage, depth: usize) -> bool {
            depth < 32
                && stage
                    .program
                    .as_ref()
                    .is_none_or(|program| !program.is_empty() && !program.contains('\0'))
                && (stage.program.is_some() || stage.items.is_empty() && stage.nested.is_none())
                && stage.items.iter().all(|item| {
                    item.definition.takes != Some(ValueType::Command)
                        && !item.definition.key.contains('\0')
                        && item
                            .value
                            .as_ref()
                            .is_none_or(|value| valid_value(&item.definition, value))
                })
                && stage
                    .nested
                    .as_ref()
                    .is_none_or(|nested| stage_valid(nested, depth.saturating_add(1)))
        }
        let valid_draft = |draft: &Draft| {
            draft.stages.len() <= 64 && draft.stages.iter().all(|stage| stage_valid(stage, 0))
        };
        valid_draft(&self.draft)
            && self.focus < segments(&self.draft).len().max(1)
            && self.row <= self.history.len()
            && self
                .undo
                .iter()
                .all(|(draft, focus)| valid_draft(draft) && *focus < segments(draft).len().max(1))
            && self
                .history
                .iter()
                .all(|entry| valid_draft(&entry.tree) && entry.tree.ready().is_some())
            && self.list.as_ref().is_none_or(|list| {
                !list.entries.is_empty()
                    && list.selected < list.entries.len()
                    && list.entries.iter().all(|entry| self.valid_choice(&entry.choice))
            })
    }

    fn valid_choice(&self, choice: &Choice) -> bool {
        let address = match choice {
            Choice::Program(address, _)
            | Choice::Whole(address, _)
            | Choice::Move(address, _)
            | Choice::Value(address, _, _)
            | Choice::Operand(address, _, _) => address,
        };
        self.draft.stage(address).is_some()
    }

    fn focused(&self) -> Option<Segment> {
        (self.row == 0)
            .then(|| segments(&self.draft).get(self.focus).cloned())
            .flatten()
    }

    fn clamp(&mut self) {
        self.focus = self.focus.min(segments(&self.draft).len().saturating_sub(1));
        self.row = self.row.min(self.history.len());
    }

    fn after_fill(&mut self, top: usize) {
        let segs = segments(&self.draft);
        self.focus = segs
            .iter()
            .position(|segment| match segment {
                Segment::Program(address) => self
                    .draft
                    .stage(address)
                    .is_some_and(|stage| stage.program.is_none()),
                Segment::Item(address, index) => self
                    .draft
                    .stage(address)
                    .and_then(|stage| stage.items.get(*index))
                    .is_some_and(|item| item.definition.takes.is_some() && item.value.is_none()),
                Segment::Cwd | Segment::Pipe(_) | Segment::End(_) => false,
            })
            .or_else(|| {
                segs.iter()
                    .position(|segment| matches!(segment, Segment::End(address) if address.top == top))
            })
            .unwrap_or(0);
    }

    fn open(&mut self, entries: Vec<Entry>) {
        self.list = (!entries.is_empty()).then_some(OpenList { entries, selected: 0 });
        if self.list.is_none() {
            self.notice = "Nothing to choose here.".into();
        }
    }

    fn fill_operand(&mut self, address: &Address, definition: Move, value: String) {
        if let Some(stage) = self.draft.stage_mut(address) {
            if let Some(item) = stage
                .items
                .iter_mut()
                .find(|item| item.definition.key == definition.key && item.value.is_none())
            {
                item.value = Some(value);
            } else {
                let index = stage.insert(definition, &self.specs);
                if let Some(item) = stage.items.get_mut(index) {
                    item.value = Some(value);
                }
            }
        }
    }

    fn apply(&mut self, choice: Choice) {
        let top = match &choice {
            Choice::Program(address, _)
            | Choice::Whole(address, _)
            | Choice::Move(address, _)
            | Choice::Value(address, _, _)
            | Choice::Operand(address, _, _) => address.top,
        };
        match choice {
            Choice::Program(address, name) => {
                if let Some(stage) = self.draft.stage_mut(&address) {
                    stage.set_program(name, &self.specs);
                }
            }
            Choice::Whole(address, parsed) => {
                if let Some(stage) = self.draft.stage_mut(&address) {
                    *stage = parsed;
                }
            }
            Choice::Move(address, definition) => {
                if let Some(stage) = self.draft.stage_mut(&address) {
                    if definition.takes == Some(ValueType::Command) && address.depth >= 30 {
                        return;
                    }
                    if definition.takes == Some(ValueType::Command) && address.depth < 30 {
                        stage.nested = Some(Box::default());
                    } else {
                        let takes = definition.takes;
                        let index = stage.insert(definition, &self.specs);
                        if takes.is_some() {
                            self.focus = segments(&self.draft)
                                .iter()
                                .position(|segment| *segment == Segment::Item(address.clone(), index))
                                .unwrap_or(0);
                            return;
                        }
                    }
                }
            }
            Choice::Value(address, index, value) => {
                if let Some(item) = self
                    .draft
                    .stage_mut(&address)
                    .and_then(|stage| stage.items.get_mut(index))
                    && valid_value(&item.definition, &value)
                {
                    item.value = Some(value);
                }
            }
            Choice::Operand(address, definition, value) => {
                self.fill_operand(&address, definition, value);
            }
        }
        self.after_fill(top);
    }

    fn program_entries(&self, address: &Address, text: Option<&str>) -> Vec<Entry> {
        if text.is_none() {
            return self
                .specs
                .iter()
                .map(|spec| Entry {
                    candidate: Candidate {
                        value: spec.program.clone(),
                        confidence: crate::Confidence::High,
                        note: spec.summary.clone(),
                    },
                    choice: Choice::Program(address.clone(), spec.program.clone()),
                })
                .collect();
        }
        let mut programs = self.env.programs.clone();
        programs.extend(self.specs.iter().map(|spec| spec.program.clone()));
        programs.sort();
        programs.dedup();
        let values = ranked(
            programs
                .into_iter()
                .map(|program| {
                    let score = text.map_or(100, |heard| similarity(strip_transcript(heard), &program));
                    let note = self
                        .specs
                        .iter()
                        .find(|spec| spec.program == program)
                        .map_or_else(String::new, |spec| spec.summary.clone());
                    (program, score, note)
                })
                .collect(),
        );
        values
            .into_iter()
            .map(|candidate| Entry {
                choice: Choice::Program(address.clone(), candidate.value.clone()),
                candidate,
            })
            .collect()
    }

    fn choose(&mut self, text: Option<&str>) {
        let entries = match self.focused() {
            Some(Segment::Program(address)) => {
                if let Some(text) = text {
                    let words = spoken_words(text);
                    if words.len() > 1
                        && let Some(parsed) = parse_stage(&words, &self.specs, &self.env, 0)
                    {
                        self.open(vec![Entry {
                            candidate: Candidate {
                                value: parsed.argv().join(" "),
                                confidence: crate::Confidence::High,
                                note: "whole command".into(),
                            },
                            choice: Choice::Whole(address, parsed),
                        }]);
                        return;
                    }
                }
                self.program_entries(&address, text)
            }
            Some(Segment::Item(address, index)) => self
                .draft
                .stage(&address)
                .and_then(|stage| stage.items.get(index))
                .map_or_else(Vec::new, |item| {
                    read_value(&item.definition, text, &self.env)
                        .into_iter()
                        .map(|candidate| Entry {
                            choice: Choice::Value(address.clone(), index, candidate.value.clone()),
                            candidate,
                        })
                        .collect()
                }),
            Some(Segment::End(address)) => self.move_entries(&address, text),
            Some(Segment::Cwd | Segment::Pipe(_)) | None => Vec::new(),
        };
        self.open(entries);
    }

    fn operand_entry(
        &self,
        address: &Address,
        definition: &Move,
        text: Option<&str>,
    ) -> Option<(usize, Entry)> {
        let heard = text?;
        if !definition.operand || definition.takes == Some(ValueType::Command) || definition.key == "argument"
        {
            return None;
        }
        let value = read_value(definition, Some(heard), &self.env)
            .into_iter()
            .next()?;
        let score = if value.confidence == crate::Confidence::High {
            99
        } else {
            40
        };
        Some((
            score,
            Entry {
                choice: Choice::Operand(address.clone(), definition.clone(), value.value.clone()),
                candidate: value,
            },
        ))
    }

    fn move_entries(&self, root: &Address, text: Option<&str>) -> Vec<Entry> {
        let mut scored = Vec::new();
        let mut address = root.clone();
        while let Some(stage) = self.draft.stage(&address) {
            if stage.program.is_none() {
                break;
            }
            for (position, definition) in stage.moves(&self.specs).into_iter().enumerate() {
                let score = move_score(&definition, position, text);
                let candidate = Candidate {
                    value: definition.key.clone(),
                    confidence: if score >= 90 {
                        crate::Confidence::High
                    } else if score >= 65 {
                        crate::Confidence::Medium
                    } else {
                        crate::Confidence::Low
                    },
                    note: format!(
                        "{}: {}",
                        stage.program.as_deref().unwrap_or(""),
                        definition.summary
                    ),
                };
                scored.push((
                    score,
                    Entry {
                        candidate,
                        choice: Choice::Move(address.clone(), definition.clone()),
                    },
                ));
                if let Some(entry) = self.operand_entry(&address, &definition, text) {
                    scored.push(entry);
                }
            }
            if stage.nested.is_none() {
                break;
            }
            address.depth = address.depth.saturating_add(1);
        }
        scored.sort_by_key(|entry| core::cmp::Reverse(entry.0));
        if text.is_some() {
            scored.truncate(8);
        }
        scored.into_iter().map(|(_, entry)| entry).collect()
    }

    fn typed(&mut self, text: String) {
        if text.contains('\0') {
            self.notice = "NUL cannot occur in an argument.".into();
            return;
        }
        self.list = None;
        match self.focused() {
            Some(Segment::Program(address)) => match parse_words(&text) {
                Ok(words) if words.len() > 1 => {
                    if let Some(parsed) = parse_stage(&words, &self.specs, &self.env, 0) {
                        self.apply(Choice::Whole(address, parsed));
                    }
                }
                Ok(words) if !words.is_empty() => {
                    if let Some(program) = words.into_iter().next() {
                        self.apply(Choice::Program(address, program));
                    }
                }
                Ok(_) => self.notice = "A program name cannot be empty.".into(),
                Err(error) => self.notice = error,
            },
            Some(Segment::Item(address, index)) => self.apply(Choice::Value(address, index, text)),
            Some(Segment::End(address)) => {
                let entry = self
                    .move_entries(&address, None)
                    .into_iter()
                    .find(|entry| entry.candidate.value == text);
                if let Some(entry) = entry {
                    self.apply(entry.choice);
                } else if let Some(stage) = self.draft.stage_mut(&address)
                    && stage.program.is_some()
                {
                    let definition = stage
                        .moves(&self.specs)
                        .into_iter()
                        .find(|definition| {
                            definition.operand
                                && !text.starts_with('-')
                                && definition.key != "argument"
                                && definition.takes != Some(ValueType::Command)
                                && valid_value(definition, &text)
                        })
                        .unwrap_or_else(raw_move);
                    self.apply(Choice::Operand(address, definition, text));
                }
            }
            Some(Segment::Cwd | Segment::Pipe(_)) | None => {
                self.notice = "Typing goes to the builder's focused segment.".into();
            }
        }
    }

    fn navigate(&mut self, target: usize) {
        let old = self.focused();
        let segs = segments(&self.draft);
        let target = segs.get(target.min(segs.len().saturating_sub(1))).cloned();
        self.list = None;
        self.focus = segs
            .iter()
            .position(|segment| Some(segment) == target.as_ref())
            .unwrap_or(0);
        let old_top = segment_top(old.as_ref());
        if let Some(top) = old_top
            && self.draft.stages.len() > 1
            && self
                .draft
                .stages
                .get(top)
                .is_some_and(|stage| stage.program.is_none())
            && segment_top(target.as_ref()) != Some(top)
        {
            let _removed = self.draft.stages.remove(top);
            self.focus = self.focus.saturating_sub(
                usize::from(segment_top(target.as_ref()).is_some_and(|next| next > top)).saturating_mul(3),
            );
        }
    }

    fn delete(&mut self) {
        self.list = None;
        match self.focused() {
            Some(Segment::Pipe(top)) => {
                let _removed = self.draft.stages.remove(top);
            }
            Some(Segment::Program(address)) => {
                if let Some(stage) = self.draft.stage_mut(&address) {
                    *stage = Stage::default();
                }
            }
            Some(Segment::Item(address, index)) => {
                if let Some(stage) = self.draft.stage_mut(&address)
                    && let Some(item) = stage.items.get_mut(index)
                {
                    if item.definition.required {
                        item.value = None;
                    } else {
                        let _removed = stage.items.remove(index);
                    }
                }
            }
            Some(Segment::Cwd | Segment::End(_)) | None => self.notice = "Nothing to delete here.".into(),
        }
        self.focus = self.focus.saturating_sub(1);
    }

    fn jump_stage(&mut self, button: Button) {
        let Some(top) = segment_top(self.focused().as_ref()) else {
            self.navigate(1);
            return;
        };
        if button == Button::NextStage && top.saturating_add(1) >= self.draft.stages.len() {
            if self
                .draft
                .stages
                .last()
                .is_some_and(|stage| stage.program.is_some())
                && self.draft.stages.len() < 64
            {
                self.draft.stages.push(Stage::default());
                self.list = None;
                self.after_fill(self.draft.stages.len().saturating_sub(1));
            }
        } else {
            let next = if button == Button::NextStage {
                top.saturating_add(1)
            } else {
                top.saturating_sub(1)
            };
            let index = segments(&self.draft)
                .iter()
                .position(|segment| *segment == Segment::Program(Address { top: next, depth: 0 }))
                .unwrap_or(0);
            self.navigate(index);
        }
    }

    fn move_button(&mut self, button: Button) {
        match button {
            Button::ListUp | Button::Down => {
                if let Some(list) = &mut self.list {
                    list.selected = if button == Button::Down {
                        list.selected
                            .saturating_add(1)
                            .checked_rem(list.entries.len())
                            .unwrap_or(0)
                    } else if list.selected == 0 {
                        list.entries.len().saturating_sub(1)
                    } else {
                        list.selected.saturating_sub(1)
                    };
                } else {
                    self.choose(None);
                }
            }
            Button::Left | Button::Right | Button::LsLeft | Button::LsRight => {
                let distance = if matches!(button, Button::LsLeft | Button::LsRight) {
                    3
                } else {
                    1
                };
                let target = if matches!(button, Button::Left | Button::LsLeft) {
                    self.focus.saturating_sub(distance)
                } else {
                    self.focus.saturating_add(distance)
                };
                self.navigate(target);
            }
            Button::PreviousStage | Button::NextStage => {
                self.jump_stage(button);
            }
            Button::Accept | Button::Back | Button::Delete | Button::New | Button::LsUp | Button::LsDown => {}
        }
    }

    fn history_button(&mut self, button: Button) {
        if button == Button::Accept
            && let Some(entry) = self.history.get(self.history.len().saturating_sub(self.row))
        {
            self.draft = entry.tree.clone();
            self.row = 0;
            self.list = None;
            self.after_fill(self.draft.stages.len().saturating_sub(1));
        }
    }

    fn press(&mut self, button: Button) -> Vec<Effect> {
        if button == Button::New {
            self.draft = Draft {
                stages: vec![Stage::default()],
            };
            self.focus = 1;
            self.notice = "Say a whole command (for example: find src dash name star dot r s). A accepts the tree; A again runs it.".into();
            self.row = 0;
            self.list = None;
            return Vec::new();
        }
        if matches!(button, Button::LsUp | Button::LsDown) {
            self.row = if button == Button::LsUp {
                self.row.saturating_add(1).min(self.history.len())
            } else {
                self.row.saturating_sub(1)
            };
            self.list = None;
            return Vec::new();
        }
        if self.row > 0 {
            self.history_button(button);
            return Vec::new();
        }
        match button {
            Button::Accept => {
                if let Some(list) = self.list.take() {
                    if let Some(entry) = list.entries.get(list.selected) {
                        self.apply(entry.choice.clone());
                    }
                } else if let Some(invocation) = self.draft.ready() {
                    self.draft = Draft::default();
                    self.focus = 0;
                    return vec![Effect::Run(invocation)];
                } else {
                    self.notice = "Not ready: empty slots remain.".into();
                }
            }
            Button::Back => {
                if self.list.take().is_none()
                    && let Some((draft, focus)) = self.undo.pop()
                {
                    self.draft = draft;
                    self.focus = focus;
                }
            }
            Button::Delete => self.delete(),
            Button::ListUp
            | Button::Down
            | Button::Left
            | Button::Right
            | Button::LsLeft
            | Button::LsRight
            | Button::PreviousStage
            | Button::NextStage => self.move_button(button),
            Button::New | Button::LsUp | Button::LsDown => {}
        }
        Vec::new()
    }
}

const fn segment_top(segment: Option<&Segment>) -> Option<usize> {
    match segment {
        Some(Segment::Program(address) | Segment::Item(address, _) | Segment::End(address)) => {
            Some(address.top)
        }
        Some(Segment::Pipe(top)) => Some(*top),
        Some(Segment::Cwd) | None => None,
    }
}

pub(crate) fn valid_value(definition: &Move, value: &str) -> bool {
    !value.contains('\0')
        && match definition.takes {
            Some(ValueType::Integer) => value.parse::<i64>().is_ok(),
            Some(ValueType::Enum) => definition
                .values
                .as_ref()
                .is_some_and(|values| values.contains_key(value)),
            Some(ValueType::Command) => false,
            Some(ValueType::Path | ValueType::Glob | ValueType::Regex | ValueType::String) | None => true,
        }
}

fn parse_stage(words: &[String], specs: &[CommandSpec], env: &Environment, depth: usize) -> Option<Stage> {
    if depth >= 32 {
        return None;
    }
    let (program, args) = words.split_first()?;
    if program.is_empty() || program.contains('\0') {
        return None;
    }
    let mut stage = parsed_program(program, specs);
    let mut args = args.iter();
    while let Some(word) = args.next() {
        let moves = stage.moves(specs);
        let exact = moves.iter().find(|entry| entry.key == *word && !entry.operand);
        let definition = parsed_move(&stage, &moves, exact, word);
        if definition.takes == Some(ValueType::Command) {
            let mut remaining = vec![word.clone()];
            remaining.extend(args.cloned());
            stage.nested = parse_stage(&remaining, specs, env, depth.saturating_add(1)).map(Box::new);
            break;
        }
        let value = if definition.takes.is_none() {
            None
        } else if exact.is_some() {
            args.next().cloned()
        } else {
            Some(word.clone())
        };
        let value = value.map(|value| {
            if definition.takes == Some(ValueType::Path) {
                env.names
                    .iter()
                    .find(|name| name.eq_ignore_ascii_case(&value))
                    .cloned()
                    .unwrap_or(value)
            } else {
                value
            }
        });
        let value = value.filter(|value| valid_value(&definition, value));
        if let Some(item) = stage
            .items
            .iter_mut()
            .find(|item| item.definition.key == definition.key && item.value.is_none())
        {
            item.value = value;
        } else {
            let index = stage.insert(definition, specs);
            if let Some(item) = stage.items.get_mut(index) {
                item.value = value;
            }
        }
    }
    Some(stage)
}

/// Advance the builder without performing I/O.
#[must_use]
pub fn step(mut state: State, input: Input) -> (State, Vec<Effect>) {
    let previous = (state.draft.clone(), state.focus);
    let undoing = matches!(input, Input::Press(Button::Back));
    state.notice.clear();
    let effects = match input {
        Input::Press(button) => state.press(button),
        Input::Say(text) => {
            if !text.contains('\0') && state.row == 0 {
                state.choose(Some(&text));
            }
            Vec::new()
        }
        Input::Type(text) => {
            state.typed(text);
            Vec::new()
        }
        Input::Environment(mut env) => {
            env.programs
                .retain(|name| !name.is_empty() && !name.contains('\0'));
            env.names.retain(|name| !name.contains('\0'));
            state.env = env;
            Vec::new()
        }
        Input::Completed {
            invocation,
            exit,
            output,
        } => {
            state.history.push(HistoryRow {
                text: invocation.shell_text(),
                tree: invocation.draft().clone(),
                exit,
                output,
            });
            Vec::new()
        }
    };
    if state.draft != previous.0 && !undoing && effects.is_empty() {
        state.undo.push(previous);
        if state.undo.len() > 50 {
            let _removed = state.undo.remove(0);
        }
    }
    state.clamp();
    (state, effects)
}

fn move_score(definition: &Move, position: usize, text: Option<&str>) -> usize {
    text.map_or_else(
        || 1000_usize.saturating_sub(position),
        |heard| {
            let said = spelled(heard).to_lowercase();
            similarity(&said, &definition.key)
                .max(similarity(&said, definition.key.trim_start_matches('-')))
                .max(
                    definition
                        .summary
                        .split_whitespace()
                        .map(|word| {
                            similarity(&said, word)
                                .saturating_mul(85)
                                .checked_div(100)
                                .unwrap_or(0)
                        })
                        .max()
                        .unwrap_or(0),
                )
        },
    )
}

fn parsed_program(program: &str, specs: &[CommandSpec]) -> Stage {
    let program = specs
        .iter()
        .find(|spec| spec.program.eq_ignore_ascii_case(program))
        .map_or_else(|| program.to_owned(), |spec| spec.program.clone());
    let mut stage = Stage::default();
    stage.set_program(program, specs);
    stage
}

fn parsed_move(stage: &Stage, moves: &[Move], exact: Option<&Move>, word: &str) -> Move {
    exact
        .cloned()
        .or_else(|| {
            stage
                .items
                .iter()
                .find(|item| item.definition.required && item.value.is_none() && !word.starts_with('-'))
                .map(|item| item.definition.clone())
        })
        .or_else(|| {
            moves
                .iter()
                .find(|entry| entry.operand && entry.key != "argument" && !word.starts_with('-'))
                .cloned()
        })
        .unwrap_or_else(raw_move)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn initial() -> Result<State, serde_json::Error> {
        Ok(State::new("/tmp".into(), crate::specs()?))
    }

    fn event() -> impl Strategy<Value = Input> {
        prop_oneof![
            prop::sample::select(vec![
                Button::Accept,
                Button::Back,
                Button::Delete,
                Button::New,
                Button::PreviousStage,
                Button::NextStage,
                Button::ListUp,
                Button::Down,
                Button::Left,
                Button::Right,
                Button::LsUp,
                Button::LsDown,
                Button::LsLeft,
                Button::LsRight
            ])
            .prop_map(Input::Press),
            prop::sample::select(vec![
                "find",
                "grep",
                "xargs",
                "cat",
                "wc",
                "-name",
                "-type",
                "command",
                "src",
                "star dot r s",
                "TODO",
                "directory",
                "",
                "\0",
                "'",
                "ls -l",
                "find src dash name star dot r s"
            ])
            .prop_map(|text| Input::Say(text.into())),
            ".{0,24}".prop_map(Input::Type),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]
        #[test]
        fn every_sequence_preserves_validity(events in prop::collection::vec(event(), 0..100)) {
            let mut state = initial()?;
            for input in events {
                let before = state.clone();
                let is_back = matches!(input, Input::Press(Button::Back));
                let (next, effects) = step(state, input);
                prop_assert!(next.valid(), "invalid state: {:?}", next);
                let encoded = serde_json::to_string(&next)?;
                let restored: State = serde_json::from_str(&encoded)?;
                prop_assert_eq!(&next, &restored);
                if next.draft != before.draft && effects.is_empty() && !is_back {
                    let mut closed = next.clone();
                    closed.list = None;
                    closed.row = 0;
                    let (undone, _) = step(closed, Input::Press(Button::Back));
                    prop_assert_eq!(&undone.draft, &before.draft, "undo must restore any edit");
                }
                for Effect::Run(invocation) in effects {
                    prop_assert!(invocation.draft().ready().is_some());
                    prop_assert!(before.draft.ready().is_some(), "a draft with a hole cannot run");
                }
                state = next;
            }
        }

        #[test]
        fn boundary_parsers_accept_arbitrary_text(text in ".{0,256}") {
            let _parsed = parse_words(&text);
            let _spoken = spoken_words(&text);
            let (mut state, _) = step(initial()?, Input::Press(Button::New));
            for input in [Input::Say(text.clone()), Input::Press(Button::Accept), Input::Type(text)] {
                (state, _) = step(state, input);
                prop_assert!(state.valid());
            }
        }
    }

    fn build(text: &str) -> Result<State, serde_json::Error> {
        let (state, _) = step(initial()?, Input::Press(Button::New));
        Ok(step(state, Input::Type(text.into())).0)
    }

    #[test]
    fn whole_command_speech_and_direct_operand_fill() -> Result<(), Box<dyn core::error::Error>> {
        let mut state = initial()?;
        (state, _) = step(
            state,
            Input::Environment(Environment {
                programs: vec![],
                names: vec!["src".into(), "src/main.rs".into()],
            }),
        );
        (state, _) = step(state, Input::Press(Button::New));
        (state, _) = step(state, Input::Say("Find src dash name star dot r s.".into()));
        (state, _) = step(state, Input::Press(Button::Accept));
        assert_eq!(
            state.draft.preview(),
            "find src -name '*.rs'",
            "whole speech builds the tree"
        );
        (state, _) = step(state, Input::Press(Button::New));
        (state, _) = step(state, Input::Type("find".into()));
        (state, _) = step(state, Input::Say("Src.".into()));
        assert_eq!(
            state
                .screen()
                .list
                .first()
                .map(|candidate| candidate.value.as_str()),
            Some("src"),
            "existing paths outrank nonexistent literals"
        );
        (state, _) = step(state, Input::Press(Button::Accept));
        assert_eq!(
            state.draft.preview(),
            "find src",
            "speech fills an operand at the end slot"
        );
        assert!(
            state
                .screen()
                .segments
                .iter()
                .all(|segment| !segment.label.contains("run")),
            "run is only a status affordance"
        );
        Ok(())
    }

    #[test]
    fn holes_required_deletion_and_nested_commands() -> Result<(), Box<dyn core::error::Error>> {
        let mut state = build("grep")?;
        assert!(state.draft.ready().is_none(), "grep requires a pattern");
        let effects;
        (state, effects) = step(state, Input::Press(Button::Accept));
        assert!(effects.is_empty(), "a required hole cannot run");
        (state, _) = step(state, Input::Type("TODO".into()));
        (state, _) = step(state, Input::Press(Button::Left));
        (state, _) = step(state, Input::Press(Button::Delete));
        assert!(
            state.draft.ready().is_none(),
            "deleting a required operand clears it"
        );
        let nested = build("xargs -0 cat -n")?;
        assert_eq!(
            nested.draft.preview(),
            "xargs -0 cat -n",
            "nested argv follows outer options"
        );
        assert!(nested.draft.ready().is_some(), "the nested program is complete");
        assert_eq!(
            build("find --unknown")?.draft.preview(),
            "find --unknown",
            "unknown items remain raw"
        );
        Ok(())
    }

    #[test]
    fn completion_history_copy_and_environment_are_inputs() -> Result<(), Box<dyn core::error::Error>> {
        let mut state = build("cat")?;
        let effects;
        (state, effects) = step(state, Input::Press(Button::Accept));
        assert!(state.draft.stages.is_empty(), "running clears the builder");
        for Effect::Run(invocation) in effects {
            (state, _) = step(
                state,
                Input::Completed {
                    invocation,
                    exit: 7,
                    output: "recorded".into(),
                },
            );
        }
        assert_eq!(state.history.len(), 1, "completion records one history row");
        assert_eq!(
            state.history.first().map(|row| row.exit),
            Some(7),
            "completion preserves exit status"
        );
        (state, _) = step(state, Input::Press(Button::LsUp));
        (state, _) = step(state, Input::Press(Button::Accept));
        assert_eq!(state.draft.preview(), "cat", "history acceptance copies the tree");
        assert_eq!(state.row, 0, "copy returns to the builder");
        let env = Environment {
            programs: vec!["custom".into()],
            names: vec!["src".into()],
        };
        (state, _) = step(state, Input::Environment(env.clone()));
        assert_eq!(state.env, env, "environment facts arrive as input");
        Ok(())
    }
}
