//! Select the host window or the login session's seat without opening a device.

use std::ffi::OsStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Nested,
    Tty,
}

#[must_use]
pub fn select(wayland_display: Option<&OsStr>, display: Option<&OsStr>) -> Backend {
    if wayland_display.is_some_and(|value| !value.is_empty())
        || display.is_some_and(|value| !value.is_empty())
    {
        Backend::Nested
    } else {
        Backend::Tty
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escape {
    ChangeVt(i32),
    Stop,
}

#[derive(Debug, Default)]
pub struct EscapeKeyboard {
    held: std::collections::BTreeSet<u32>,
}

impl EscapeKeyboard {
    #[must_use]
    pub fn key(&mut self, key: u32, pressed: bool) -> Option<Escape> {
        if !pressed {
            let _removed = self.held.remove(&key);
            return None;
        }
        if !self.held.insert(key) {
            return None;
        }
        let control = self.held.contains(&29) || self.held.contains(&97);
        let alt = self.held.contains(&56) || self.held.contains(&100);
        if !control || !alt {
            return None;
        }
        match key {
            59..=68 => Some(Escape::ChangeVt(i32::try_from(key).ok()?.checked_sub(58)?)),
            87 => Some(Escape::ChangeVt(11)),
            88 => Some(Escape::ChangeVt(12)),
            14 => Some(Escape::Stop),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Escape, EscapeKeyboard};

    #[test]
    fn control_alt_maps_every_function_key_and_backspace() {
        for control in [29, 97] {
            for alt in [56, 100] {
                for (key, number) in [
                    (59, 1),
                    (60, 2),
                    (61, 3),
                    (62, 4),
                    (63, 5),
                    (64, 6),
                    (65, 7),
                    (66, 8),
                    (67, 9),
                    (68, 10),
                    (87, 11),
                    (88, 12),
                ] {
                    let mut keyboard = EscapeKeyboard::default();
                    assert_eq!(keyboard.key(control, true), None);
                    assert_eq!(keyboard.key(alt, true), None);
                    assert_eq!(keyboard.key(key, true), Some(Escape::ChangeVt(number)));
                    assert_eq!(keyboard.key(key, true), None);
                    assert_eq!(keyboard.key(key, false), None);
                    assert_eq!(keyboard.key(14, true), Some(Escape::Stop));
                }
            }
        }
    }

    #[test]
    fn missing_modifiers_and_releases_do_not_escape() {
        for modifier in [29, 56] {
            let mut keyboard = EscapeKeyboard::default();
            assert_eq!(keyboard.key(modifier, true), None);
            assert_eq!(keyboard.key(59, true), None);
            assert_eq!(keyboard.key(14, true), None);
            assert_eq!(keyboard.key(modifier, false), None);
        }
        let mut keyboard = EscapeKeyboard::default();
        assert_eq!(keyboard.key(29, true), None);
        assert_eq!(keyboard.key(56, true), None);
        assert_eq!(keyboard.key(29, false), None);
        assert_eq!(keyboard.key(14, true), None);
    }
}
