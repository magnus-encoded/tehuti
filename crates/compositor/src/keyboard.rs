//! Physical keyboard gestures translated into controller events.
use std::collections::BTreeSet;
use tehuti_protocol::{ButtonName, ButtonState, Event};

#[derive(Debug, Default)]
pub struct Keyboard {
    held: BTreeSet<u32>,
    pending_meta: Option<u32>,
    ptt: Option<u32>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Output {
    Key(u32, bool),
    Button(Event),
}

const fn button(name: ButtonName, pressed: bool) -> Output {
    Output::Button(Event::Button {
        name,
        state: if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        },
    })
}

impl Keyboard {
    pub fn reset(&mut self) -> Vec<Output> {
        if let Some(meta) = self.pending_meta.take() {
            let _removed = self.held.remove(&meta);
        }
        let held = core::mem::take(&mut self.held);
        held.into_iter().flat_map(|key| self.key(key, false, 0)).collect()
    }

    #[must_use]
    pub fn key(&mut self, key: u32, pressed: bool, ptt: u32) -> Vec<Output> {
        if pressed && !self.held.insert(key) {
            return Vec::new();
        }
        if !pressed {
            let _removed = self.held.remove(&key);
        }
        if self.ptt == Some(key) {
            if !pressed {
                self.ptt = None;
                return vec![button(ButtonName::KeyboardPtt, false)];
            }
            return Vec::new();
        }
        let mut output = Vec::new();
        if let Some(meta) = self.pending_meta.take() {
            if key == meta && !pressed {
                return vec![
                    button(ButtonName::GuideTap, true),
                    button(ButtonName::GuideTap, false),
                ];
            }
            output.push(Output::Key(meta, true));
        }
        if pressed && matches!(key, 125 | 126) && self.held.len() == 1 {
            self.pending_meta = Some(key);
        } else if pressed && key == ptt && !self.held.contains(&125) && !self.held.contains(&126) {
            self.ptt = Some(key);
            output.push(button(ButtonName::KeyboardPtt, true));
        } else {
            output.push(Output::Key(key, pressed));
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_chord_with_the_ptt_key_passes_through() {
        let mut keyboard = Keyboard::default();
        assert_eq!(keyboard.key(126, true, 119), Vec::<Output>::new());
        assert_eq!(
            keyboard.key(119, true, 119),
            vec![Output::Key(126, true), Output::Key(119, true)]
        );
        assert_eq!(keyboard.key(119, false, 119), vec![Output::Key(119, false)]);
    }

    #[test]
    fn reset_releases_ptt_and_cancels_a_pending_meta_tap() {
        let mut keyboard = Keyboard::default();
        assert_eq!(
            keyboard.key(119, true, 119),
            vec![button(ButtonName::KeyboardPtt, true)]
        );
        assert_eq!(keyboard.reset(), vec![button(ButtonName::KeyboardPtt, false)]);
        assert_eq!(keyboard.key(125, true, 119), Vec::<Output>::new());
        assert_eq!(keyboard.reset(), Vec::<Output>::new());
    }

    #[test]
    fn meta_tap_routes_guide() {
        let mut keyboard = Keyboard::default();
        assert_eq!(keyboard.key(125, true, 100), Vec::<Output>::new());
        assert_eq!(
            keyboard.key(125, false, 100),
            vec![
                button(ButtonName::GuideTap, true),
                button(ButtonName::GuideTap, false)
            ]
        );
    }

    #[test]
    fn meta_chord_forwards_both_keys() {
        let mut keyboard = Keyboard::default();
        assert_eq!(keyboard.key(125, true, 100), Vec::<Output>::new());
        assert_eq!(
            keyboard.key(30, true, 100),
            vec![Output::Key(125, true), Output::Key(30, true)]
        );
        assert_eq!(keyboard.key(30, false, 100), vec![Output::Key(30, false)]);
        assert_eq!(keyboard.key(125, false, 100), vec![Output::Key(125, false)]);
    }

    #[test]
    fn configured_ptt_routes_press_and_release() {
        let mut keyboard = Keyboard::default();
        assert_eq!(
            keyboard.key(100, true, 100),
            vec![Output::Button(Event::Button {
                name: ButtonName::KeyboardPtt,
                state: ButtonState::Pressed
            })]
        );
        assert_eq!(
            keyboard.key(100, false, 97),
            vec![Output::Button(Event::Button {
                name: ButtonName::KeyboardPtt,
                state: ButtonState::Released
            })]
        );
        assert_eq!(keyboard.key(100, true, 97), vec![Output::Key(100, true)]);
    }
}
