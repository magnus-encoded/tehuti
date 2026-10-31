#[cfg(test)]
mod tests {
    use smithay::reexports::wayland_server::Display;
    use tehuti::{destinations::Paths, state::Compositor};
    use tehuti_protocol::{ButtonName, ButtonState, Event, Text};

    #[test]
    fn guide_items_show_phrases_that_rank_their_destination_first() {
        let root = tempfile::tempdir().unwrap();
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.configure_destinations(Paths {
            config: root.path().into(),
            projects: root.path().join("projects"),
            data: Vec::new(),
        });
        let menu = tehuti_core::menu::Menu::default().text();
        assert!(menu.lines().all(|line| line.chars().count() <= 40));
        for name in [
            "LOG OUT",
            "RESTART TEHUTI",
            "SWITCH TO PLASMA",
            "REBOOT",
            "SHUT DOWN",
            "TASK MONITOR",
            "SETTINGS",
        ] {
            assert!(menu.contains(&format!("open {name}")));
            compositor.handle(&Event::Speech {
                text: Text::new(format!("open {name}")).unwrap(),
            });
            assert_eq!(
                compositor.choice_list().unwrap().candidates()[0].candidate.name,
                name
            );
        }
    }

    #[test]
    fn a_menu_destination_keeps_power_commands_behind_confirmation() {
        let root = tempfile::tempdir().unwrap();
        let display = Display::<Compositor>::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.configure_destinations(Paths {
            config: root.path().into(),
            projects: root.path().join("projects"),
            data: Vec::new(),
        });
        compositor.say(Text::new("open RESTART TEHUTI".into()).unwrap());
        for expected in [false, false, true] {
            for state in [ButtonState::Pressed, ButtonState::Released] {
                compositor.handle(&Event::Button {
                    name: ButtonName::South,
                    state,
                });
            }
            assert_eq!(compositor.restart_requested(), expected);
        }
    }

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
