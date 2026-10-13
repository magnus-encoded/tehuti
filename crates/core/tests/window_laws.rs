//! Window laws across arbitrary lifecycle and action sequences.

#[cfg(test)]
mod window_laws {
    use proptest::prelude::*;
    use tehuti_core::windows::{Effect, Input, Windows};

    proptest! {
        #[test]
        fn focus_survives_arbitrary_lifecycle_and_action_sequences(
            installed in prop::collection::vec(any::<tehuti_protocol::Text>(), 0..30),
            events in prop::collection::vec((0_u8..6, any::<u8>(), any::<tehuti_protocol::Text>()), 0..300),
        ) {
            let mut windows = Windows::new();
            let mut existing = std::collections::HashSet::new();
            for (kind, window, entry_id) in events {
                let before = windows.focus().copied();
                let input = match kind {
                    0 => { let _inserted = existing.insert(window); Input::Map(window) }
                    1 => { let _removed = existing.remove(&window); Input::Unmap(window) }
                    2 => {
                        if let Some(focused) = before { let _removed = existing.remove(&focused); }
                        Input::Close
                    }
                    3 => Input::Launch { desktop_entry: entry_id.as_str().into(), installed: installed.iter().chain(core::iter::once(&entry_id)).map(|entry| entry.as_str().into()).collect() },
                    4 => Input::Launch { desktop_entry: entry_id.as_str().into(), installed: installed.iter().filter(|entry| **entry != entry_id).map(|entry| entry.as_str().into()).collect() },
                    _ => Input::Notify,
                };
                let effect = windows.step(input);
                let after = windows.focus().copied();
                prop_assert_eq!(after.is_none(), existing.is_empty());
                if let Some(focused) = after { prop_assert!(existing.contains(&focused)); }
                if matches!(kind, 3..=5) { prop_assert_eq!(after, before); }
                if kind == 4 || kind == 5 { prop_assert_eq!(effect.as_ref(), Some(&Effect::Notice)); }
                if kind == 3 { prop_assert_eq!(effect.as_ref(), Some(&Effect::Launch)); }
                if kind == 2 {
                    prop_assert_eq!(effect, before.map(Effect::Close));
                    if before.is_some() { prop_assert_ne!(after, before); }
                }
            }
        }
    }
}
