//! End to end without a display or devices: a synthetic pad press reaches a Wayland client as a key.

#[cfg(test)]
mod synthetic_pad {
    use core::{
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };
    use std::{
        io::{Read, Seek, SeekFrom},
        os::unix::net::UnixStream,
        sync::{Arc, mpsc},
        thread,
        time::Instant,
    };

    use proptest::prelude::*;
    use smithay::reexports::wayland_server::Display;
    use tehuti::{
        input::{SyntheticPad, route},
        state::{ClientState, Compositor},
    };
    use tehuti_protocol::{ButtonName, ButtonState, Event};
    use wayland_client::{
        Connection, Dispatch, QueueHandle, WEnum, delegate_noop,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{
            wl_compositor::WlCompositor,
            wl_keyboard::{self, KeyState, WlKeyboard},
            wl_registry::WlRegistry,
            wl_seat::WlSeat,
            wl_surface::WlSurface,
        },
    };
    use wayland_protocols::wp::text_input::zv3::client::{
        zwp_text_input_manager_v3::ZwpTextInputManagerV3,
        zwp_text_input_v3::{self, ZwpTextInputV3},
    };
    use wayland_protocols::xdg::shell::client::{
        xdg_surface::{self, XdgSurface},
        xdg_toplevel::XdgToplevel,
        xdg_wm_base::{self, XdgWmBase},
    };

    /// `KEY_ENTER` from linux/input-event-codes.h, as `wl_keyboard.key` reports it.
    const KEY_ENTER: u32 = 28;

    #[derive(Debug, Default)]
    struct App {
        keys: Vec<(u32, KeyState)>,
        focused: Option<wayland_client::backend::ObjectId>,
        close_requested: bool,
        text: String,
        keyboard_text: String,
        keymap_source: String,
        pending_text: String,
        suppress_enable: bool,
        modifiers: (u32, u32, u32),
        group: u32,
    }

    impl Dispatch<WlRegistry, GlobalListContents> for App {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as wayland_client::Proxy>::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<XdgWmBase, ()> for App {
        fn event(
            _: &mut Self,
            wm_base: &XdgWmBase,
            event: xdg_wm_base::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_wm_base::Event::Ping { serial } = event {
                wm_base.pong(serial);
            }
        }
    }

    impl Dispatch<XdgSurface, ()> for App {
        fn event(
            _: &mut Self,
            surface: &XdgSurface,
            event: xdg_surface::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let xdg_surface::Event::Configure { serial } = event {
                surface.ack_configure(serial);
            }
        }
    }

    fn update_client_focus(app: &mut App, event: &wl_keyboard::Event) {
        match event {
            wl_keyboard::Event::Enter { surface, .. } => {
                app.focused = Some(wayland_client::Proxy::id(surface));
            }
            wl_keyboard::Event::Leave { .. } => app.focused = None,
            wl_keyboard::Event::Keymap { .. }
            | wl_keyboard::Event::Key { .. }
            | wl_keyboard::Event::Modifiers { .. }
            | wl_keyboard::Event::RepeatInfo { .. }
            | _ => {}
        }
    }

