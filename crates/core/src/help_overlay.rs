//! Global "what can I say / press" reference overlay, built from mapping ticker data.

use crate::adjuster::SizeAxis;
use crate::pad::{PadFamily, Role};
use crate::ticker::{self, MappedEntry, Origin, Piece, TriggerKind};
use crate::tool_words::{TOOL_WORDS, ToolWord};
use tehuti_protocol::{Action, ButtonName, Context, TickerSpeech, normalize_phrase};

/// Maximum rows visible at once inside one group before scrolling.
pub const VISIBLE_ROWS: usize = 8;

/// Built-in spoken phrase for starting a recording.
pub const START_RECORDING_PHRASE: &str = "start recording";
/// Built-in spoken phrase for stopping a recording.
pub const STOP_RECORDING_PHRASE: &str = "stop recording";
/// Built-in spoken phrase for closing the focused window.
pub const CLOSE_PHRASE: &str = "close";
/// Built-in spoken phrase for undoing the last dictated phrase.
pub const SCRATCH_THAT_PHRASE: &str = "scratch that";
/// Built-in spoken phrase for opening the volume adjuster.
pub const VOLUME_PHRASE: &str = "volume";
/// Built-in spoken phrase for the creation launcher.
pub const NEW_PHRASE: &str = "new";

/// What confirming a help overlay entry does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Applies a protocol action directly to the focused window or compositor.
    Action(Action),
    /// Routes a built-in spoken command through dispatch.
    Speech(String),
    /// Reference-only entry that does not run on confirm.
    None,
}

/// Which section of the help overlay a group represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    FocusedVoice,
    FocusedPad,
    GlobalVoice,
    GlobalPad,
}

/// One row in the help overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Mapping ticker pieces for what triggers the command.
    pub trigger: Vec<Piece>,
    /// Mapping ticker pieces for what the command does.
    pub result: Vec<Piece>,
    /// The spoken phrase voice uses for this unit, when one exists.
    pub voice_name: Option<String>,
    /// What Confirm runs when this row is selected.
    pub target: Target,
}

/// One named section of commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub kind: GroupKind,
    pub title: String,
    pub items: Vec<Item>,
}

/// Controller-navigable snapshot of global and focused-context commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpOverlay {
    context_name: Option<String>,
    level: crate::Level,
    groups: Vec<Group>,
    group: usize,
    entry: usize,
    scroll: usize,
}

