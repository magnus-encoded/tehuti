//! The mapping ticker: what the focused context maps, as entries for a scrolling line on the bar.

use core::time::Duration;
use tehuti_protocol::{
    Action, ButtonName, Context, KeyName, LayoutAction, Modifier, TickerSpeech, normalize_phrase,
};

/// How long the ticker holds one scroll position. A few steps a second keep redraws rare.
pub const STEP: Duration = Duration::from_millis(250);

/// How far the ticker moves per step, in logical pixels.
pub const STEP_PIXELS: usize = 6;

/// The blank logical pixels between the end of the ticker text and its next repeat.
pub const REPEAT_GAP: usize = 48;

/// Most phrases a group of synonyms shows before it is cut short.
const PHRASES_SHOWN: usize = 3;

/// The button names printed on a pad, chosen by its USB vendor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PadFamily {
    #[default]
    Letters,
    Symbols,
}

impl PadFamily {
    #[must_use]
    pub const fn for_vendor(vendor: u16) -> Self {
        match vendor {
            0x054c => Self::Symbols,
            _ => Self::Letters,
        }
    }

    #[must_use]
    pub const fn label(self, button: ButtonName) -> &'static str {
        match (self, button) {
            (Self::Letters, ButtonName::South) => "A",
            (Self::Letters, ButtonName::East) => "B",
            (Self::Letters, ButtonName::North) => "Y",
            (Self::Letters, ButtonName::West) => "X",
            (Self::Letters, ButtonName::LeftBumper) => "LB",
            (Self::Letters, ButtonName::RightBumper) => "RB",
            (Self::Letters, ButtonName::Select) => "View",
            (Self::Letters, ButtonName::Start) => "Menu",
            (_, ButtonName::Guide) => "Guide",
            (Self::Letters, ButtonName::Record) | (Self::Symbols, ButtonName::Select) => "Share",
            (Self::Letters, ButtonName::LeftStick) => "LS",
            (Self::Letters, ButtonName::RightStick) => "RS",
            (Self::Symbols, ButtonName::South) => "Cross",
            (Self::Symbols, ButtonName::East) => "Circle",
            (Self::Symbols, ButtonName::North) => "Triangle",
            (Self::Symbols, ButtonName::West) => "Square",
            (Self::Symbols, ButtonName::LeftBumper) => "L1",
            (Self::Symbols, ButtonName::RightBumper) => "R1",
            (Self::Symbols, ButtonName::Start) => "Options",
            (Self::Symbols, ButtonName::Record) => "Record",
            (Self::Symbols, ButtonName::LeftStick) => "L3",
            (Self::Symbols, ButtonName::RightStick) => "R3",
            (_, ButtonName::DpadUp) => "Up",
            (_, ButtonName::DpadDown) => "Down",
            (_, ButtonName::DpadLeft) => "Left",
            (_, ButtonName::DpadRight) => "Right",
        }
    }
}

/// The order buttons appear in; Guide is reserved for Tehuti and never listed.
const BUTTONS: [ButtonName; 15] = [
    ButtonName::South,
    ButtonName::East,
    ButtonName::West,
    ButtonName::North,
    ButtonName::DpadUp,
    ButtonName::DpadDown,
    ButtonName::DpadLeft,
    ButtonName::DpadRight,
    ButtonName::LeftBumper,
    ButtonName::RightBumper,
    ButtonName::LeftStick,
    ButtonName::RightStick,
    ButtonName::Select,
    ButtonName::Start,
    ButtonName::Record,
];

