#[cfg(test)]
mod tests {
    use std::error::Error;

    use smithay::reexports::wayland_server::Display;
    use tehuti::{display_mode_store::Store, state::Compositor};

    #[test]
    fn failed_save_shows_a_notice_and_keeps_the_confirmed_mode() -> Result<(), Box<dyn Error>> {
        let display: Display<Compositor> = Display::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.configure_display_modes(vec!["1920x1080@60".into(), "3840x2160@60".into()], 1);
        let directory = tempfile::tempdir()?;
        let parent = directory.path().join("file");
        std::fs::write(&parent, "not a directory")?;
        let result = Store::new(parent.join("display-mode")).write("3840x2160@60");
        assert!(result.is_err());
        compositor.display_mode_saved(result);
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("mode not saved")
        );
        assert_eq!(compositor.settings().current, 1);
        assert!(!compositor.quit_requested());
        assert!(compositor.take_settings_request().is_none());
        Ok(())
    }
}