impl HelpOverlay {
    /// Builds a grouped overlay from the same mapping ticker data used by the bar,
    /// plus built-in system voice units and Guide controls.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "pure builder taking every routing input without reaching into I/O"
    )]
    pub fn build(
        active: Option<&Context>,
        active_name: Option<String>,
        global: Option<&Context>,
        pad: PadFamily,
        level: crate::Level,
        modifier: Option<ButtonName>,
        talk: &[ButtonName],
        extra_adjusters: &[String],
    ) -> Self {
        let mapped = ticker::mapped_entries_for(
            active,
            global,
            pad,
            level,
            modifier,
            Some(talk),
            TickerSpeech::All,
            true,
        );
        let context_label = active.map(|context| {
            active_name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| context.app_id().to_owned())
        });
        let groups = assemble_groups(mapped, context_label.as_deref(), pad, level, extra_adjusters);
        Self {
            context_name: context_label,
            level,
            groups,
            group: 0,
            entry: 0,
            scroll: 0,
        }
    }

    #[must_use]
    pub fn context_name(&self) -> Option<&str> {
        self.context_name.as_deref()
    }

    #[must_use]
    pub const fn level(&self) -> crate::Level {
        self.level
    }

    #[must_use]
    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    #[must_use]
    pub const fn active_group_index(&self) -> usize {
        self.group
    }

    #[must_use]
    pub fn active_group(&self) -> Option<&Group> {
        self.groups.get(self.group)
    }

    #[must_use]
    pub const fn selected_index(&self) -> usize {
        self.entry
    }

    #[must_use]
    pub fn selected_item(&self) -> Option<&Item> {
        self.active_group()?.items.get(self.entry)
    }

    /// Range of item indices currently visible inside the active group.
    #[must_use]
    pub fn visible_range(&self) -> core::ops::Range<usize> {
        let len = self.active_group().map_or(0, |group| group.items.len());
        let start = self.scroll.min(len);
        let end = start.saturating_add(VISIBLE_ROWS).min(len);
        start..end
    }

    /// Moves focus to the previous or next group, resetting selection to its first entry.
    pub fn step_group(&mut self, forward: bool) {
        if self.groups.is_empty() {
            return;
        }
        let next = if forward {
            self.group
                .saturating_add(1)
                .min(self.groups.len().saturating_sub(1))
        } else {
            self.group.saturating_sub(1)
        };
        if next != self.group {
            self.group = next;
            self.entry = 0;
            self.scroll = 0;
        }
    }

    /// Steps one entry up or down, crossing smoothly into the neighbouring group at each end.
    pub fn step_entry(&mut self, down: bool) {
        let Some(group_len) = self.active_group().map(|group| group.items.len()) else {
            return;
        };
        if down {
            if self.entry.saturating_add(1) < group_len {
                self.entry = self.entry.saturating_add(1);
                self.keep_selected_visible();
            } else if self.group.saturating_add(1) < self.groups.len() {
                self.group = self.group.saturating_add(1);
                self.entry = 0;
                self.scroll = 0;
            }
        } else if self.entry > 0 {
            self.entry = self.entry.saturating_sub(1);
            self.keep_selected_visible();
        } else if self.group > 0 {
            self.group = self.group.saturating_sub(1);
            let prev_len = self.active_group().map_or(0, |group| group.items.len());
            self.entry = prev_len.saturating_sub(1);
            self.scroll = prev_len.saturating_sub(VISIBLE_ROWS);
        }
    }

    /// What confirming the currently selected entry runs.
    #[must_use]
    pub fn confirm_target(&self) -> Target {
        self.selected_item()
            .map_or(Target::None, |item| item.target.clone())
    }

    /// The pad glyph and label for Confirm when the selected entry can be run.
    #[must_use]
    pub fn confirm_hint(&self, family: PadFamily) -> Option<(Piece, &'static str)> {
        self.selected_item()
            .filter(|item| item.target != Target::None)
            .map(|_| (family.glyph(Role::Confirm), "Run"))
    }

    /// The pad glyph and label for closing the overlay.
    #[must_use]
    pub fn back_hint(&self, family: PadFamily) -> (Piece, &'static str) {
        (family.glyph(Role::Back), "Close")
    }

    const fn keep_selected_visible(&mut self) {
        if self.entry < self.scroll {
            self.scroll = self.entry;
        }
        let end = self.scroll.saturating_add(VISIBLE_ROWS);
        if self.entry >= end {
            self.scroll = self.entry.saturating_sub(VISIBLE_ROWS.saturating_sub(1));
        }
    }
}

fn assemble_groups(
    mapped: Vec<MappedEntry>,
    context_label: Option<&str>,
    pad: PadFamily,
    level: crate::Level,
    extra_adjusters: &[String],
) -> Vec<Group> {
    let (mut focused_voice_m, mut focused_pad_m) = (Vec::new(), Vec::new());
    let (mut global_voice_m, mut global_pad_m) = (Vec::new(), Vec::new());
    for entry in mapped {
        match (entry.origin, entry.kind) {
            (Origin::Focused, TriggerKind::Voice) => focused_voice_m.push(entry),
            (Origin::Focused, TriggerKind::Pad) => focused_pad_m.push(entry),
            (Origin::Global, TriggerKind::Voice) => global_voice_m.push(entry),
            (Origin::Global, TriggerKind::Pad) => global_pad_m.push(entry),
        }
    }
    let mapped_phrases = collect_normalized_phrases(&focused_voice_m, &global_voice_m);
    let action_voices = build_action_voice_map(&focused_voice_m, &global_voice_m, level);
    let focused_voice: Vec<Item> = focused_voice_m.into_iter().map(voice_item).collect();
    let focused_pad: Vec<Item> = focused_pad_m
        .into_iter()
        .map(|entry| pad_item(entry, &action_voices))
        .collect();
    let mut global_voice: Vec<Item> = global_voice_m.into_iter().map(voice_item).collect();
    global_voice.extend(built_in_voice_items(level, extra_adjusters, &mapped_phrases));
    let mut global_pad: Vec<Item> = global_pad_m
        .into_iter()
        .map(|entry| pad_item(entry, &action_voices))
        .collect();
    global_pad.extend(built_in_guide_items(pad, level));

    let mut groups = Vec::new();
    if let Some(name) = context_label {
        push_group(
            &mut groups,
            GroupKind::FocusedVoice,
            format!("{name}: Voice"),
            focused_voice,
        );
        push_group(
            &mut groups,
            GroupKind::FocusedPad,
            format!("{name}: Pad"),
            focused_pad,
        );
    }
    push_group(
        &mut groups,
        GroupKind::GlobalVoice,
        "Global: Voice".into(),
        global_voice,
    );
    push_group(
        &mut groups,
        GroupKind::GlobalPad,
        "Global: Pad".into(),
        global_pad,
    );
    groups
}