/// Recognisable actions and the icon each gets when its mapping names none.
enum Recognised {
    Close,
    Layout(LayoutAction),
    LaunchContaining(&'static str),
    Chord(KeyName, Modifier),
    ToggleTicker,
}

const BUILT_IN_ICONS: [(Recognised, &str); 9] = [
    (Recognised::Close, "\u{274c}"),
    (Recognised::Layout(LayoutAction::ToggleFullscreen), "\u{26f6}"),
    (Recognised::LaunchContaining("volume-up"), "\u{1f50a}"),
    (Recognised::LaunchContaining("volume-down"), "\u{1f509}"),
    (Recognised::LaunchContaining("mute"), "\u{1f507}"),
    (Recognised::Chord(KeyName::S, Modifier::Ctrl), "\u{1f4be}"),
    (Recognised::Chord(KeyName::O, Modifier::Ctrl), "\u{1f4c2}"),
    (Recognised::Chord(KeyName::P, Modifier::Ctrl), "\u{23ef}"),
    (Recognised::ToggleTicker, "\u{1f4dc}"),
];

/// Short words for icons, shown when the renderer cannot draw the icon.
const ICON_LABELS: [(&str, &str); 13] = [
    ("\u{1f4be}", "Save"),
    ("\u{1f4c2}", "Open"),
    ("\u{1f509}", "Vol-"),
    ("\u{1f50a}", "Vol+"),
    ("\u{1f507}", "Mute"),
    ("\u{274c}", "Close"),
    ("\u{26f6}", "Fullscreen"),
    ("\u{23ef}", "Pause"),
    ("\u{23f8}", "Pause"),
    ("\u{1f504}", "Restart"),
    ("\u{1f3a4}", "Talk"),
    ("\u{1f4dc}", "Ticker"),
    ("\u{1f4e5}", "Load"),
];

/// The built-in icon for an action, if it is a recognisable one.
#[must_use]
pub fn built_in_icon(action: &Action) -> Option<&'static str> {
    BUILT_IN_ICONS
        .iter()
        .find(|(recognised, _)| match (recognised, action) {
            (Recognised::Close, Action::Close) | (Recognised::ToggleTicker, Action::ToggleTicker) => true,
            (Recognised::Layout(wanted), Action::Layout { action }) => wanted == action,
            (Recognised::LaunchContaining(part), Action::Launch { desktop_entry }) => {
                desktop_entry.as_str().contains(part)
            }
            (Recognised::Chord(key, modifier), Action::Key { key: pressed, mods }) => {
                pressed == key && mods.as_slice() == [*modifier]
            }
            _ => false,
        })
        .map(|(_, icon)| *icon)
}

/// The word for an icon, if the ticker knows it.
#[must_use]
pub fn icon_label(icon: &str) -> Option<&'static str> {
    ICON_LABELS
        .iter()
        .find(|(known, _)| *known == icon)
        .map(|(_, label)| *label)
}

/// Short text for what an action does, such as `Ctrl+O` or `editor`.
#[must_use]
pub fn action_text(action: &Action) -> String {
    match action {
        Action::Key { key, mods } => {
            let mut text = String::new();
            for modifier in mods {
                text.push_str(match modifier {
                    Modifier::Ctrl => "Ctrl+",
                    Modifier::Alt => "Alt+",
                    Modifier::Shift => "Shift+",
                    Modifier::Super => "Super+",
                });
            }
            text.push_str(&key_text(*key));
            text
        }
        Action::KeyDown { key } | Action::KeyUp { key } => key_text(*key),
        Action::Text { text } => format!("\"{}\"", text.as_str().trim()),
        Action::Launch { desktop_entry } => {
            let entry = desktop_entry.as_str();
            let entry = entry.strip_suffix(".desktop").unwrap_or(entry);
            entry.rsplit('.').next().unwrap_or(entry).into()
        }
        Action::Close => "Close".into(),
        Action::Notice { text } => text.as_str().into(),
        Action::SecretRequest { .. } => "Secret".into(),
        Action::Quit => "Quit".into(),
        Action::Layout { action } => layout_text(*action).into(),
        Action::ToggleTicker => "Ticker".into(),
    }
}

fn key_text(key: KeyName) -> String {
    let keysym = key.keysym_name();
    if keysym.chars().count() == 1 {
        keysym.to_uppercase()
    } else if key == KeyName::PageUp {
        "PgUp".into()
    } else if key == KeyName::PageDown {
        "PgDn".into()
    } else {
        format!("{key:?}")
    }
}

