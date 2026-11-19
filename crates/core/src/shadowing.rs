//! Context mappings that routing can never reach, for configuration checks.

use tehuti_protocol::{Action, ButtonName, Context, KeyName};

use crate::{
    GLOBAL_APP_ID, Level,
    ownership::{Owner, OwnershipView, Phase, TalkSource, Why, owner},
};

/// A part of the configuration that never takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shadowing {
    /// A context whose app id an earlier context already claims; it never becomes active.
    Context { context: usize, earlier: usize },
    /// A `[buttons]` entry that an earlier routing layer always consumes.
    Button {
        context: usize,
        button: ButtonName,
        owner: Why,
    },
}

const GLOBAL_ENTER: Action = Action::Key {
    key: KeyName::Enter,
    mods: Vec::new(),
};

/// Lists the shadowed contexts and button mappings of `contexts`, given in load order.
///
/// Indices refer to `contexts`. Button mappings of a shadowed context are not listed, nor is
/// a global Confirm mapping to a plain Enter, which matches what the global layer sends.
/// Modal layers that take buttons only while open (on-screen keyboard, mouse mode, secret
/// confirmation, settings pages) are not static shadowing and are not reported.
#[must_use]
pub fn shadowed(contexts: &[Context]) -> Vec<Shadowing> {
    shadowed_for_pad(contexts, crate::pad::PadFamily::default())
}

/// Lists unreachable mappings for the connected pad's roles.
#[must_use]
pub fn shadowed_for_pad(contexts: &[Context], pad: crate::pad::PadFamily) -> Vec<Shadowing> {
    let global = contexts.iter().find(|context| context.app_id() == GLOBAL_APP_ID);
    let global_talk = || {
        global.map_or_else(
            || (vec![ButtonName::Record], TalkSource::Default),
            |global| (global.push_to_talk_buttons().to_vec(), TalkSource::Global),
        )
    };
    let mut found = Vec::new();
    for (index, context) in contexts.iter().enumerate() {
        if let Some(earlier) = contexts
            .iter()
            .take(index)
            .position(|other| other.app_id() == context.app_id() && other.title() == context.title())
        {
            found.push(Shadowing::Context {
                context: index,
                earlier,
            });
            continue;
        }
        let is_global = context.app_id() == GLOBAL_APP_ID;
        // The global context's own buttons are judged against the global list; any other
        // context's own list replaces it, as routing does.
        let (talk, talk_source) = match context.own_push_to_talk() {
            Some(own) if !is_global => (own.to_vec(), TalkSource::Context),
            _ => global_talk(),
        };
        let view = OwnershipView {
            confirm: pad.button(crate::pad::Role::Confirm),
            passthrough: false,
            level: Level::App,
            guide_mode: tehuti_protocol::GuideMode::Tehuti,
            guide_held: false,
            start_in_chord: false,
            chord_buttons: Vec::new(),
            menu_open: false,
            held_buttons: Vec::new(),
            menu_buttons: Vec::new(),
            talk,
            talk_source,
            plugin_active: false,
            // The global layer owns Confirm unless the focused context maps it.
            focused_maps: if is_global {
                Vec::new()
            } else {
                context.mapped_buttons().collect()
            },
            global_maps: Vec::new(),
            holds: Vec::new(),
            stick_modifiers: Vec::new(),
            sticks_held: Vec::new(),
        };
        let mut buttons: Vec<(ButtonName, Why)> = context
            .mapped_buttons()
            .filter_map(|button| {
                let why = match owner(&view, button, Phase::Pressed) {
                    Owner::Tehuti(Why::EscapeChord) => Why::EscapeChord,
                    Owner::Talk(_) => Why::PushToTalk,
                    Owner::GlobalLayer => {
                        if context.action_for(button) == Some(GLOBAL_ENTER) {
                            return None;
                        }
                        Why::GlobalEnter
                    }
                    _ => return None,
                };
                Some((button, why))
            })
            .collect();
        buttons.sort_by_cached_key(|(button, _)| format!("{button:?}"));
        found.extend(buttons.into_iter().map(|(button, owner)| Shadowing::Button {
            context: index,
            button,
            owner,
        }));
    }
    found
}
