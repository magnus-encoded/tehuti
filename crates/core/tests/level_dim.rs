//! Focus selects dimming overrides independently of the level.
#[cfg(test)]
mod level_dim {
    use core::time::Duration;
    use proptest::prelude::*;
    use tehuti_core::{State, step};
    use tehuti_protocol::{Event, parse_context};

    proptest! {
        #[test]
        #[expect(clippy::float_arithmetic, reason = "compare generated colour fractions within a tolerance")]
        fn focus_and_reload_resolve_each_dim_option_over_the_global_context(
            global in any::<bool>(), override_enabled in prop::option::of(any::<bool>()),
            strength in 0u16..=1000, fade in any::<u16>(),
        ) {
            let global_context = parse_context(&format!("app_id = '*'\n[buttons]\n[dim]\nenabled = {global}\nstrength = 0.5\nfade_ms = {fade}\n")).unwrap();
            let enabled = override_enabled.map_or_else(String::new, |value| format!("enabled = {value}\n"));
            let active = parse_context(&format!("app_id = 'org.example.Editor'\n[buttons]\n[dim]\n{enabled}strength = {}\n", f32::from(strength) / 1000.0)).unwrap();
            let state = State::with_contexts(vec![global_context.clone(), active]);
            let (mut state, _) = step(state, &Event::Focus { app_id: Some("org.example.Editor".into()), title: String::new() });
            let (actual_enabled, actual_strength, actual_fade) = state.dim_config();
            prop_assert_eq!(actual_enabled, override_enabled.unwrap_or(global));
            prop_assert!((actual_strength - f32::from(strength) / 1000.0).abs() < 0.0001);
            prop_assert_eq!(actual_fade, Duration::from_millis(u64::from(fade)));
            state.replace_contexts(vec![global_context]);
            prop_assert_eq!(state.dim_config(), (global, 0.5, Duration::from_millis(u64::from(fade))));
        }
    }

    #[test]
    fn invalid_dim_strengths_are_rejected_at_the_toml_boundary() {
        assert!(parse_context("app_id = '*'\n[buttons]\n[dim]\nstrength = 0.67\n").is_ok());
        for value in ["-0.01", "1.01", "nan", "inf", "-inf"] {
            assert!(parse_context(&format!("app_id = '*'\n[buttons]\n[dim]\nstrength = {value}\n")).is_err());
        }
    }
}
