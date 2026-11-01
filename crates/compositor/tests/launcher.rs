#[cfg(test)]
mod tests {
    use smithay::reexports::wayland_server::Display;
    use tehuti::{destinations::Paths, state::Compositor};
    use tehuti_protocol::{ButtonName, ButtonState, Event, Text};

    #[test]
    fn guide_item_names_rank_their_destination_first() {
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
            assert_eq!(
                menu.lines()
                    .filter(|line| line.trim_start_matches(['>', ' ']) == name)
                    .count(),
                1
            );
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

    #[test]
    fn an_empty_session_starts_on_the_invitation_and_cancel_brings_it_back()
    -> Result<(), Box<dyn core::error::Error>> {
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.enable_invitation();
        compositor.advance_time();
        let title = |shown: &Compositor| {
            shown
                .choice_list()
                .filter(|list| list.is_open())
                .map(|list| list.slots().title.clone())
        };
        assert_eq!(title(&compositor).as_deref(), Some("No controller connected"));
        compositor.handle(&Event::Button {
            name: ButtonName::East,
            state: ButtonState::Pressed,
        });
        compositor.handle(&Event::Button {
            name: ButtonName::East,
            state: ButtonState::Released,
        });
        compositor.handle(&Event::Tick {
            elapsed: core::time::Duration::from_millis(16),
        });
        assert_eq!(title(&compositor).as_deref(), Some("No controller connected"));
        Ok(())
    }

    #[test]
    fn volume_on_the_empty_session_opens_the_gauge_instead_of_the_invitation()
    -> Result<(), Box<dyn core::error::Error>> {
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.set_volume_backend(tehuti::volume::Backend::with_command(|_| {
            Ok("Volume: 0.50\n".into())
        })?);
        compositor.enable_invitation();
        compositor.advance_time();
        assert!(
            compositor.choice_list().is_some(),
            "the invitation starts the session"
        );
        compositor.say(Text::new("volume".into())?);
        let deadline = std::time::Instant::now() + core::time::Duration::from_secs(2);
        while compositor.adjuster().is_none() && std::time::Instant::now() < deadline {
            compositor.handle(&Event::Tick {
                elapsed: core::time::Duration::from_millis(16),
            });
            std::thread::yield_now();
        }
        assert_eq!(
            compositor.adjuster().map(tehuti_core::adjuster::Gauge::value),
            Some(50)
        );
        assert!(
            compositor.choice_list().is_none_or(|list| !list.is_open()),
            "the invitation stays away while the gauge is open"
        );
        Ok(())
    }

    #[test]
    fn an_open_instance_outranks_launching_another() {
        use tehuti::destinations::{Destination, Route, Source, Step, Target};
        let route = |target| Route {
            context: None,
            steps: vec![Step {
                icon: "terminal".into(),
                target,
            }],
        };
        let entries = [
            Destination {
                name: "kitty".into(),
                extra_names: Vec::new(),
                routes: vec![route(Target::Desktop("kitty.desktop".into()))],
                source: Source::Desktop,
            },
            Destination {
                name: "~: fish".into(),
                extra_names: vec!["Terminal".into(), "kitty".into()],
                routes: vec![route(Target::Window(3))],
                source: Source::Window,
            },
        ];
        let (rows, routes) = tehuti::launcher::destinations(
            &entries,
            tehuti_core::unit_verbs::DestinationKind::Application,
            None,
        );
        let mut launcher = tehuti_core::launcher::Launcher::new("kitty", rows, None);
        let picked = launcher.confirm().and_then(|index| routes.get(index));
        assert_eq!(
            picked.map(|chosen| &chosen.steps[0].target),
            Some(&Target::Window(3))
        );
    }
}
