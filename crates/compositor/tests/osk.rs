//! The on-screen keyboard docks above the bar and takes its height from tiled windows.

#[cfg(test)]
mod osk {
    use proptest::prelude::*;
    use smithay::reexports::wayland_server::Display;
    use tehuti::state::Compositor;
    use tehuti_protocol::{ButtonName, ButtonState, Event, Text};

    fn usable_height(compositor: &Compositor) -> f64 {
        compositor
            .layout_snapshot()
            .outputs
            .first()
            .unwrap()
            .geometry
            .usable
            .height
    }

    fn say(compositor: &mut Compositor, text: &str) {
        compositor.handle(&Event::Speech {
            text: Text::new(text.into()).unwrap(),
        });
    }

    fn tap(compositor: &mut Compositor, name: ButtonName) {
        for state in [ButtonState::Pressed, ButtonState::Released] {
            compositor.handle(&Event::Button { name, state });
        }
    }

    #[test]
    fn saying_keyboard_reserves_its_height_and_b_gives_it_back() {
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.fill_output();
        let closed = usable_height(&compositor);
        say(&mut compositor, "keyboard");
        assert!(compositor.on_screen_keyboard().is_open());
        assert_eq!(
            closed - usable_height(&compositor),
            f64::from(tehuti::osk::logical_height(compositor.on_screen_keyboard()))
        );
        say(&mut compositor, "hello");
        tap(&mut compositor, ButtonName::East);
        assert!(!compositor.on_screen_keyboard().is_open());
        assert_eq!(usable_height(&compositor), closed);
    }

    #[test]
    fn rows_are_as_tall_as_the_bar() {
        assert_eq!(
            tehuti::osk::logical_height(&tehuti_core::keyboard::Keyboard::default()),
            tehuti_core::bar::HEIGHT.saturating_mul(4)
        );
    }

    proptest! {
        #[test]
        fn the_keyboard_sits_on_the_bar_and_fits_the_output(height in 0i32..4000, scale in 1i32..4, terminal in any::<bool>()) {
            let mut keyboard_layout = tehuti_core::keyboard::Keyboard::default();
            keyboard_layout.set_terminal(terminal);
            let (top, keyboard) = tehuti::osk::placement(height, scale, &keyboard_layout);
            let bar = tehuti_core::bar::HEIGHT.saturating_mul(scale);
            prop_assert!(top >= 0 && keyboard >= 0);
            prop_assert!(keyboard <= tehuti::osk::logical_height(&keyboard_layout).saturating_mul(scale));
            if height >= bar {
                prop_assert_eq!(top.saturating_add(keyboard), height.saturating_sub(bar));
            }
        }
    }
}
