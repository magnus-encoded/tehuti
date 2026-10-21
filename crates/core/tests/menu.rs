#[cfg(test)]
mod menu {
    use proptest::prelude::*;
    use tehuti_core::menu::{Command, Menu, handle};
    use tehuti_protocol::{ButtonName, ButtonState, Event};

    fn button(menu: &mut Option<Menu>, name: ButtonName, state: ButtonState) -> Option<Command> {
        handle(menu, &Event::Button { name, state })
    }

    proptest! {
        #[test]
        fn session_commands_require_two_distinct_presses(row in 0usize..4, repeats in 0usize..20) {
            let mut menu = Some(Menu::default());
            for _ in 0..row {
                assert_eq!(button(&mut menu, ButtonName::DpadDown, ButtonState::Pressed), None);
            }
            prop_assert_eq!(button(&mut menu, ButtonName::South, ButtonState::Pressed), None);
            for _ in 0..repeats {
                prop_assert_eq!(button(&mut menu, ButtonName::South, ButtonState::Pressed), None);
            }
            prop_assert!(menu.as_ref().is_some_and(|open| open.text().contains("CONFIRM")));
            prop_assert_eq!(button(&mut menu, ButtonName::South, ButtonState::Released), None);
            let command = button(&mut menu, ButtonName::South, ButtonState::Pressed);
            prop_assert!(command.is_some());
            prop_assert!(menu.is_none());
        }

        #[test]
        fn back_cancels_confirmation(row in 0usize..4, guide in any::<bool>()) {
            let mut menu = Some(Menu::default());
            for _ in 0..row {
                let _command = button(&mut menu, ButtonName::DpadDown, ButtonState::Pressed);
            }
            prop_assert_eq!(button(&mut menu, ButtonName::South, ButtonState::Pressed), None);
            prop_assert_eq!(button(&mut menu, if guide { ButtonName::Guide } else { ButtonName::East }, ButtonState::Pressed), None);
            prop_assert!(menu.is_none());
        }
    }

    #[test]
    fn up_wraps_to_task_monitor() {
        let mut menu = Some(Menu::default());
        assert_eq!(button(&mut menu, ButtonName::DpadUp, ButtonState::Pressed), None);
        assert_eq!(button(&mut menu, ButtonName::South, ButtonState::Pressed), Some(Command::TaskMonitor));
        assert!(menu.is_none());
    }
}