fn push_group(groups: &mut Vec<Group>, kind: GroupKind, title: String, items: Vec<Item>) {
    if !items.is_empty() {
        groups.push(Group { kind, title, items });
    }
}

fn collect_normalized_phrases(
    focused: &[MappedEntry],
    global: &[MappedEntry],
) -> std::collections::HashSet<String> {
    focused
        .iter()
        .chain(global.iter())
        .flat_map(|mapped| mapped.phrases.iter())
        .map(|phrase| normalize_phrase(phrase))
        .collect()
}

fn build_action_voice_map(
    focused: &[MappedEntry],
    global: &[MappedEntry],
    level: crate::Level,
) -> Vec<(Action, String)> {
    let mut map: Vec<(Action, String)> = Vec::new();
    for mapped in focused.iter().chain(global.iter()) {
        if let Some(action) = &mapped.action
            && let Some(phrase) = mapped.phrases.first()
            && !phrase.contains('<')
            && !map.iter().any(|(known, _)| known == action)
        {
            map.push((action.clone(), phrase.clone()));
        }
    }
    if !map.iter().any(|(known, _)| *known == Action::Close) {
        map.push((Action::Close, CLOSE_PHRASE.into()));
    }
    if level == crate::Level::App && !map.iter().any(|(known, _)| *known == Action::UndoDictation) {
        map.push((Action::UndoDictation, SCRATCH_THAT_PHRASE.into()));
    }
    map
}

fn voice_item(mapped: MappedEntry) -> Item {
    let primary = mapped.phrases.first().cloned();
    let is_template = mapped.phrases.iter().any(|phrase| phrase.contains('<'));
    let target = if is_template {
        Target::None
    } else {
        mapped.action.map_or(Target::None, Target::Action)
    };
    Item {
        trigger: mapped.entry.trigger,
        result: mapped.entry.result,
        voice_name: primary,
        target,
    }
}

fn pad_item(mapped: MappedEntry, action_voices: &[(Action, String)]) -> Item {
    let voice_name = mapped.action.as_ref().and_then(|action| {
        action_voices
            .iter()
            .find(|(known, _)| known == action)
            .map(|(_, phrase)| phrase.clone())
    });
    Item {
        trigger: mapped.entry.trigger,
        result: mapped.entry.result,
        voice_name,
        target: mapped.action.map_or(Target::None, Target::Action),
    }
}

fn spoken_unit(phrase: &str, result: Vec<Piece>, target: Target) -> Item {
    Item {
        trigger: vec![Piece::Text(format!("\"{phrase}\""))],
        result,
        voice_name: Some(phrase.into()),
        target,
    }
}

