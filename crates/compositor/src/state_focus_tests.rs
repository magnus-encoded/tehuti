//! Keyboard focus changes never re-enter smithay's keyboard lock (#445).

use core::time::Duration;
use std::{
    io::Write as _,
    os::{fd::AsFd as _, unix::net::UnixStream},
    sync::{Arc, mpsc},
    thread,
    time::Instant,
};

use smithay::{
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::wayland_server::Display,
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_buffer::WlBuffer,
        wl_compositor::WlCompositor,
        wl_registry::WlRegistry,
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::XdgToplevel,
    xdg_wm_base::{self, XdgWmBase},
};

use super::{ClientState, Compositor};

#[derive(Default)]
struct App {
    acked: bool,
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
        app: &mut Self,
        surface: &XdgSurface,
        event: xdg_surface::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            app.acked = true;
        }
    }
}
delegate_noop!(App: ignore XdgToplevel);
delegate_noop!(App: ignore WlSurface);
delegate_noop!(App: ignore WlCompositor);
delegate_noop!(App: ignore WlShm);
delegate_noop!(App: ignore WlShmPool);
delegate_noop!(App: ignore WlBuffer);

/// Maps one window, then keeps the connection alive until told to stop.
fn client(stream: UnixStream, stop: &mpsc::Receiver<()>) {
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
    let handle = queue.handle();
    let compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
    let wm_base: XdgWmBase = globals.bind(&handle, 1..=1, ()).unwrap();
    let shm: WlShm = globals.bind(&handle, 1..=1, ()).unwrap();
    let mut app = App::default();
    let surface = compositor.create_surface(&handle, ());
    let xdg = wm_base.get_xdg_surface(&surface, &handle, ());
    let toplevel = xdg.get_toplevel(&handle, ());
    toplevel.set_app_id("org.example.Window".into());
    toplevel.set_title("Keyboard relayout".into());
    surface.commit();
    let mut drawn = false;
    while stop.try_recv().is_err() {
        if app.acked && !drawn {
            let (width, height, stride, bytes) = (64, 64, 256, 16_384);
            let mut file = tempfile::tempfile().unwrap();
            file.write_all(&vec![255; usize::try_from(bytes).unwrap()])
                .unwrap();
            let pool = shm.create_pool(file.as_fd(), bytes, &handle, ());
            let buffer = pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, &handle, ());
            surface.attach(Some(&buffer), 0, 0);
            surface.damage_buffer(0, 0, width, height);
            surface.commit();
            drawn = true;
        }
        if queue.roundtrip(&mut app).is_err() {
            return;
        }
    }
}

fn attach_output(display: &Display<Compositor>, compositor: &mut Compositor) {
    let output = Output::new(
        "test".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Example".into(),
            model: "Test".into(),
        },
    );
    let _global = output.create_global::<Compositor>(&display.handle());
    crate::scale::apply(
        &output,
        Mode {
            size: (1920, 1080).into(),
            refresh: 60_000,
        },
        1,
    );
    compositor.space.map_output(&output, (0, 0));
    compositor.output = Some(output);
    compositor.fill_output();
}

/// Runs the compositor until a window maps and takes focus; reports whether it got there.
fn map_window_while_focus_change_relayouts(built: &mpsc::Sender<()>) -> bool {
    let mut display = Display::<Compositor>::new().unwrap();
    let mut compositor = Compositor::new(&display.handle()).unwrap();
    built.send(()).unwrap();
    attach_output(&display, &mut compositor);
    // The OSK is open over a terminal field, so losing that field on a focus
    // change shrinks the keyboard and relayouts, as in the #445 freeze.
    compositor.handle(&tehuti_protocol::Event::Speech {
        text: tehuti_protocol::Text::new("keyboard".into()).unwrap(),
    });
    let _rejection = compositor.dispatch(super::DispatchInput::TextField {
        active: true,
        changed: true,
        kind: tehuti_core::keyboard::FieldKind::Terminal,
    });
    assert!(compositor.dispatch.keyboard().is_open());
    let reserved = compositor.keyboard_reserved;
    let (server, stream) = UnixStream::pair().unwrap();
    let _client = display
        .handle()
        .insert_client(server, Arc::new(ClientState::default()))
        .unwrap();
    let (stop, stopped) = mpsc::channel();
    let thread = thread::spawn(move || client(stream, &stopped));
    let started = Instant::now();
    let mut focused = false;
    while !focused && started.elapsed() < Duration::from_secs(5) {
        let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
        compositor.space.refresh();
        display.flush_clients().unwrap();
        focused = compositor.keyboard.current_focus().is_some();
        thread::sleep(Duration::from_millis(1));
    }
    assert_ne!(
        compositor.keyboard_reserved, reserved,
        "the focus change never resized the keyboard"
    );
    stop.send(()).unwrap();
    while !thread.is_finished() {
        let _dispatched = display.dispatch_clients(&mut compositor).unwrap();
        display.flush_clients().unwrap();
        thread::sleep(Duration::from_millis(1));
    }
    thread.join().unwrap();
    focused
}

#[test]
fn mapping_a_window_while_focus_change_relayouts_returns() {
    let (done, finished) = mpsc::channel();
    let (built, constructed) = mpsc::channel();
    let _worker = thread::spawn(move || done.send(map_window_while_focus_change_relayouts(&built)));
    // Construction loads wallpapers and can be slow; only the mapping is timed.
    constructed.recv().expect("compositor construction failed");
    let focused = finished
        .recv_timeout(Duration::from_secs(20))
        .expect("compositor deadlocked in focus_changed");
    assert!(focused, "the mapped window never took keyboard focus");
}