const fn layout_text(action: LayoutAction) -> &'static str {
    match action {
        LayoutAction::FocusColumnLeft => "Column left",
        LayoutAction::FocusColumnRight => "Column right",
        LayoutAction::FocusWindowUp => "Window up",
        LayoutAction::FocusWindowDown => "Window down",
        LayoutAction::MoveColumnLeft => "Move left",
        LayoutAction::MoveColumnRight => "Move right",
        LayoutAction::SwitchPresetColumnWidth => "Wider",
        LayoutAction::SwitchPresetColumnWidthBack => "Narrower",
        LayoutAction::FocusWorkspaceDown => "Workspace down",
        LayoutAction::FocusWorkspaceUp => "Workspace up",
        LayoutAction::MoveColumnToWorkspaceDown => "Move down",
        LayoutAction::MoveColumnToWorkspaceUp => "Move up",
        LayoutAction::ToggleFullscreen => "Fullscreen",
        LayoutAction::MaximizeColumn => "Maximize",
    }
}

/// One ticker item: a trigger, and what it does as an icon with a fallback word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub trigger: String,
    pub icon: Option<String>,
    pub label: String,
}

impl Entry {
    /// `trigger -> result`, with the icon when `drawable` accepts it and the label otherwise.
    #[must_use]
    pub fn text(&self, drawable: impl Fn(&str) -> bool) -> String {
        let result = self
            .icon
            .as_deref()
            .filter(|icon| drawable(icon))
            .unwrap_or(&self.label);
        format!("{} -> {result}", self.trigger)
    }

    fn new(trigger: String, action: &Action, icon: Option<&str>) -> Self {
        let icon = icon.or_else(|| built_in_icon(action));
        let label = icon
            .and_then(icon_label)
            .map_or_else(|| action_text(action), Into::into);
        Self {
            trigger,
            icon: icon.map(Into::into),
            label,
        }
    }
}

/// Whether the ticker shows for `focused`: its context's choice, else the global one; off by default.
#[must_use]
pub fn enabled(contexts: &[Context], focused: Option<&str>) -> bool {
    let (active, global) = active_and_global(contexts, focused);
    active
        .and_then(|context| context.ticker().enabled)
        .or_else(|| global.and_then(|context| context.ticker().enabled))
        .unwrap_or(false)
}

/// What the ticker lists while `focused` has focus: buttons in a fixed order, then phrases.
///
/// Buttons resolve like routing: the active context, then the global context, then
/// Enter on South. Push-to-talk buttons show as talk. In passthrough the pad belongs to
/// the application, so one entry says so. Phrases with the same action and icon
/// form one entry; phrases with an icon come first.
#[must_use]
pub fn entries(
    contexts: &[Context],
    focused: Option<&str>,
    pad: PadFamily,
    level: crate::Level,
) -> Vec<Entry> {
    let (active, global) = active_and_global(contexts, focused);
    let mut entries = button_entries(active, global, pad, level);
    let speech = active
        .and_then(|context| context.ticker().speech)
        .or_else(|| global.and_then(|context| context.ticker().speech))
        .unwrap_or(TickerSpeech::All);
    entries.extend(speech_entries(active, global, speech));
    entries
}

fn active_and_global<'contexts>(
    contexts: &'contexts [Context],
    focused: Option<&str>,
) -> (Option<&'contexts Context>, Option<&'contexts Context>) {
    let global = contexts
        .iter()
        .find(|context| context.app_id() == crate::GLOBAL_APP_ID);
    let active = contexts
        .iter()
        .find(|context| Some(context.app_id()) == focused && context.app_id() != crate::GLOBAL_APP_ID);
    (active, global)
}

fn button_entries(
    active: Option<&Context>,
    global: Option<&Context>,
    pad: PadFamily,
    level: crate::Level,
) -> Vec<Entry> {
    let talk = global.map_or(&[ButtonName::Record][..], Context::push_to_talk_buttons);
    if active.is_some_and(Context::passthrough) {
        let mut entries = vec![Entry {
            trigger: "Pad".into(),
            icon: None,
            label: "app".into(),
        }];
        let chord = active.map_or(&[][..], Context::passthrough_push_to_talk);
        entries.push(talk_entry(chord_text(chord, pad)));
        return entries;
    }
    BUTTONS
        .iter()
        .filter_map(|button| {
            let trigger = pad.label(*button).to_owned();
            if talk.contains(button) {
                return Some(talk_entry(trigger));
            }
            if level == crate::Level::Columns {
                if *button == ButtonName::LeftBumper {
                    return Some(Entry::new(
                        trigger,
                        &Action::Layout {
                            action: LayoutAction::FocusColumnLeft,
                        },
                        None,
                    ));
                }
                if *button == ButtonName::RightBumper {
                    return Some(Entry::new(
                        trigger,
                        &Action::Layout {
                            action: LayoutAction::FocusColumnRight,
                        },
                        None,
                    ));
                }
            }
            let context = active
                .filter(|context| context.action_for(*button).is_some())
                .or(global);
            let action = context
                .and_then(|context| context.action_for(*button))
                .or_else(|| {
                    (*button == ButtonName::South).then_some(Action::Key {
                        key: KeyName::Enter,
                        mods: Vec::new(),
                    })
                })?;
            let mut entry = Entry::new(trigger, &action, None);
            if context
                .and_then(|context| context.held_key_for(*button))
                .is_some()
            {
                entry.label.push_str(" (hold)");
            }
            Some(entry)
        })
        .collect()
}

