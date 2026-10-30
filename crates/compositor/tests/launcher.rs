#[cfg(test)]
mod tests {
    use smithay::reexports::wayland_server::Display;
    use tehuti::{destinations::Paths, state::Compositor};
    use tehuti_protocol::{ButtonName, ButtonState, Event, Text};

    #[test]
    fn spoken_unit_shows_a_route_and_confirm_carries_it_out() -> Result<(), Box<dyn core::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::write(
            root.path().join("destinations.toml"),
            r#"
[[destinations]]
name = "Library"
[[destinations.routes]]
steps = [{ icon = "book", target = { kind = "action", value = { type = "notice", text = "Route reached" } } }]
"#,
        )?;
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.configure_destinations(Paths {
            config: root.path().into(),
            projects: root.path().join("projects"),
            data: Vec::new(),
        });
        compositor.handle(&Event::Speech {
            text: Text::new("open library".into())?,
        });
        let list = compositor.choice_list().ok_or("launcher missing")?;
        assert_eq!(
            list.candidates().first().map(|row| row.candidate.name.as_str()),
            Some("Library")
        );
        assert_eq!(
            list.candidates()
                .first()
                .and_then(|row| row.candidate.meta.as_deref()),
            Some("book: Library")
        );
        compositor.handle(&Event::Button {
            name: ButtonName::South,
            state: ButtonState::Pressed,
        });
        assert_eq!(compositor.visible_notice_text().as_deref(), Some("Route reached"));
        Ok(())
    }
}
