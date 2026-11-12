#[cfg(test)]
mod tests {
    use smithay::reexports::wayland_server::Display;
    use tehuti::{destinations::Paths, state::Compositor};
    use tehuti_protocol::{ButtonName, ButtonState, Event, Text};

    #[test]
    fn launcher_artwork_keeps_icons_with_ranked_routes_at_both_screen_sizes()
    -> Result<(), Box<dyn core::error::Error>> {
        const PROBE: &str = "TEHUTI_LAUNCHER_ICON_PROBE";
        if std::env::var_os(PROBE).is_none() {
            for size in ["1024x600", "1920x1080"] {
                let status = std::process::Command::new(std::env::current_exe()?)
                    .args([
                        "--exact",
                        "tests::launcher_artwork_keeps_icons_with_ranked_routes_at_both_screen_sizes",
                        "--nocapture",
                    ])
                    .env(PROBE, "1")
                    .env("TEHUTI_TEST_SIZE", size)
                    .status()?;
                assert!(status.success(), "icon artwork failed at {size}");
            }
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        icon_destinations(root.path())?;
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        tehuti::driver::headless_output(&display.handle(), &mut compositor)?;
        compositor.configure_destinations(Paths {
            config: root.path().into(),
            projects: root.path().join("projects"),
            data: Vec::new(),
        });
        let mut driver = tehuti::driver::Driver::bind(root.path(), tehuti::backend::Backend::Nested)?;
        for query in ["open Browser Zulu", "open Browser Alpha"] {
            let _scene = driver.execute(
                &mut compositor,
                tehuti::driver::Request::Say { text: query.into() },
            )?;
            let path = root.path().join("shot.png");
            // Icons decode off the render thread, so poll until they land.
            let mut attempts = 0_u32;
            loop {
                let _shot = driver.execute(
                    &mut compositor,
                    tehuti::driver::Request::Shot { path: path.clone() },
                )?;
                let pixels = image::open(&path)?.to_rgba8();
                attempts = attempts.saturating_add(1);
                match assert_ranked_icons(&pixels, query) {
                    Ok(()) => break,
                    Err(_) if attempts < 100 => std::thread::sleep(core::time::Duration::from_millis(50)),
                    Err(error) => return Err(error),
                }
            }
            if let Ok(copy) = std::env::var("TEHUTI_SHOT_COPY") {
                let _bytes = std::fs::copy(
                    &path,
                    format!("{copy}-{}.png", std::env::var("TEHUTI_TEST_SIZE")?),
                )?;
            }
        }
        Ok(())
    }

    fn assert_ranked_icons(
        pixels: &image::RgbaImage,
        query: &str,
    ) -> Result<(), Box<dyn core::error::Error>> {
        let red_top = pixels
            .enumerate_pixels()
            .filter(|(_, _, pixel)| pixel.0 == [255, 0, 0, 255])
            .map(|(_, y, _)| y)
            .min()
            .ok_or("red icon absent")?;
        let blue_top = pixels
            .enumerate_pixels()
            .filter(|(_, _, pixel)| pixel.0 == [0, 0, 255, 255])
            .map(|(_, y, _)| y)
            .min()
            .ok_or("blue icon absent")?;
        if !pixels.pixels().any(|pixel| pixel.0 == [0, 200, 0, 255]) {
            return Err("svg icon absent".into());
        }
        assert_eq!(
            blue_top < red_top,
            query == "open Browser Zulu",
            "icons must follow ranked routes"
        );
        Ok(())
    }

    fn icon_destinations(root: &std::path::Path) -> Result<(), Box<dyn core::error::Error>> {
        let red = root.join("red.png");
        let blue = root.join("blue.png");
        image::RgbaImage::from_pixel(48, 48, image::Rgba([255, 0, 0, 255])).save(&red)?;
        image::RgbaImage::from_pixel(48, 24, image::Rgba([0, 0, 255, 255])).save(&blue)?;
        let green = root.join("green.svg");
        std::fs::write(
            &green,
            "<svg xmlns='http://www.w3.org/2000/svg' width='64' height='64'><circle cx='32' cy='32' r='30' fill='#00c800'/></svg>",
        )?;
        std::fs::write(
            root.join("destinations.toml"),
            format!(
                r#"
[[destinations]]
name = "Browser Alpha"
routes = [{{steps = [{{icon = '{}', target = {{kind = 'action', value = {{type = 'notice', text = 'Alpha'}}}}}}]}}]
[[destinations]]
name = "Browser Zulu"
routes = [{{steps = [{{icon = '{}', target = {{kind = 'action', value = {{type = 'notice', text = 'Zulu'}}}}}}]}}]
[[destinations]]
name = "Browser Vector"
routes = [{{steps = [{{icon = '{}', target = {{kind = 'action', value = {{type = 'notice', text = 'Vector'}}}}}}]}}]
[[destinations]]
name = "Browser missing icon"
routes = [{{steps = [{{icon = 'missing-icon-for-547', target = {{kind = 'action', value = {{type = 'notice', text = 'Missing'}}}}}}]}}]
"#,
                red.display(),
                blue.display(),
                green.display()
            ),
        )?;
        Ok(())
    }

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

#[cfg(test)]
mod hardware_keyboard {
    use smithay::{input::keyboard::Keycode, reexports::wayland_server::Display};
    use tehuti::{destinations::Paths, state::Compositor};
    use tehuti_protocol::Text;

    const KEY_ESC: u32 = 1;
    const KEY_BACKSPACE: u32 = 14;
    const KEY_ENTER: u32 = 28;
    const KEY_K: u32 = 37;
    const KEY_I: u32 = 23;
    const KEY_T: u32 = 20;
    const KEY_O: u32 = 24;

    fn tap(compositor: &mut Compositor, code: u32) {
        for pressed in [true, false] {
            compositor.host_key(Keycode::new(code.saturating_add(8)), pressed);
        }
    }

    fn session(
        root: &std::path::Path,
    ) -> Result<(Display<Compositor>, Compositor), Box<dyn core::error::Error>> {
        std::fs::write(
            root.join("destinations.toml"),
            r#"
[[destinations]]
name = "Kitty"
[[destinations.routes]]
steps = [{ icon = "terminal", target = { kind = "action", value = { type = "notice", text = "Kitty opened" } } }]
[[destinations]]
name = "Orbit"
[[destinations.routes]]
steps = [{ icon = "globe", target = { kind = "action", value = { type = "notice", text = "Orbit opened" } } }]
"#,
        )?;
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        compositor.configure_destinations(Paths {
            config: root.into(),
            projects: root.join("projects"),
            data: Vec::new(),
        });
        // A pad is connected by default; speech opens the launcher on an unrelated query.
        compositor.say(Text::new("open zzz".into())?);
        Ok((display, compositor))
    }

    fn first_row(compositor: &Compositor) -> Option<String> {
        compositor
            .choice_list()
            .filter(|list| list.is_open())
            .and_then(|list| list.candidates().first().map(|row| row.candidate.name.clone()))
    }

    #[test]
    fn typing_kit_and_enter_launches_kitty_with_a_pad_connected() -> Result<(), Box<dyn core::error::Error>> {
        let root = tempfile::tempdir()?;
        let (_display, mut compositor) = session(root.path())?;
        assert!(
            compositor
                .choice_list()
                .is_some_and(tehuti_core::choice_list::List::is_open),
            "open after speech"
        );
        for code in [KEY_BACKSPACE, KEY_BACKSPACE, KEY_BACKSPACE] {
            tap(&mut compositor, code);
        }
        assert!(
            compositor
                .choice_list()
                .is_some_and(tehuti_core::choice_list::List::is_open),
            "open after erase"
        );
        for code in [KEY_K, KEY_I, KEY_T] {
            tap(&mut compositor, code);
        }
        assert_eq!(first_row(&compositor).as_deref(), Some("Kitty"));
        tap(&mut compositor, KEY_ENTER);
        assert_eq!(compositor.visible_notice_text().as_deref(), Some("Kitty opened"));
        Ok(())
    }

    #[test]
    fn backspace_widens_the_filter_and_escape_closes_the_launcher() -> Result<(), Box<dyn core::error::Error>>
    {
        let root = tempfile::tempdir()?;
        let (_display, mut compositor) = session(root.path())?;
        for code in [KEY_BACKSPACE, KEY_BACKSPACE, KEY_BACKSPACE, KEY_O] {
            tap(&mut compositor, code);
        }
        assert_eq!(first_row(&compositor).as_deref(), Some("Orbit"));
        tap(&mut compositor, KEY_ESC);
        assert_eq!(first_row(&compositor), None);
        assert_eq!(compositor.visible_notice_text(), None);
        Ok(())
    }

    /// Renders the filtered list to the path in `TEHUTI_SHOT_COPY`; a no-op otherwise.
    #[test]
    fn typed_filter_renders_for_review() -> Result<(), Box<dyn core::error::Error>> {
        let Ok(copy) = std::env::var("TEHUTI_SHOT_COPY") else {
            return Ok(());
        };
        let root = tempfile::tempdir()?;
        let (display, mut compositor) = session(root.path())?;
        tehuti::driver::headless_output(&display.handle(), &mut compositor)?;
        for code in [KEY_BACKSPACE, KEY_BACKSPACE, KEY_BACKSPACE, KEY_K, KEY_I] {
            tap(&mut compositor, code);
        }
        let mut driver = tehuti::driver::Driver::bind(root.path(), tehuti::backend::Backend::Nested)?;
        let _scene = driver.execute(
            &mut compositor,
            tehuti::driver::Request::Shot { path: copy.into() },
        )?;
        Ok(())
    }
}
