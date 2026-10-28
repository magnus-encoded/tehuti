//! The pad identity boundary keeps device facts and interaction roles together.

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tehuti_core::pad::{Identity, PadFamily, Role};
    use tehuti_core::ticker::{Piece, Rgb, Shape};
    use tehuti_protocol::ButtonName;

    proptest! {
        #[test]
        fn device_products_preserve_the_supported_vendor_profiles(product in any::<u16>()) {
            let nintendo = Identity::for_device(0x057e, product);
            prop_assert_eq!(nintendo.family, PadFamily::Nintendo);
            prop_assert!(!nintendo.swap_north_west);
            let symbols = Identity::for_device(0x054c, product);
            prop_assert_eq!(symbols.family, PadFamily::Symbols);
            prop_assert!(!symbols.swap_north_west);
            let letters = Identity::for_device(0x045e, product);
            prop_assert_eq!(letters.family, PadFamily::Letters);
            prop_assert!(letters.swap_north_west);
        }

        #[test]
        fn unknown_devices_keep_positional_letter_buttons(
            vendor in any::<u16>().prop_filter("supported vendors have their own profiles", |vendor| ![0x054c, 0x045e, 0x057e].contains(vendor)),
            product in any::<u16>(),
        ) {
            let identity = Identity::for_device(vendor, product);
            prop_assert_eq!(identity.family, PadFamily::Letters);
            prop_assert!(!identity.swap_north_west);
        }
    }

    #[test]
    fn nintendo_devices_confirm_on_printed_a_and_back_on_printed_b() {
        let identity = Identity::for_device(0x057e, 0x2009);
        assert_eq!(identity.family, PadFamily::Nintendo);
        assert_eq!(identity.family.button(Role::Confirm), ButtonName::East);
        assert_eq!(identity.family.button(Role::Back), ButtonName::South);
        assert_eq!(identity.family.button(Role::Keyboard), ButtonName::North);
        assert!(!identity.swap_north_west);
        for (button, letter) in [
            (ButtonName::East, 'A'),
            (ButtonName::South, 'B'),
            (ButtonName::North, 'X'),
            (ButtonName::West, 'Y'),
        ] {
            assert_eq!(identity.family.label(button), letter.to_string());
            assert_eq!(
                identity.family.piece(button),
                Piece::Glyph {
                    shape: Shape::Disc(letter),
                    colour: Rgb::WHITE,
                }
            );
        }
    }

    #[test]
    fn supported_families_use_the_existing_role_positions() {
        for family in [PadFamily::Letters, PadFamily::Symbols] {
            assert_eq!(family.button(Role::Confirm), ButtonName::South);
            assert_eq!(family.button(Role::Back), ButtonName::East);
            assert_eq!(family.button(Role::Keyboard), ButtonName::North);
        }
    }

    #[test]
    fn every_physical_button_keeps_its_family_label() {
        let buttons = [
            ButtonName::South,
            ButtonName::East,
            ButtonName::West,
            ButtonName::North,
            ButtonName::LeftBumper,
            ButtonName::RightBumper,
            ButtonName::Select,
            ButtonName::Start,
            ButtonName::Record,
            ButtonName::Guide,
            ButtonName::LeftStick,
            ButtonName::RightStick,
            ButtonName::DpadUp,
            ButtonName::DpadDown,
            ButtonName::DpadLeft,
            ButtonName::DpadRight,
        ];
        for (family, labels) in [
            (
                PadFamily::Letters,
                [
                    "A", "B", "X", "Y", "LB", "RB", "View", "Menu", "Share", "Guide", "LS", "RS", "Up",
                    "Down", "Left", "Right",
                ],
            ),
            (
                PadFamily::Symbols,
                [
                    "Cross", "Circle", "Square", "Triangle", "L1", "R1", "Share", "Options", "Record",
                    "Guide", "L3", "R3", "Up", "Down", "Left", "Right",
                ],
            ),
        ] {
            for (button, label) in buttons.into_iter().zip(labels) {
                assert_eq!(family.label(button), label);
            }
        }
    }

    #[test]
    fn role_glyphs_follow_the_connected_family() {
        for (family, role, shape, colour) in [
            (
                PadFamily::Letters,
                Role::Confirm,
                Shape::Disc('A'),
                Rgb(76, 175, 80),
            ),
            (PadFamily::Letters, Role::Back, Shape::Disc('B'), Rgb(229, 57, 53)),
            (
                PadFamily::Letters,
                Role::Keyboard,
                Shape::Disc('Y'),
                Rgb(253, 216, 53),
            ),
            (
                PadFamily::Symbols,
                Role::Confirm,
                Shape::Cross,
                Rgb(124, 178, 232),
            ),
            (PadFamily::Symbols, Role::Back, Shape::Ring, Rgb(255, 102, 102)),
            (
                PadFamily::Symbols,
                Role::Keyboard,
                Shape::Triangle,
                Rgb(64, 226, 160),
            ),
        ] {
            assert_eq!(family.glyph(role), Piece::Glyph { shape, colour });
        }
    }
}
