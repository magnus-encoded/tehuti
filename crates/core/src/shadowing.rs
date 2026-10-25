//! Context mappings that routing can never reach, for configuration checks.

use tehuti_protocol::{Action, ButtonName, Context, KeyName};

use crate::GLOBAL_APP_ID;

/// The routing layer that consumes a button before the active context sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonOwner {
    /// The escape chord consumes every Guide press and release.
    EscapeChord,
    /// The global layer sends Enter for a South press that no focused context maps, so the
    /// global context's own South mapping never fires. A focused context's South replaces it.
    GlobalEnter,
    /// The global context's `push_to_talk` list records speech with the button.
    PushToTalk,
}

/// A part of the configuration that never takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shadowing {
    /// A context whose app id an earlier context already claims; it never becomes active.
    Context { context: usize, earlier: usize },
    /// A `[buttons]` entry that an earlier routing layer always consumes.
    Button {
        context: usize,
        button: ButtonName,
        owner: ButtonOwner,
    },
}

const GLOBAL_ENTER: Action = Action::Key {
    key: KeyName::Enter,
    mods: Vec::new(),
};

/// Lists the shadowed contexts and button mappings of `contexts`, given in load order.
///
/// Indices refer to `contexts`. Button mappings of a shadowed context are not listed, nor is
/// a global South mapping to a plain Enter, which matches what the global layer sends.
/// Modal layers that take buttons only while open (on-screen keyboard, mouse mode, secret
/// confirmation, settings pages) are not static shadowing and are not reported.
#[must_use]
pub fn shadowed(contexts: &[Context]) -> Vec<Shadowing> {
    let push_to_talk = |button: ButtonName| {
        contexts
            .iter()
            .find(|context| context.app_id() == GLOBAL_APP_ID)
            .map_or(button == ButtonName::Record, |global| global.push_to_talk(button))
    };
    let mut found = Vec::new();
    for (index, context) in contexts.iter().enumerate() {
        if let Some(earlier) = contexts
            .iter()
            .take(index)
            .position(|other| other.app_id() == context.app_id())
        {
            found.push(Shadowing::Context {
                context: index,
                earlier,
            });
            continue;
        }
        let mut buttons: Vec<(ButtonName, ButtonOwner)> = context
            .mapped_buttons()
            .filter_map(|button| {
                let owner = if button == ButtonName::Guide {
                    ButtonOwner::EscapeChord
                } else if push_to_talk(button) {
                    ButtonOwner::PushToTalk
                } else if button == ButtonName::South && context.app_id() == GLOBAL_APP_ID {
                    if context.action_for(button) == Some(GLOBAL_ENTER) {
                        return None;
                    }
                    ButtonOwner::GlobalEnter
                } else {
                    return None;
                };
                Some((button, owner))
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
