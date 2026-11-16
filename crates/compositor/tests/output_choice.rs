//! The fallback output lights only when no other connector is connected.

use proptest::prelude::*;
use tehuti::output_choice::{fallback_from_config, try_order};

fn first<'a>(connected: &[&'a str], fallback: Option<&str>) -> Option<&'a str> {
    try_order(connected, fallback).first().map(|index| connected[*index])
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
    assert_eq!(first(&["HDMI-A-1", "HDMI-A-2"], Some("HDMI-A-1")), Some("HDMI-A-2"));
}

#[test]
fn config_reads_the_output_fallback() {
    assert_eq!(
        fallback_from_config("[output]\nfallback = \"HDMI-A-1\"\n"),
        Some("HDMI-A-1".to_owned())
    );
    assert_eq!(fallback_from_config("[output]\nfallback = \"\"\n"), None);
    assert_eq!(fallback_from_config("[speech]\nx = 1\n"), None);
    assert_eq!(fallback_from_config("not toml ["), None);
}

proptest! {
    #[test]
    fn order_is_a_permutation_with_the_fallback_last(
        names in prop::collection::hash_set("[A-Z]{2,3}-[1-4]", 0..6),
        pick in any::<prop::sample::Index>(),
    ) {
        let connected: Vec<&str> = names.iter().map(String::as_str).collect();
        let fallback = (!connected.is_empty()).then(|| connected[pick.index(connected.len())]);
        let order = try_order(&connected, fallback);
        let mut sorted = order.clone();
        sorted.sort_unstable();
        prop_assert_eq!(sorted, (0..connected.len()).collect::<Vec<_>>(), "nothing connected is dropped");
        if let Some(name) = fallback {
            prop_assert_eq!(connected[*order.last().unwrap()], name);
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
