//! Font choices flow through settings, persistence and the rendered powerline.

#[cfg(test)]
mod tests {
    use smithay::reexports::wayland_server::Display;
    use tehuti::{display_mode_store::Store, state::Compositor};
    use tehuti_core::bar::Font;
    use tehuti_protocol::{ButtonName, ButtonState, Event, Text};

    fn press(compositor: &mut Compositor, name: ButtonName) {
        for state in [ButtonState::Released, ButtonState::Pressed] {
            compositor.handle(&Event::Button { name, state });
        }
    }

    #[test]
    fn choosing_a_bar_font_applies_live_and_survives_a_restart() -> Result<(), Box<dyn core::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("bar-font");
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.load_bar_font(Store::new(path.clone()));
        assert_eq!(compositor.voice_bar().font, Font::LiberationMono);
        compositor.handle(&Event::Speech {
            text: Text::new("pop the hood".into())?,
        });
        press(&mut compositor, ButtonName::South);
        press(&mut compositor, ButtonName::DpadDown);
        press(&mut compositor, ButtonName::DpadRight);
        assert_eq!(compositor.voice_bar().font, Font::Bitmap);
        assert_eq!(Store::new(path.clone()).read()?.as_deref(), Some("Bitmap"));
        let mut restarted = Compositor::new(&display.handle())?;
        restarted.load_bar_font(Store::new(path.clone()));
        assert_eq!(restarted.voice_bar().font, Font::Bitmap);
        press(&mut compositor, ButtonName::DpadLeft);
        assert_eq!(compositor.voice_bar().font, Font::LiberationMono);
        restarted.load_bar_font(Store::new(path));
        assert_eq!(restarted.voice_bar().font, Font::LiberationMono);
        press(&mut compositor, ButtonName::South);
        assert_eq!(compositor.voice_bar().font, Font::Bitmap);
        Ok(())
    }

    #[test]
    fn a_failed_save_keeps_the_current_font() -> Result<(), Box<dyn core::error::Error>> {
        let directory = tempfile::tempdir()?;
        let file = directory.path().join("file");
        std::fs::write(&file, "not a directory")?;
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.load_bar_font(Store::new(file.join("bar-font")));
        compositor.handle(&Event::Speech {
            text: Text::new("pop the hood".into())?,
        });
        press(&mut compositor, ButtonName::South);
        press(&mut compositor, ButtonName::DpadDown);
        press(&mut compositor, ButtonName::DpadRight);
        assert_eq!(compositor.voice_bar().font, Font::LiberationMono);
        Ok(())
    }
}
