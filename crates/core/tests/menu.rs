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
                assert_eq!(button(&mut menu, ButtonName::DpadDown, ButtonState::Pressed), None);
            }
            prop_assert_eq!(button(&mut menu, ButtonName::South, ButtonState::Pressed), None);
            prop_assert_eq!(button(&mut menu, if guide { ButtonName::Guide } else { ButtonName::East }, ButtonState::Pressed), None);
            prop_assert!(menu.is_none());
        }
    }

    #[test]
    fn menu_text_marks_the_selected_command_and_wraps_forward() {
        let mut menu = Some(Menu::default());
        assert_eq!(
            menu.as_ref().unwrap().text(),
            "SYSTEM MENU\n> LOG OUT\n  SWITCH TO PLASMA\n  REBOOT\n  SHUT DOWN\n  TASK MONITOR\n  SETTINGS\nB OR GUIDE CLOSE"
        );
        assert_eq!(button(&mut menu, ButtonName::DpadUp, ButtonState::Pressed), None);
        assert!(menu.as_ref().unwrap().text().contains("\n> SETTINGS"));
        assert_eq!(
            button(&mut menu, ButtonName::DpadDown, ButtonState::Pressed),
            None
        );
        assert!(menu.as_ref().unwrap().text().contains("\n> LOG OUT"));
    }

    #[test]
    fn command_keys_name_the_selected_session_action() {
        for (command, key) in [
            (Command::LogOut, "log_out"),
            (Command::Plasma, "plasma"),
            (Command::Reboot, "reboot"),
            (Command::ShutDown, "shut_down"),
            (Command::TaskMonitor, "task_monitor"),
            (Command::Settings, "settings"),
        ] {
            assert_eq!(command.key(), key);
        }
    }

    #[test]
    fn an_open_menu_updates_its_hints_when_the_pad_changes() {
        let mut menu = Menu::default();
        menu.set_pad_family(tehuti_core::pad::PadFamily::Symbols);
        assert!(menu.text().contains("Circle OR GUIDE CLOSE"));
    }

    #[test]
    fn symbol_pad_menu_names_its_confirm_and_back_buttons() {
        let mut menu = Some(Menu::new(tehuti_core::pad::PadFamily::Symbols));
        assert!(menu.as_ref().unwrap().text().contains("Circle OR GUIDE CLOSE"));
        assert_eq!(button(&mut menu, ButtonName::South, ButtonState::Pressed), None);
        assert!(
            menu.as_ref()
                .unwrap()
                .text()
                .contains("Cross CONFIRM / Circle CANCEL")
        );
        assert_eq!(button(&mut menu, ButtonName::East, ButtonState::Pressed), None);
        assert!(menu.is_none());
    }

    #[test]
    fn moving_after_confirmation_requires_two_fresh_presses() {
        for direction in [ButtonName::DpadUp, ButtonName::DpadDown] {
            let mut menu = Some(Menu::default());
            assert_eq!(
                button(&mut menu, ButtonName::DpadDown, ButtonState::Pressed),
                None
            );
            assert_eq!(button(&mut menu, ButtonName::South, ButtonState::Pressed), None);
            assert_eq!(button(&mut menu, ButtonName::South, ButtonState::Released), None);
            assert_eq!(button(&mut menu, direction, ButtonState::Pressed), None);
            assert!(menu.as_ref().is_some_and(|open| !open.text().contains("CONFIRM")));
            assert_eq!(button(&mut menu, ButtonName::South, ButtonState::Pressed), None);
            assert!(menu.as_ref().is_some_and(|open| open.text().contains("CONFIRM")));
        }
    }

    #[test]
    fn up_wraps_to_settings() {
        let mut menu = Some(Menu::default());
        assert_eq!(button(&mut menu, ButtonName::DpadUp, ButtonState::Pressed), None);
        assert_eq!(
            button(&mut menu, ButtonName::South, ButtonState::Pressed),
            Some(Command::Settings)
        );
        assert!(menu.is_none());
    }
}

#[cfg(test)]
mod defaults {
    use tehuti_protocol::{ButtonName, parse_context};

    #[test]
    fn shipped_contexts_leave_guide_reserved() {
        for source in [
            include_str!("../../../examples/global/context.toml"),
            include_str!("../../../examples/foot/context.toml"),
            include_str!("../../../dist/contexts/firefox/context.toml"),
        ] {
            let context = parse_context(source).unwrap();
            assert_eq!(context.action_for(ButtonName::Guide), None);
            assert_ne!(context.keyboard().button, Some(ButtonName::Guide));
        }
    }
}
