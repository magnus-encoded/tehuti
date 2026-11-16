//! The fallback output lights only when no other connector is connected.

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tehuti::output_choice::{connector_name, try_order};

    fn first<'a>(connected: &[&'a str], fallback: Option<&str>) -> Option<&'a str> {
        try_order(connected, fallback)
            .first()
            .and_then(|index| connected.get(*index).copied())
    }

    #[test]
    fn plugged_in_monitor_is_lit_over_the_fallback() {
        let before = ["HDMI-A-1"];
        assert_eq!(first(&before, Some("HDMI-A-1")), Some("HDMI-A-1"));
        let after = ["HDMI-A-1", "DP-2"];
        assert_eq!(first(&after, Some("HDMI-A-1")), Some("DP-2"));
    }

    #[test]
    fn unplugging_the_monitor_lights_the_fallback_again() {
        assert_eq!(first(&["DP-2", "HDMI-A-1"], Some("HDMI-A-1")), Some("DP-2"));
        assert_eq!(first(&["HDMI-A-1"], Some("HDMI-A-1")), Some("HDMI-A-1"));
    }

    #[test]
    fn connectors_listed_after_the_fallback_win_over_it() {
        assert_eq!(
            first(&["HDMI-A-1", "HDMI-A-2"], Some("HDMI-A-1")),
            Some("HDMI-A-2")
        );
    }

    proptest! {
        #[test]
        fn order_is_a_permutation_with_the_fallback_last(
            names in prop::collection::hash_set("[A-Z]{2,3}-[1-4]", 0..6),
            pick in any::<prop::sample::Index>(),
        ) {
            let connected: Vec<&str> = names.iter().map(String::as_str).collect();
            let fallback = connected.get(pick.index(connected.len().max(1))).copied();
            let order = try_order(&connected, fallback);
            let mut sorted = order.clone();
            sorted.sort_unstable();
            prop_assert_eq!(sorted, (0..connected.len()).collect::<Vec<_>>(), "nothing connected is dropped");
            if let Some(name) = fallback {
                prop_assert_eq!(order.last().and_then(|index| connected.get(*index).copied()).unwrap_or_default(), name);
            }
        }

        #[test]
        fn without_a_matching_fallback_kernel_order_is_kept(
            names in prop::collection::hash_set("[A-Z]{2,3}-[1-4]", 0..6),
        ) {
            let connected: Vec<&str> = names.iter().map(String::as_str).collect();
            prop_assert_eq!(try_order(&connected, None), (0..connected.len()).collect::<Vec<_>>());
            prop_assert_eq!(try_order(&connected, Some("absent-1")), (0..connected.len()).collect::<Vec<_>>());
        }
    }

    #[test]
    fn connector_names_follow_the_kernel_for_every_interface() {
        use smithay::reexports::drm::control::connector::Interface::{
            Component, Composite, DPI, DSI, DVIA, DVID, DVII, DisplayPort, EmbeddedDisplayPort, HDMIA, HDMIB,
            LVDS, NinePinDIN, SPI, SVideo, TV, USB, Unknown, VGA, Virtual, Writeback,
        };
        let expected = [
            (Unknown, "Unknown"),
            (VGA, "VGA"),
            (DVII, "DVI-I"),
            (DVID, "DVI-D"),
            (DVIA, "DVI-A"),
            (Composite, "Composite"),
            (SVideo, "SVIDEO"),
            (LVDS, "LVDS"),
            (Component, "Component"),
            (NinePinDIN, "DIN"),
            (DisplayPort, "DP"),
            (HDMIA, "HDMI-A"),
            (HDMIB, "HDMI-B"),
            (TV, "TV"),
            (EmbeddedDisplayPort, "eDP"),
            (Virtual, "Virtual"),
            (DSI, "DSI"),
            (DPI, "DPI"),
            (Writeback, "Writeback"),
            (SPI, "SPI"),
            (USB, "USB"),
        ];
        for (interface, kernel) in expected {
            assert_eq!(connector_name(interface, 7), format!("{kernel}-7"));
        }
    }
}