    impl Dispatch<WlKeyboard, ()> for App {
        fn event(
            app: &mut Self,
            _: &WlKeyboard,
            event: wl_keyboard::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            update_client_focus(app, &event);
            if let wl_keyboard::Event::Keymap { fd: descriptor, .. } = event {
                app.keymap_source.clear();
                let mut keymap_file = std::fs::File::from(descriptor);
                let _offset = keymap_file.seek(SeekFrom::Start(0)).unwrap();
                let _bytes = keymap_file.read_to_string(&mut app.keymap_source).unwrap();
                return;
            }
            if let wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } = &event
            {
                app.modifiers = (*mods_depressed, *mods_latched, *mods_locked);
                app.group = *group;
            }
            if let wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(state),
                ..
            } = event
            {
                app.keys.push((key, state));
                if state == KeyState::Pressed {
                    use smithay::input::keyboard::xkb;
                    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
                    let keymap = xkb::Keymap::new_from_string(
                        &context,
                        app.keymap_source.trim_end_matches('\0').into(),
                        xkb::KEYMAP_FORMAT_TEXT_V1,
                        xkb::KEYMAP_COMPILE_NO_FLAGS,
                    )
                    .unwrap();
                    let mut xkb_state = xkb::State::new(&keymap);
                    let _changed = xkb_state.update_mask(
                        app.modifiers.0,
                        app.modifiers.1,
                        app.modifiers.2,
                        0,
                        0,
                        app.group,
                    );
                    app.keyboard_text.push_str(
                        &xkb_state
                            .key_get_utf8(smithay::input::keyboard::Keycode::new(key.saturating_add(8))),
                    );
                }
            }
        }
    }

    impl Dispatch<ZwpTextInputV3, ()> for App {
        fn event(
            app: &mut Self,
            input: &ZwpTextInputV3,
            event: zwp_text_input_v3::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                zwp_text_input_v3::Event::Enter { .. } => {
                    if !app.suppress_enable {
                        input.enable();
                        input.commit();
                    }
                }
                zwp_text_input_v3::Event::CommitString { text } => {
                    if let Some(text) = text {
                        app.pending_text.push_str(&text);
                    }
                }
                zwp_text_input_v3::Event::Done { serial } => {
                    assert_eq!(serial, 1);
                    app.text.push_str(&core::mem::take(&mut app.pending_text));
                }
                zwp_text_input_v3::Event::Leave { .. }
                | zwp_text_input_v3::Event::PreeditString { .. }
                | zwp_text_input_v3::Event::DeleteSurroundingText { .. }
                | zwp_text_input_v3::Event::Action { .. }
                | zwp_text_input_v3::Event::Language { .. }
                | zwp_text_input_v3::Event::PreeditHint { .. } => {}
                _ => panic!("unknown text-input event"),
            }
        }
    }
    delegate_noop!(App: ignore ZwpTextInputManagerV3);

    delegate_noop!(App: ignore WlCompositor);
    delegate_noop!(App: ignore WlSurface);
    delegate_noop!(App: ignore WlSeat);
    impl Dispatch<XdgToplevel, ()> for App {
        fn event(
            app: &mut Self,
            _: &XdgToplevel,
            event: <XdgToplevel as wayland_client::Proxy>::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if matches!(
                event,
                wayland_protocols::xdg::shell::client::xdg_toplevel::Event::Close
            ) {
                app.close_requested = true;
            }
        }
    }

    /// Opens one toplevel and returns every key it receives once it has seen one full key tap.
    fn run_client(
        stream: UnixStream,
        app_id: &str,
        text_state: ClientTextState,
        ready: &mpsc::Sender<()>,
    ) -> App {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
        let handle = queue.handle();
        let compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
        let wm_base: XdgWmBase = globals.bind(&handle, 1..=1, ()).unwrap();
        let seat: WlSeat = globals.bind(&handle, 1..=1, ()).unwrap();
        let _keyboard = seat.get_keyboard(&handle, ());
        let manager: ZwpTextInputManagerV3 = globals.bind(&handle, 1..=1, ()).unwrap();
        let text_input = (!matches!(text_state, ClientTextState::Absent))
            .then(|| manager.get_text_input(&seat, &handle, ()));
        let surface = compositor.create_surface(&handle, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &handle, ());
        let toplevel = xdg_surface.get_toplevel(&handle, ());
        toplevel.set_title("Synthetic-window-title-1234567890".into());
        let initial_app_id_before_focus_change = if app_id == "test" { "other" } else { "test" };
        toplevel.set_app_id(initial_app_id_before_focus_change.into());
        surface.commit();
        let mut app = App::default();
        let _initial_dispatch = queue.roundtrip(&mut app).unwrap();
        toplevel.set_app_id(app_id.into());
        let _focus_dispatch = queue.roundtrip(&mut app).unwrap();
        let _enabled_dispatch = queue.roundtrip(&mut app).unwrap();
        if matches!(text_state, ClientTextState::Disabled) {
            text_input.as_ref().unwrap().disable();
            text_input.as_ref().unwrap().commit();
            let _disabled_dispatch = queue.roundtrip(&mut app).unwrap();
        }
        if matches!(text_state, ClientTextState::FocusChanged) {
            app.suppress_enable = true;
            let next_surface = compositor.create_surface(&handle, ());
            let next_xdg_surface = wm_base.get_xdg_surface(&next_surface, &handle, ());
            let next_toplevel = next_xdg_surface.get_toplevel(&handle, ());
            next_toplevel.set_app_id(app_id.into());
            next_surface.commit();
            let _changed_focus = queue.roundtrip(&mut app).unwrap();
        }
        ready.send(()).unwrap();
        while app.keys.len() < 2 && !app.close_requested {
            let _dispatched = queue.blocking_dispatch(&mut app).unwrap();
        }
        // Any further key the compositor sent arrives before this roundtrip completes.
        let _dispatched = queue.roundtrip(&mut app).unwrap();
        app
    }

    #[derive(Clone, Debug)]
    enum Command {
        Connect,
        Disconnect,
        Open,
        Close,
        Start,
        Crash,
        Hang(usize),
        Hold,
        Button,
        Text,
        Settle,
    }

    fn command() -> impl Strategy<Value = Command> {
        prop_oneof![
            Just(Command::Connect),
            Just(Command::Disconnect),
            Just(Command::Open),
            Just(Command::Close),
            Just(Command::Start),
            Just(Command::Crash),
            (1usize..4).prop_map(Command::Hang),
            Just(Command::Hold),
            Just(Command::Button),
            Just(Command::Text),
            Just(Command::Settle),
        ]
    }

    #[derive(Default)]
    struct ControllerReference {
        connected: bool,
        held: bool,
    }

    #[derive(Default)]
    struct Reference {
        controller: ControllerReference,
        windows: Vec<usize>,
        focus: Option<usize>,
        next_window: usize,
        taps: usize,
        misses: Vec<usize>,
        started: bool,
        backoff: Duration,
        /// The focused window as last announced to the plugin host, which only sees focus once started.
        plugin_focus: bool,
    }

    impl Reference {
        fn apply(&mut self, command: &Command) {
            match command {
                Command::Connect => self.controller.connected = true,
                Command::Disconnect => {
                    self.controller.connected = false;
                    self.controller.held = false;
                }
                Command::Open => {
                    self.windows.push(self.next_window);
                    self.focus = Some(self.next_window);
                    self.plugin_focus |= self.started;
                    self.next_window = self.next_window.saturating_add(1);
                }
                Command::Close => {
                    if let Some(closed) = self.windows.pop()
                        && self.focus == Some(closed)
                    {
                        // Closing the focused window hands focus to the most recently mapped one.
                        self.focus = self.windows.last().copied();
                        if self.started {
                            self.plugin_focus = self.focus.is_some();
                        }
                    }
                }
                Command::Button if self.controller.connected => {
                    self.taps = self
                        .taps
                        .saturating_add(usize::from(!self.controller.held && self.focus.is_some()));
                    self.controller.held = false;
                }
                Command::Hold if self.controller.connected && !self.controller.held => {
                    self.controller.held = true;
                    self.taps = self.taps.saturating_add(usize::from(self.focus.is_some()));
                }
                Command::Hang(batch) if self.focus.is_some() => self.taps = self.taps.saturating_add(*batch),
                Command::Text if self.focus.is_some() => {
                    self.taps = self.taps.saturating_add(1);
                }
                Command::Start => {
                    self.started = true;
                    if self.misses.is_empty() {
                        self.misses.push(0);
                    }
                    if self.backoff.is_zero() {
                        self.backoff = Duration::from_millis(100);
                    }
                }
                Command::Crash if self.started => {
                    if self.focus.is_some() {
                        self.taps = self.taps.saturating_add(1);
                    }
                }
                Command::Crash
                | Command::Hang(_)
                | Command::Hold
                | Command::Button
                | Command::Text
                | Command::Settle => {}
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]
        #[test]
        fn supervision_and_input_match_a_window_reference_model(
            commands in prop::collection::vec(command(), 1..20),
        ) {
            let directory = unresponsive_plugin("stalled");
            let mut display: Display<Compositor> = Display::new().unwrap();
            let mut compositor = Compositor::new(&display.handle()).unwrap();
            compositor.set_contexts(vec![tehuti_protocol::parse_context("app_id = 'test'\n[buttons.east]\nkey = 'enter'\n[speech]\nconfirm = { key = 'enter' }").unwrap()]);
            let mut event_loop = smithay::reexports::calloop::EventLoop::try_new().unwrap();
            let (server, client) = UnixStream::pair().unwrap();
            let _client = display.handle().insert_client(server, Arc::new(ClientState::default())).unwrap();
            let (send, receive) = mpsc::channel::<Command>();
            let (reports, snapshots) = mpsc::channel();
            let client = thread::spawn(move || {
                let connection = Connection::from_socket(client).unwrap();
                let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
                let handle = queue.handle();
                let wayland: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
                let shell: XdgWmBase = globals.bind(&handle, 1..=1, ()).unwrap();
                let seat: WlSeat = globals.bind(&handle, 1..=1, ()).unwrap();
                let _keyboard = seat.get_keyboard(&handle, ());
                let mut app = App::default();
                let mut windows = Vec::new();
                let mut ids = Vec::new();
                for command in receive {
                    match command {
                        Command::Open => {
                            let surface = wayland.create_surface(&handle, ());
                            ids.push(wayland_client::Proxy::id(&surface));
                            let xdg = shell.get_xdg_surface(&surface, &handle, ());
                            let toplevel = xdg.get_toplevel(&handle, ());
                            toplevel.set_app_id("test".into());
                            surface.commit();
                            windows.push((surface, xdg, toplevel));
                        }
                        Command::Close => {
                            if let Some((surface, xdg, toplevel)) = windows.pop() {
                                toplevel.destroy();
                                xdg.destroy();
                                surface.destroy();
                            }
                        }
                        Command::Connect | Command::Disconnect | Command::Start | Command::Crash | Command::Hang(_) | Command::Hold | Command::Button | Command::Text | Command::Settle => {}
                    }
                    let _roundtrip = queue.roundtrip(&mut app).unwrap();
                    let focus = app.focused.as_ref().and_then(|surface_id| ids.iter().position(|candidate| candidate == surface_id));
                    reports.send((windows.len(), focus, app.keys.clone())).unwrap();
                }
            });
            let mut model = Reference::default();
            let mut pad = None;
            let mut installed = false;
            let mut session = 0_u64;
            let commands = [Command::Connect, Command::Open, Command::Hold, Command::Disconnect, Command::Start, Command::Open, Command::Hang(3), Command::Hang(2), Command::Hang(1)]
                .into_iter().chain(commands).chain([Command::Disconnect]);
            for command in commands {
                let requests = if model.started && model.plugin_focus && model.focus.is_some() {
                    match command {
                        Command::Hang(batch) => batch,
                        Command::Button if model.controller.connected => 1,
                        Command::Disconnect if model.controller.held => 1,
                        Command::Connect | Command::Disconnect | Command::Open | Command::Close | Command::Start | Command::Crash | Command::Hold | Command::Button | Command::Text | Command::Settle => 0,
                    }
                } else { 0 };
                model.apply(&command);
                match &command {
                    Command::Connect => {
                        compositor.controller_input(tehuti_core::controller::Input::Reconnected);
                        pad = Some(SyntheticPad::default());
                    },
                    Command::Disconnect => {
                        if let Some(mut source) = pad.take() {
                            tehuti::input::InputSource::release(&mut source).unwrap();
                            prop_assert!(source.is_released());
                        }
                        compositor.controller_input(tehuti_core::controller::Input::Disconnected);
                        compositor.speech_input(tehuti_core::speech::SpeechInput::Disconnected);
                    }
                    Command::Start if !installed => {
                        compositor.set_plugins(tehuti::plugins::install(&event_loop, directory.path(), |state| state).unwrap());
                        installed = true;
                        let began = Instant::now();
                        while compositor.initialized_plugins() == 0 {
                            event_loop.dispatch(Duration::from_millis(1), &mut compositor).unwrap();
                            prop_assert!(began.elapsed() < Duration::from_secs(5));
                        }
                    }
                    Command::Crash if installed => {
                        compositor.handle(&Event::Button {
                            name: ButtonName::East, state: ButtonState::Pressed,
                        });
                        let pid = std::fs::read_to_string(directory.path().join("pid")).unwrap();
                        let status = std::process::Command::new("kill").arg(pid.trim()).status().unwrap();
                        prop_assert!(status.success());
                        let pressed = compositor.pending_plugin_decisions();
                        let began = Instant::now();
                        while compositor.initialized_plugins() != 0 {
                            event_loop.dispatch(Duration::from_millis(1), &mut compositor).unwrap();
                            compositor.handle(&Event::Tick { elapsed: Duration::ZERO });
                            prop_assert!(began.elapsed() < Duration::from_secs(5));
                        }
                        prop_assert_eq!(compositor.pending_plugin_decisions(), 0, "crash releases pending fallbacks");
                        prop_assert_eq!(pressed, usize::from(model.plugin_focus && model.focus.is_some()), "a focused press awaits the plugin");
                        let failed = Instant::now();
                        let earliest = failed.checked_add(model.backoff.saturating_sub(Duration::from_millis(20))).unwrap();
                        while compositor.initialized_plugins() == 0 {
                            event_loop.dispatch(Duration::from_millis(1), &mut compositor).unwrap();
                            prop_assert!(failed.elapsed() < model.backoff.saturating_add(Duration::from_secs(5)));
                        }
                        prop_assert!(Instant::now() >= earliest, "restart respects model backoff");
                        prop_assert_ne!(std::fs::read_to_string(directory.path().join("pid")).unwrap(), pid);
                        model.misses.fill(0);
                        model.backoff = model.backoff.saturating_mul(2).min(Duration::from_secs(30));
                    }
                    Command::Hang(batch) => {
                        for _ in 0..*batch {
                            compositor.handle(&Event::Button { name: ButtonName::East, state: ButtonState::Pressed });
                        }
                    }
                    Command::Hold => {
                        if let Some(source) = &mut pad {
                            for name in [ButtonName::South, ButtonName::Guide] {
                                source.push(Event::Button { name, state: ButtonState::Pressed });
                            }
                            route(&mut compositor, source).unwrap();
                        }
                    }
                    Command::Button => {
                        if let Some(source) = &mut pad {
                            for state in [ButtonState::Pressed, ButtonState::Released] {
                                source.push(Event::Button { name: ButtonName::South, state });
                            }
                            source.push(Event::Button { name: ButtonName::Guide, state: ButtonState::Released });
                            route(&mut compositor, source).unwrap();
                        }
                    }
                    Command::Text => {
                        use tehuti_core::speech::SpeechInput;
                        let pending = compositor.pending_plugin_decisions();
                        compositor.speech_input(SpeechInput::Available);
                        compositor.speech_input(SpeechInput::Press);
                        compositor.speech_input(SpeechInput::Release);
                        compositor.speech_input(SpeechInput::Transcript {
                            session, text: tehuti_protocol::Text::new("confirm".into()).unwrap(),
                        });
                        compositor.speech_input(SpeechInput::Transcript {
                            session, text: tehuti_protocol::Text::new("confirm".into()).unwrap(),
                        });
                        session = session.saturating_add(1);
                        prop_assert_eq!(compositor.pending_plugin_decisions(), pending, "sidecar text issues no plugin request");
                    }
                    Command::Open | Command::Close | Command::Start | Command::Crash | Command::Settle => {}
                }
                prop_assert_eq!(compositor.initialized_plugins(), usize::from(model.started));
                prop_assert_eq!(compositor.pending_plugin_decisions(), requests, "requests remain pending before expiry");
                if requests > 0 {
                    let pid = std::fs::read_to_string(directory.path().join("pid")).unwrap();
                    thread::sleep(tehuti_core::plugin::BUTTON_BUDGET.saturating_add(Duration::from_millis(10)));
                    model.misses[0] = model.misses[0].saturating_add(1);
                    compositor.handle(&Event::Tick { elapsed: tehuti_core::plugin::BUTTON_BUDGET.saturating_add(Duration::from_millis(10)) });
                    prop_assert_eq!(compositor.pending_plugin_decisions(), 0, "every decision settles");
                    if model.misses[0] == 3 {
                        prop_assert_eq!(compositor.initialized_plugins(), 0, "three expiry batches stop the plugin");
                        let failed = Instant::now();
                        while failed.elapsed() < model.backoff.saturating_sub(Duration::from_millis(20)) {
                            event_loop.dispatch(Duration::from_millis(1), &mut compositor).unwrap();
                            prop_assert_eq!(compositor.initialized_plugins(), 0, "restart waits for backoff");
                        }
                        while compositor.initialized_plugins() == 0 {
                            event_loop.dispatch(Duration::from_millis(1), &mut compositor).unwrap();
                            prop_assert!(failed.elapsed() < model.backoff.saturating_add(Duration::from_secs(5)));
                        }
                        prop_assert_ne!(std::fs::read_to_string(directory.path().join("pid")).unwrap(), pid);
                        model.misses[0] = 0;
                        model.backoff = model.backoff.saturating_mul(2).min(Duration::from_secs(30));
                    } else {
                        prop_assert_eq!(compositor.initialized_plugins(), 1, "fewer than three expiry batches keep the plugin alive");
                        prop_assert_eq!(std::fs::read_to_string(directory.path().join("pid")).unwrap(), pid);
                    }
                }
                if !model.controller.held {
                    compositor.handle(&Event::Tick { elapsed: Duration::from_secs(3) });
                    prop_assert!(!compositor.quit_requested(), "disconnect drains held Guide before its quit deadline");
                }
                send.send(command).unwrap();
                let started = Instant::now();
                let snapshot = loop {
                    let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
                    event_loop.dispatch(Duration::ZERO, &mut compositor).unwrap();
                    display.flush_clients().unwrap();
                    if let Ok(snapshot) = snapshots.try_recv() { break snapshot; }
                    prop_assert!(started.elapsed() < Duration::from_secs(5), "client progresses despite plugins");
                    thread::yield_now();
                };
                prop_assert_eq!((snapshot.0, snapshot.1, snapshot.2.len()), (model.windows.len(), model.focus, model.taps.saturating_mul(2)));
                let mut held_keys = std::collections::HashSet::new();
                for (key, state) in snapshot.2 {
                    match state {
                        KeyState::Pressed => prop_assert!(held_keys.insert(key), "duplicate press"),
                        KeyState::Released => prop_assert!(held_keys.remove(&key), "release without press"),
                        KeyState::Repeated | _ => prop_assert!(false, "unexpected key state"),
                    }
                }
                prop_assert!(held_keys.is_empty(), "every forwarded press has exactly one release");
                prop_assert!(model.focus.is_none_or(|focus| model.windows.contains(&focus)));
            }
            drop(send);
            client.join().unwrap();
        }
    }

    #[test]
    fn focus_cancels_pending_transcript_before_polling_elapsed_time() {
        use tehuti_core::speech::SpeechInput;
        let directory = tempfile::tempdir().unwrap();
        let delivered = directory.path().join("delivered");
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_speech(tehuti::speech::SpeechProcess::new(vec![
            "python3".into(), "-u".into(), "-c".into(),
            format!("import sys,json\nfrom pathlib import Path\nprint(json.dumps({{'ready':True}}),flush=True)\nfor line in sys.stdin:\n c=json.loads(line)\n if c.get('transcribe'):\n  print(json.dumps({{'session':c['stop'],'text':'stale'}}),flush=True)\n  Path({delivered:?}).touch()").into(),
        ]));
        let began = Instant::now();
        while !compositor.speech_ready() {
            compositor.advance_time();
            assert!(began.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(1));
        }
        compositor.speech_input(SpeechInput::Available);
        compositor.speech_input(SpeechInput::Press);
        compositor.speech_input(SpeechInput::Release);
        // Flushes the queued stop command; the reply cannot have been written yet.
        compositor.advance_time();
        while !delivered.exists() {
            assert!(began.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(1));
        }
        compositor.handle(&Event::Focus {
            app_id: Some("other".into()),
            title: String::new(),
        });
        assert!(!compositor.speech_notice_visible());
    }

    #[test]
    fn notices_preserve_focus_and_close_transfers_it_to_the_remaining_client() {
        let mut display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_contexts(vec![tehuti_protocol::parse_context(
            "app_id = 'test'\n[buttons]\n[speech]\nnotice = { notify = 'Ready' }\nunknown = { launch = 'tehuti-missing-entry-9.desktop' }\nclose = { close = 'focused' }"
        ).unwrap()]);
        let first = start_action_client(&mut display, &mut compositor);
        let second = start_action_client(&mut display, &mut compositor);
        for (phrase, notice) in [("notice", "Ready"), ("unknown", "APPLICATION UNAVAILABLE")] {
            compositor.handle(&Event::Speech {
                text: tehuti_protocol::Text::new(phrase.into()).unwrap(),
            });
            assert_eq!(compositor.visible_notice_text().as_deref(), Some(notice));
        }
        compositor.handle(&Event::Speech {
            text: tehuti_protocol::Text::new("close".into()).unwrap(),
        });
        compositor.handle(&Event::Button {
            name: ButtonName::South,
            state: ButtonState::Pressed,
        });
        let began = Instant::now();
        while !first.is_finished() || !second.is_finished() {
            assert!(
                began.elapsed() < Duration::from_secs(5),
                "close or focus transfer stalled"
            );
            let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
            display.flush_clients().unwrap();
            thread::sleep(Duration::from_millis(1));
        }
        let closed = second.join().unwrap();
        assert!(closed.close_requested, "the focused client received close");
        assert!(closed.keys.is_empty(), "the closed client received no later key");
        let remaining = first.join().unwrap();
        assert!(!remaining.close_requested, "the remaining client stayed open");
        assert_eq!(
            remaining.keys,
            vec![(KEY_ENTER, KeyState::Pressed), (KEY_ENTER, KeyState::Released)]
        );
    }

    fn start_action_client(
        display: &mut Display<Compositor>,
        compositor: &mut Compositor,
    ) -> thread::JoinHandle<App> {
        let (server, client) = UnixStream::pair().unwrap();
        let _inserted = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || run_client(client, "test", ClientTextState::Absent, &sender));
        let began = Instant::now();
        while receiver.try_recv().is_err() {
            assert!(began.elapsed() < Duration::from_secs(5), "client setup stalled");
            let _dispatched = display.dispatch_clients(compositor).unwrap();
            display.flush_clients().unwrap();
            thread::sleep(Duration::from_millis(1));
        }
        worker
    }

    #[test]
    fn sidecar_transcript_reaches_client() {
        let mut display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_contexts(vec![
            tehuti_protocol::parse_context("app_id = 'test'\n[buttons]\n[speech]\nconfirm = { key = 'tab' }")
                .unwrap(),
        ]);
        let process = tehuti::speech::SpeechProcess::new(vec!["python3".into(), "-u".into(), "-c".into(),
            "import sys,json\nprint(json.dumps({'ready':True}),flush=True)\nfor line in sys.stdin:\n c=json.loads(line)\n if 'stop' in c and c['transcribe']: print(json.dumps({'session':c['stop'],'text':'confirm'}),flush=True)".into()]);
        compositor.set_speech(process);
        let (server_end, client_end) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server_end, Arc::new(ClientState::default()))
            .unwrap();
        let (ready_sender, ready_receiver) = mpsc::channel();
        let client =
            thread::spawn(move || run_client(client_end, "test", ClientTextState::Absent, &ready_sender));
        let began = Instant::now();
        let mut ready = false;
        let mut spoken = false;
        let mut speech_loop = smithay::reexports::calloop::EventLoop::<Compositor>::try_new().unwrap();
        let mut pad = SyntheticPad::default();
        while !client.is_finished() {
            assert!(
                began.elapsed() < Duration::from_secs(5),
                "speech delivery stalled: client ready={ready}, sidecar ready={}, spoken={spoken}",
                compositor.speech_ready()
            );
            let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
            speech_loop.dispatch(Duration::ZERO, &mut compositor).unwrap();
            display.flush_clients().unwrap();
            ready |= ready_receiver.try_recv().is_ok();
            compositor.handle(&Event::Tick {
                elapsed: Duration::from_millis(10),
            });
            if ready && compositor.speech_ready() && !spoken {
                spoken = true;
                for state in [ButtonState::Pressed, ButtonState::Released] {
                    pad.push(Event::Button {
                        name: ButtonName::Record,
                        state,
                    });
                }
                route(&mut compositor, &mut pad).unwrap();
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            client.join().unwrap().keys,
            vec![(15, KeyState::Pressed), (15, KeyState::Released)],
            "released speech executes focused mapping once"
        );
    }

    #[test]
    fn unmatched_speech_shows_no_speech_mapping_overlay() {
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.handle(&Event::Speech {
            text: tehuti_protocol::Text::new("unknown".into()).unwrap(),
        });
        assert!(
            compositor.speech_notice_visible(),
            "unmatched speech produces a visible notice"
        );
        assert_eq!(
            compositor.visible_notice_text().as_deref(),
            Some("NO SPEECH MAPPING")
        );
    }

    #[test]
    fn changing_to_an_unmatched_app_id_uses_global_and_fallback_routing() {
        check_routing("other", KEY_ENTER);
    }

    #[test]
    fn changing_to_a_matching_app_id_sends_tab_from_a_synthetic_pad() {
        check_routing("test", 15);
    }

    #[test]
    fn guide_hold_releases_the_synthetic_pad_and_exits_the_event_loop() {
        use smithay::reexports::calloop::EventLoop;
        let display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let mut pad = SyntheticPad::default();
        pad.push(Event::Button {
            name: ButtonName::Guide,
            state: ButtonState::Pressed,
        });
        pad.push(Event::Tick {
            elapsed: Duration::from_millis(2999),
        });
        route(&mut compositor, &mut pad).unwrap();
        assert!(!compositor.quit_requested());
        assert!(!pad.is_released());

        let mut event_loop: EventLoop<()> = EventLoop::try_new().unwrap();
        let signal = event_loop.get_signal();
        pad.push(Event::Tick {
            elapsed: Duration::from_millis(1),
        });
        let mut iterations = 0;
        event_loop
            .run(Some(Duration::ZERO), &mut (), |()| {
                iterations += 1;
                assert_eq!(iterations, 1, "the quit action stops the loop");
                route(&mut compositor, &mut pad).unwrap();
                tehuti::input::finish_quit(&compositor, &mut pad, &signal).unwrap();
            })
            .unwrap();
        assert!(pad.is_released(), "the pad is released before the loop exits");
        assert!(compositor.quit_requested());
    }

    fn load_test_context(compositor: &mut Compositor, text: Option<&str>) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "tehuti-wayland-context-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("context.toml"),
            text.map_or_else(
                || "app_id = 'test'\n[buttons.east]\nkey = 'tab'".into(),
                |text| {
                    format!(
                        "app_id = 'test'\n[buttons.east]\ntext = {}",
                        toml::Value::String(text.into())
                    )
                },
            ),
        )
        .unwrap();
        compositor.set_contexts(tehuti::contexts::load(&directory));
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn check_routing(app_id: &'static str, expected_key: u32) {
        let app = exchange(app_id, None);
        assert_eq!(
            app.keys,
            if expected_key == KEY_ENTER {
                vec![(KEY_ENTER, KeyState::Pressed), (KEY_ENTER, KeyState::Released)]
            } else {
                vec![
                    (expected_key, KeyState::Pressed),
                    (expected_key, KeyState::Released),
                    (KEY_ENTER, KeyState::Pressed),
                    (KEY_ENTER, KeyState::Released),
                ]
            }
        );
    }

    fn exchange(app_id: &'static str, text: Option<&str>) -> App {
        exchange_with_enablement(app_id, text, true)
    }

    fn exchange_with_enablement(app_id: &'static str, text: Option<&str>, enabled: bool) -> App {
        exchange_with_text_state(
            app_id,
            text,
            if enabled {
                ClientTextState::Enabled
            } else {
                ClientTextState::Disabled
            },
        )
    }

    #[derive(Clone, Copy)]
    enum ClientTextState {
        Enabled,
        Absent,
        Disabled,
        FocusChanged,
    }

    fn exchange_with_text_state(
        app_id: &'static str,
        text: Option<&str>,
        text_state: ClientTextState,
    ) -> App {
        exchange_with_plugin(app_id, text, text_state, false)
    }

    #[test]
    fn documented_python_plugin_sends_tab_to_a_real_wayland_client() {
        let app = exchange_with_plugin("test", None, ClientTextState::Absent, true);
        assert_eq!(
            app.keys,
            vec![(15, KeyState::Pressed), (15, KeyState::Released)],
            "the child process remaps East through SyntheticPad and input::route"
        );
    }

    #[test]
    fn plugins_that_never_read_stall_or_flood_cannot_block_input_or_escape() {
        for mode in ["never_read", "stalled", "flood", "overlong_flood"] {
            let (sender, receiver) = mpsc::channel();
            let worker = thread::spawn(move || {
                exercise_unresponsive_plugin(mode);
                sender.send(()).unwrap();
            });
            receiver
                .recv_timeout(Duration::from_secs(12))
                .unwrap_or_else(|error| panic!("{mode} blocked compositor input or escape: {error}"));
            worker.join().unwrap();
        }
    }

    #[test]
    fn a_hanging_selected_plugin_releases_context_input_within_the_button_latency_budget() {
        check_hanging_plugin_deadline(false);
    }

    #[test]
    fn a_focus_notification_releases_overdue_plugin_fallback_once() {
        check_hanging_plugin_deadline(true);
    }

    fn check_hanging_plugin_deadline(change_focus: bool) {
        let directory = unresponsive_plugin("stalled");
        let mut display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        load_test_context(&mut compositor, None);
        let mut event_loop = smithay::reexports::calloop::EventLoop::try_new().unwrap();
        compositor
            .set_plugins(tehuti::plugins::install(&event_loop, directory.path(), |state| state).unwrap());
        let (server_end, client_end) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server_end, Arc::new(ClientState::default()))
            .unwrap();
        let (sender, receiver) = mpsc::channel();
        let client = thread::spawn(move || run_client(client_end, "test", ClientTextState::Absent, &sender));
        wait_for_focused_plugin(&mut display, &mut compositor, &mut event_loop, &receiver);
        thread::sleep(Duration::from_millis(150));
        let started = Instant::now();
        let mut pad = SyntheticPad::default();
        for state in [ButtonState::Pressed, ButtonState::Released] {
            pad.push(Event::Button {
                name: ButtonName::East,
                state,
            });
        }
        route(&mut compositor, &mut pad).unwrap();
        if change_focus {
            change_focus_after_deadline(&mut compositor, &mut pad);
        }
        while !client.is_finished() {
            assert!(
                started.elapsed() < tehuti_core::plugin::BUTTON_LATENCY_BUDGET,
                "selected hanging plugin exceeded the client delivery budget"
            );
            compositor.advance_time();
            let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
            event_loop
                .dispatch(Duration::from_millis(1), &mut compositor)
                .unwrap();
            display.flush_clients().unwrap();
        }
        let delivered = client.join().unwrap();
        assert!(started.elapsed() < tehuti_core::plugin::BUTTON_LATENCY_BUDGET);
        assert!(started.elapsed() >= tehuti_core::plugin::BUTTON_BUDGET);
        let mut expected_keys = vec![(15, KeyState::Pressed), (15, KeyState::Released)];
        if change_focus {
            expected_keys.extend([(KEY_ENTER, KeyState::Pressed), (KEY_ENTER, KeyState::Released)]);
        }
        assert_eq!(delivered.keys, expected_keys);
    }

    fn change_focus_after_deadline(compositor: &mut Compositor, pad: &mut SyntheticPad) {
        thread::sleep(Duration::from_millis(150));
        compositor.handle(&Event::Focus {
            app_id: Some("other".into()),
            title: String::new(),
        });
        pad.push(Event::Button {
            name: ButtonName::South,
            state: ButtonState::Pressed,
        });
        pad.push(Event::Button {
            name: ButtonName::South,
            state: ButtonState::Released,
        });
        route(compositor, pad).unwrap();
    }

    fn wait_for_focused_plugin(
        display: &mut Display<Compositor>,
        compositor: &mut Compositor,
        event_loop: &mut smithay::reexports::calloop::EventLoop<'static, Compositor>,
        receiver: &mpsc::Receiver<()>,
    ) {
        let setup_started = Instant::now();
        let mut focused = false;
        while !focused || compositor.initialized_plugins() != 1 {
            assert!(setup_started.elapsed() < Duration::from_secs(5));
            let _dispatched = display.dispatch_clients(compositor).unwrap();
            event_loop.dispatch(Duration::from_millis(1), compositor).unwrap();
            display.flush_clients().unwrap();
            focused |= receiver.try_recv().is_ok();
        }
    }

    fn unresponsive_plugin(mode: &str) -> tempfile::TempDir {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let script = format!(
            r"#!/usr/bin/env python3
import json, os, sys, time
from pathlib import Path
Path(__file__).with_name('pid').write_text(str(os.getpid()))
if {mode:?} != 'never_read':
    request = json.loads(sys.stdin.readline())
    print(json.dumps({{'jsonrpc': '2.0', 'id': request['id'], 'result': {{'protocol_version': 0, 'encoding': 'json', 'events': ['button', 'focus']}}}}), flush=True)
Path(__file__).with_name('ready').touch()
if {mode:?} in ('never_read', 'stalled'):
    while True:
        time.sleep(60)
else:
    while not Path(__file__).with_name('flood').exists():
        time.sleep(0.001)
    line = b'x' * (70000 if {mode:?} == 'overlong_flood' else 100) + b'\n'
    while True:
        os.write(1, line)
"
        );
        let executable = directory.path().join("plugin.py");
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(directory.path().join("plugin.json"),
            r#"{"executable":"plugin.py","app_ids":["test"],"events":["button","focus"],"start":"always","permissions":[]}"#).unwrap();
        directory
    }

    fn exercise_unresponsive_plugin(mode: &str) {
        use smithay::reexports::calloop::EventLoop;
        let directory = unresponsive_plugin(mode);
        let mut display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_contexts(vec![
            tehuti_protocol::parse_context("app_id = 'other'\n[buttons.east]\nkey = 'tab'").unwrap(),
            tehuti_protocol::parse_context("app_id = 'test'\n[buttons.east]\nkey = 'escape'").unwrap(),
        ]);
        let mut event_loop = EventLoop::try_new().unwrap();
        compositor
            .set_plugins(tehuti::plugins::install(&event_loop, directory.path(), |state| state).unwrap());
        let (server_end, client_end) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server_end, Arc::new(ClientState::default()))
            .unwrap();
        let (ready_sender, ready_receiver) = mpsc::channel();
        let client =
            thread::spawn(move || run_client(client_end, "test", ClientTextState::Absent, &ready_sender));
        let started = Instant::now();
        let mut client_ready = false;
        loop {
            assert!(started.elapsed() < Duration::from_secs(5), "{mode} setup stalled");
            let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
            event_loop
                .dispatch(Duration::from_millis(1), &mut compositor)
                .unwrap();
            display.flush_clients().unwrap();
            client_ready |= ready_receiver.try_recv().is_ok();
            if client_ready
                && directory.path().join("ready").exists()
                && (mode == "never_read" || compositor.initialized_plugins() == 1)
            {
                break;
            }
        }

        std::fs::write(directory.path().join("flood"), "").unwrap();
        let mut pad = SyntheticPad::default();
        let delivery_started = overload_then_send_keys(mode, &mut compositor, &mut event_loop, &mut pad);
        while !client.is_finished() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "{mode} blocked key delivery"
            );
            let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
            event_loop
                .dispatch(Duration::from_millis(1), &mut compositor)
                .unwrap();
            display.flush_clients().unwrap();
        }
        let delivered = client.join().unwrap();
        assert_unresponsive_delivery(&delivered, delivery_started.elapsed(), mode);
        assert_escape_deadline(mode, &mut compositor, &mut event_loop, &mut pad);
    }

    fn assert_unresponsive_delivery(delivered: &App, elapsed: Duration, mode: &str) {
        assert!(
            elapsed < tehuti_core::plugin::BUTTON_LATENCY_BUDGET,
            "client delivery took {elapsed:?}, exceeding the button latency budget during {mode}"
        );
        assert_eq!(
            delivered.keys,
            vec![
                (KEY_ENTER, KeyState::Pressed),
                (KEY_ENTER, KeyState::Released),
                (15, KeyState::Pressed),
                (15, KeyState::Released),
            ],
            "global and context layers receive input during {mode}"
        );
    }

    fn overload_then_send_keys(
        mode: &str,
        compositor: &mut Compositor,
        event_loop: &mut smithay::reexports::calloop::EventLoop<'static, Compositor>,
        pad: &mut SyntheticPad,
    ) -> Instant {
        for _ in 0..2048 {
            pad.push(Event::Focus {
                app_id: Some("test".into()),
                title: "x".repeat(128),
            });
            if mode != "never_read" {
                pad.push(Event::Button {
                    name: ButtonName::East,
                    state: ButtonState::Released,
                });
            }
        }
        route(compositor, pad).unwrap();
        event_loop.dispatch(Duration::ZERO, compositor).unwrap();
        let delivery_started = Instant::now();
        for name in [ButtonName::South, ButtonName::East] {
            if name == ButtonName::East {
                pad.push(Event::Focus {
                    app_id: Some("other".into()),
                    title: String::new(),
                });
            }
            for state in [ButtonState::Pressed, ButtonState::Released] {
                pad.push(Event::Button { name, state });
            }
        }
        route(compositor, pad).unwrap();
        delivery_started
    }

    fn assert_escape_deadline(
        mode: &str,
        compositor: &mut Compositor,
        event_loop: &mut smithay::reexports::calloop::EventLoop<'static, Compositor>,
        pad: &mut SyntheticPad,
    ) {
        pad.push(Event::Focus {
            app_id: Some("test".into()),
            title: String::new(),
        });
        pad.push(Event::Button {
            name: ButtonName::Guide,
            state: ButtonState::Pressed,
        });
        route(compositor, pad).unwrap();
        let escape_started = Instant::now();
        let mut previous_tick = escape_started;
        let signal = event_loop.get_signal();
        event_loop
            .run(Some(Duration::from_millis(5)), compositor, |compositor| {
                let now = Instant::now();
                assert!(
                    escape_started.elapsed() < Duration::from_secs(4),
                    "{mode} missed escape deadline"
                );
                pad.push(Event::Tick {
                    elapsed: now.duration_since(previous_tick),
                });
                previous_tick = now;
                route(compositor, pad).unwrap();
                tehuti::input::finish_quit(compositor, pad, &signal).unwrap();
            })
            .unwrap();
        assert!(compositor.quit_requested());
        assert!(pad.is_released());
        assert!(escape_started.elapsed() < Duration::from_secs(4));
    }

    fn exchange_with_plugin(
        app_id: &'static str,
        text: Option<&str>,
        text_state: ClientTextState,
        plugin: bool,
    ) -> App {
        let mut display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        let mut event_loop = smithay::reexports::calloop::EventLoop::<Compositor>::try_new().unwrap();
        if plugin {
            let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins");
            compositor.set_plugins(tehuti::plugins::install(&event_loop, &directory, |state| state).unwrap());
        }
        load_test_context(&mut compositor, text);
        if plugin {
            compositor.set_contexts(vec![
                tehuti_protocol::parse_context("app_id = 'test'\n[buttons.east]\nkey = 'escape'").unwrap(),
            ]);
        }
        let (server_end, client_end) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server_end, Arc::new(ClientState::default()))
            .unwrap();
        let (ready_sender, ready_receiver) = mpsc::channel();
        let client = thread::spawn(move || run_client(client_end, app_id, text_state, &ready_sender));

        let mut pad = SyntheticPad::default();
        for name in if plugin {
            vec![ButtonName::East]
        } else {
            vec![ButtonName::East, ButtonName::Start, ButtonName::South]
        } {
            pad.push(Event::Button {
                name,
                state: ButtonState::Pressed,
            });
            pad.push(Event::Button {
                name,
                state: ButtonState::Released,
            });
        }
        let mut pressed = false;
        let mut client_ready = false;
        let started = Instant::now();
        while !client.is_finished() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "client never received a key"
            );
            let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
            event_loop.dispatch(Duration::ZERO, &mut compositor).unwrap();
            client_ready |= ready_receiver.try_recv().is_ok();
            if !pressed && client_ready && (!plugin || compositor.initialized_plugins() == 1) {
                route(&mut compositor, &mut pad).unwrap();
                pressed = true;
            }
            display.flush_clients().unwrap();
            thread::sleep(Duration::from_millis(5));
        }

        client.join().unwrap()
    }

    proptest! {
        #[test]
        fn unicode_text_arrives_in_order_without_stuck_keys_or_modifiers(
            characters in prop::collection::vec(any::<char>().prop_filter("Wayland strings exclude NUL", |character| *character != '\0'), 0..1500),
        ) {
            let text: String = characters.into_iter().collect();
            assert_text(&text);
        }
    }

    proptest! {
        #[test]
        fn keymap_text_arrives_in_order_without_stuck_keys_or_modifiers(
            characters in prop::collection::vec(
                prop_oneof![Just('\n'), (0x20u8..=0x7e).prop_map(char::from)],
                0..80,
            ),
        ) {
            let text: String = characters.into_iter().collect();
            let app = exchange_with_text_state("test", Some(&text), ClientTextState::Absent);
            prop_assert_eq!(app.text, "");
            prop_assert_eq!(app.keyboard_text.replace('\r', "\n"), format!("{text}\n"));
            let mut pressed = std::collections::HashSet::new();
            for (key, state) in app.keys {
                match state {
                    KeyState::Pressed => prop_assert!(pressed.insert(key), "key already pressed: {key}"),
                    KeyState::Released => prop_assert!(pressed.remove(&key), "key released without press: {key}"),
                    KeyState::Repeated | _ => prop_assert!(false, "unexpected key state"),
                }
            }
            prop_assert!(pressed.is_empty());
            prop_assert_eq!(app.modifiers, (0, 0, 0));
            prop_assert_eq!(app.group, 0);
        }
    }

    #[test]
    fn keymap_newline_is_an_enter_tap() {
        let app = exchange_with_text_state("test", Some("\n"), ClientTextState::Absent);
        assert_eq!(app.keyboard_text, "\r\r");
        assert_eq!(
            app.keys,
            vec![
                (KEY_ENTER, KeyState::Pressed),
                (KEY_ENTER, KeyState::Released),
                (KEY_ENTER, KeyState::Pressed),
                (KEY_ENTER, KeyState::Released),
            ]
        );
        assert_eq!(app.modifiers, (0, 0, 0));
    }

    fn assert_text(text: &str) {
        let app = exchange("test", Some(text));
        assert_eq!(app.text, text);
        assert_eq!(app.pending_text, "");
        assert_eq!(
            app.keys,
            vec![(KEY_ENTER, KeyState::Pressed), (KEY_ENTER, KeyState::Released)]
        );
        assert_eq!(app.modifiers, (0, 0, 0));
    }

    #[test]
    fn clients_without_enabled_text_input_receive_ascii_key_taps() {
        for text_state in [
            ClientTextState::Absent,
            ClientTextState::Disabled,
            ClientTextState::FocusChanged,
        ] {
            let app = exchange_with_text_state("test", Some("aA!"), text_state);
            assert_eq!(app.text, "");
            assert_eq!(app.keyboard_text, "aA!\r");
            assert_eq!(
                app.keys,
                vec![
                    (30, KeyState::Pressed),
                    (30, KeyState::Released),
                    (30, KeyState::Pressed),
                    (30, KeyState::Released),
                    (2, KeyState::Pressed),
                    (2, KeyState::Released),
                    (KEY_ENTER, KeyState::Pressed),
                    (KEY_ENTER, KeyState::Released),
                ]
            );
            assert_eq!(app.modifiers, (0, 0, 0));
        }
    }

    #[derive(Clone)]
    struct LogWriter(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for LogWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn unavailable_character_is_logged_and_skipped_without_stuck_keys() {
        let logs = Arc::new(std::sync::Mutex::new(Vec::new()));
        let writer = LogWriter(logs.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish();
        let app = tracing::subscriber::with_default(subscriber, || {
            exchange_with_text_state("test", Some("a🦀b"), ClientTextState::Absent)
        });
        assert_eq!(app.text, "");
        assert_eq!(
            app.keys,
            vec![
                (30, KeyState::Pressed),
                (30, KeyState::Released),
                (48, KeyState::Pressed),
                (48, KeyState::Released),
                (KEY_ENTER, KeyState::Pressed),
                (KEY_ENTER, KeyState::Released),
            ]
        );
        assert_eq!(app.modifiers, (0, 0, 0));
        let output = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        assert!(
            output.contains("WARN")
                && output.contains("test")
                && output.contains('🦀')
                && output.contains("skipped"),
            "{output}"
        );
    }

    #[test]
    fn text_without_a_focused_client_is_logged() {
        let logs = Arc::new(std::sync::Mutex::new(Vec::new()));
        let writer = LogWriter(logs.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let display: Display<Compositor> = Display::new().unwrap();
            let mut compositor = Compositor::new(&display.handle()).unwrap();
            load_test_context(&mut compositor, Some("hello"));
            compositor.handle(&Event::Focus {
                app_id: Some("test".into()),
                title: String::new(),
            });
            compositor.handle(&Event::Button {
                name: ButtonName::East,
                state: ButtonState::Pressed,
            });
        });
        let output = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        assert!(
            output.contains("WARN") && output.contains("no focused client"),
            "{output}"
        );
    }

    #[test]
    fn unmatched_client_receives_only_the_global_key() {
        let app = exchange("other", Some("hello"));
        assert_eq!(app.text, "");
        assert_eq!(
            app.keys,
            vec![(KEY_ENTER, KeyState::Pressed), (KEY_ENTER, KeyState::Released)]
        );
    }

    #[derive(Debug)]
    struct FakeSecretProvider;

    impl tehuti_core::secret::SecretProvider for FakeSecretProvider {
        fn lookup(
            &mut self,
            _reference: &tehuti_protocol::SecretReference,
        ) -> Result<tehuti_core::secret::SecretValue, tehuti_core::secret::SecretProviderUnavailable>
        {
            Ok(tehuti_core::secret::SecretValue::new("s3ntinel".into()))
        }
    }

    fn dispatch_until_ready(
        display: &mut Display<Compositor>,
        event_loop: &mut smithay::reexports::calloop::EventLoop<'static, Compositor>,
        compositor: &mut Compositor,
        ready: &mpsc::Receiver<()>,
        started: Instant,
    ) {
        while ready.try_recv().is_err() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "synthetic client did not focus"
            );
            let _dispatched = display.dispatch_clients(compositor).unwrap();
            event_loop.dispatch(Duration::from_millis(1), compositor).unwrap();
            display.flush_clients().unwrap();
        }
    }

    fn dispatch_until_finished(
        display: &mut Display<Compositor>,
        event_loop: &mut smithay::reexports::calloop::EventLoop<'static, Compositor>,
        compositor: &mut Compositor,
        client: &thread::JoinHandle<App>,
        started: Instant,
    ) {
        while !client.is_finished() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "secret delivery stalled"
            );
            compositor.handle(&Event::Tick {
                elapsed: Duration::from_millis(1),
            });
            let _dispatched = display.dispatch_clients(compositor).unwrap();
            event_loop.dispatch(Duration::from_millis(1), compositor).unwrap();
            display.flush_clients().unwrap();
        }
    }

    fn assert_secret_confirmation_label(compositor: &Compositor) {
        assert_eq!(
            compositor.secret_confirmation_text().as_deref(),
            Some("CONFIRM SECRET: password-store/exampl...\nTARGET: Synthetic-window-title-123456...")
        );
        assert!(
            compositor
                .secret_confirmation_text()
                .unwrap()
                .lines()
                .all(|line| line.chars().count() <= 40),
            "both confirmation labels fit the rendered notice lines"
        );
    }

    proptest! {
        #[test]
        fn secret_store_values_stay_out_of_protocol_and_debug_records(suffix in "[A-Za-z0-9]{1,64}") {
            let value = format!("SENTINEL-UNIQUE-{suffix}");
            let secret = tehuti_core::secret::SecretValue::new(value.clone());
            let reference = tehuti_protocol::SecretReference { entry: "mail".into() };
            let action = tehuti_protocol::Action::SecretRequest { reference };
            let event = Event::Button { name: ButtonName::East, state: ButtonState::Pressed };
            let plugin_reply = tehuti_protocol::Message::Response(tehuti_protocol::Response {
                jsonrpc: tehuti_protocol::RpcVersion::Two,
                request_id: 1,
                result: tehuti_protocol::ResultValue::Actions(vec![action.clone()]),
            });
            let logs = Arc::new(std::sync::Mutex::new(Vec::new()));
            let writer = LogWriter(logs.clone());
            let subscriber = tracing_subscriber::fmt()
                .with_ansi(false)
                .without_time()
                .with_writer(move || writer.clone())
                .finish();
            tracing::subscriber::with_default(subscriber, || {
                tracing::info!(secret = ?secret, "fake provider lookup");
            });
            let records = [
                serde_json::to_string(&event).unwrap(),
                serde_json::to_string(&action).unwrap(),
                serde_json::to_string(&plugin_reply).unwrap(),
                format!("{secret:?}"),
                String::from_utf8(logs.lock().unwrap().clone()).unwrap(),
            ];
            for record in records {
                prop_assert!(!record.contains(&value), "secret value escaped: {record}");
            }
        }
    }

    #[test]
    fn controller_confirmation_delivers_fake_secret_to_the_focused_wayland_client() {
        let mut display: Display<Compositor> = Display::new().unwrap();
        let mut compositor = Compositor::new(&display.handle()).unwrap();
        compositor.set_secret_provider(Box::new(FakeSecretProvider));
        compositor.set_contexts(vec![
            tehuti_protocol::parse_context(
                "app_id = 'test'\n[buttons.east]\nsecret = { entry = 'password-store/example-production-login' }",
            )
            .unwrap(),
        ]);
        let mut event_loop = smithay::reexports::calloop::EventLoop::<Compositor>::try_new().unwrap();
        let (server_end, client_end) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server_end, Arc::new(ClientState::default()))
            .unwrap();
        let (ready_sender, ready_receiver) = mpsc::channel();
        let client =
            thread::spawn(move || run_client(client_end, "test", ClientTextState::Enabled, &ready_sender));
        let started = Instant::now();
        dispatch_until_ready(
            &mut display,
            &mut event_loop,
            &mut compositor,
            &ready_receiver,
            started,
        );

        let mut pad = SyntheticPad::default();
        for state in [ButtonState::Pressed, ButtonState::Released] {
            pad.push(Event::Button {
                name: ButtonName::East,
                state,
            });
        }
        route(&mut compositor, &mut pad).unwrap();
        assert_secret_confirmation_label(&compositor);
        compositor.note_secret_confirmation_frame_submitted();
        for state in [ButtonState::Pressed, ButtonState::Released] {
            pad.push(Event::Button {
                name: ButtonName::South,
                state,
            });
        }
        route(&mut compositor, &mut pad).unwrap();

        dispatch_until_finished(&mut display, &mut event_loop, &mut compositor, &client, started);
        let app = client.join().unwrap();
        assert_eq!(app.keyboard_text, "s3ntinel");
        assert_eq!(
            app.keys.len(),
            16,
            "one press and release per character, with no Enter"
        );
        assert_eq!(app.text, "", "secret delivery bypasses text-input instances");
    }

    #[test]
    fn context_text_delivers_characters_outside_the_keymap() {
        assert_text("héllo ✓");
        assert_text(&"🦀é".repeat(2000));
    }
}