fn built_in_voice_items(
    level: crate::Level,
    extra_adjusters: &[String],
    mapped_phrases: &std::collections::HashSet<String>,
) -> Vec<Item> {
    let mut items = Vec::new();
    for (phrase, tool) in TOOL_WORDS {
        let (label, target) = match tool {
            ToolWord::Keyboard => ("Keyboard", Target::Speech(phrase.into())),
            ToolWord::MouseMode => ("Mouse mode", Target::Speech(phrase.into())),
            ToolWord::WhatCanISay => ("What can I say", Target::None),
        };
        items.push(spoken_unit(phrase, vec![Piece::Text(label.into())], target));
    }
    items.push(spoken_unit(
        crate::dispatch::OPEN_SETTINGS_PHRASE,
        vec![Piece::Text("Settings".into())],
        Target::Speech(crate::dispatch::OPEN_SETTINGS_PHRASE.into()),
    ));
    items.push(spoken_unit(
        VOLUME_PHRASE,
        vec![Piece::Text("Volume".into())],
        Target::Speech(VOLUME_PHRASE.into()),
    ));
    for (axis, label) in [
        (SizeAxis::Width, "Window width"),
        (SizeAxis::Height, "Window height"),
    ] {
        let phrase = axis.phrase();
        items.push(spoken_unit(
            phrase,
            vec![Piece::Text(label.into())],
            Target::Speech(phrase.into()),
        ));
    }
    for name in extra_adjusters {
        if !name.trim().is_empty() {
            items.push(spoken_unit(
                name,
                vec![Piece::Text(name.clone())],
                Target::Speech(name.clone()),
            ));
        }
    }
    items.extend(built_in_session_voice_items(level, mapped_phrases));
    items
}

fn built_in_session_voice_items(
    level: crate::Level,
    mapped_phrases: &std::collections::HashSet<String>,
) -> Vec<Item> {
    let mut items = vec![
        spoken_unit(
            START_RECORDING_PHRASE,
            vec![Piece::Text("Start recording".into())],
            Target::Speech(START_RECORDING_PHRASE.into()),
        ),
        spoken_unit(
            STOP_RECORDING_PHRASE,
            vec![Piece::Text("Stop recording".into())],
            Target::Speech(STOP_RECORDING_PHRASE.into()),
        ),
        spoken_unit(
            "open <name>",
            vec![Piece::Text("Open app or window".into())],
            Target::None,
        ),
        spoken_unit(
            "work on <name>",
            vec![Piece::Text("Open project".into())],
            Target::None,
        ),
        spoken_unit(
            NEW_PHRASE,
            vec![Piece::Text("New".into())],
            Target::Speech(NEW_PHRASE.into()),
        ),
    ];
    if !mapped_phrases.contains(CLOSE_PHRASE) {
        let result = ticker::built_in_icon(&Action::Close).map_or_else(
            || ticker::action_pieces(&Action::Close),
            |icon| {
                vec![Piece::Icon {
                    icon: icon.into(),
                    fallback: ticker::icon_label(icon).map_or_else(
                        || ticker::action_pieces(&Action::Close),
                        |label| vec![Piece::Text(label.into())],
                    ),
                }]
            },
        );
        items.push(spoken_unit(CLOSE_PHRASE, result, Target::Action(Action::Close)));
    }
    if level == crate::Level::App && !mapped_phrases.contains(SCRATCH_THAT_PHRASE) {
        items.push(spoken_unit(
            SCRATCH_THAT_PHRASE,
            ticker::action_pieces(&Action::UndoDictation),
            Target::Speech(SCRATCH_THAT_PHRASE.into()),
        ));
    }
    items
}

fn built_in_guide_items(pad: PadFamily, level: crate::Level) -> Vec<Item> {
    let guide = pad.piece(ButtonName::Guide);
    let dpad_result = match level {
        crate::Level::App => "Focus column / workspace",
        crate::Level::Columns => "Move column / workspace",
    };
    vec![
        Item {
            trigger: vec![guide.clone(), Piece::Dim("tap".into())],
            result: vec![Piece::Text("Switch level".into())],
            voice_name: None,
            target: Target::None,
        },
        Item {
            trigger: vec![guide.clone(), Piece::Dim("hold".into())],
            result: vec![Piece::Text("System menu".into())],
            voice_name: None,
            target: Target::None,
        },
        Item {
            trigger: vec![
                guide.clone(),
                Piece::Text("+".into()),
                Piece::Chip("D-pad".into()),
            ],
            result: vec![Piece::Text(dpad_result.into())],
            voice_name: None,
            target: Target::None,
        },
        Item {
            trigger: vec![
                guide,
                Piece::Text("+".into()),
                pad.piece(ButtonName::Start),
                Piece::Dim("3 s".into()),
            ],
            result: vec![Piece::Text("Quit".into())],
            voice_name: None,
            target: Target::None,
        },
    ]
}