fn talk_entry(trigger: String) -> Entry {
    Entry {
        trigger,
        icon: Some("\u{1f3a4}".into()),
        label: "Talk".into(),
    }
}

fn chord_text(chord: &[ButtonName], pad: PadFamily) -> String {
    let names: Vec<&str> = chord.iter().map(|button| pad.label(*button)).collect();
    format!("Hold {}", names.join("+"))
}

fn speech_entries(active: Option<&Context>, global: Option<&Context>, shown: TickerSpeech) -> Vec<Entry> {
    if shown == TickerSpeech::None {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    let mut groups: Vec<(Action, Option<String>, Vec<String>)> = Vec::new();
    for context in active.into_iter().chain(global) {
        for mapping in context.speech_mappings() {
            if !seen.insert(normalize_phrase(mapping.phrase)) {
                continue;
            }
            let icon = mapping.icon.or_else(|| built_in_icon(&mapping.action));
            if shown == TickerSpeech::Icons && icon.is_none() {
                continue;
            }
            let phrase = display_phrase(mapping.phrase);
            let icon = icon.map(str::to_owned);
            match groups
                .iter_mut()
                .find(|(action, group_icon, _)| *action == mapping.action && *group_icon == icon)
            {
                Some((_, _, phrases)) => phrases.push(phrase),
                None => groups.push((mapping.action, icon, vec![phrase])),
            }
        }
    }
    for (_, _, phrases) in &mut groups {
        phrases.sort_by(|left, right| {
            left.chars()
                .count()
                .cmp(&right.chars().count())
                .then_with(|| left.cmp(right))
        });
    }
    groups.sort_by(|(_, left_icon, left), (_, right_icon, right)| {
        right_icon
            .is_some()
            .cmp(&left_icon.is_some())
            .then_with(|| left.cmp(right))
    });
    groups
        .into_iter()
        .map(|(action, icon, phrases)| {
            let mut trigger = phrases
                .iter()
                .take(PHRASES_SHOWN)
                .map(|phrase| format!("\"{phrase}\""))
                .collect::<Vec<_>>()
                .join(" / ");
            if phrases.len() > PHRASES_SHOWN {
                trigger.push_str(" /\u{2026}");
            }
            Entry::new(trigger, &action, icon.as_deref())
        })
        .collect()
}

/// The phrase without the punctuation the speech engine ends a sentence with.
fn display_phrase(phrase: &str) -> String {
    phrase
        .trim()
        .trim_end_matches(|character: char| !character.is_alphanumeric())
        .into()
}

/// The ticker's left shift in pixels after `step` steps, for text `content` wide in a `viewport`.
///
/// Text that fits stays still. Longer text moves left by `step_pixels` a step and
/// wraps after itself plus `gap`, so a second copy can follow at that period.
#[must_use]
pub fn scroll_offset(step: u64, content: usize, viewport: usize, step_pixels: usize, gap: usize) -> usize {
    if content <= viewport {
        return 0;
    }
    let period = u64::try_from(content.saturating_add(gap)).unwrap_or(u64::MAX);
    let moved = step
        .checked_rem(period)
        .unwrap_or_default()
        .saturating_mul(u64::try_from(step_pixels).unwrap_or_default());
    usize::try_from(moved.checked_rem(period).unwrap_or_default()).unwrap_or_default()
}
