use core::time::Duration;
use std::sync::mpsc;
use std::{
    io::Write,
    os::{fd::AsFd, unix::net::UnixStream},
    sync::Arc,
    thread,
    time::Instant,
};

use smithay::reexports::wayland_server::Display;
use tehuti::state::{ClientState, Compositor};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_buffer::WlBuffer,
        wl_compositor::WlCompositor,
        wl_output::WlOutput,
        wl_registry::WlRegistry,
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};

#[derive(Default)]
struct App {
    configures: Vec<(i32, i32, Vec<u32>)>,
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
impl Dispatch<XdgToplevel, ()> for App {
    fn event(
        app: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure {
            width,
            height,
            states,
        } = event
        {
            let states = states
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| u32::from_ne_bytes(*bytes))
                .collect();
            app.configures.push((width, height, states));
        }
    }
}
delegate_noop!(App: ignore WlSurface);
delegate_noop!(App: ignore WlCompositor);
delegate_noop!(App: ignore WlOutput);
delegate_noop!(App: ignore WlShm);
delegate_noop!(App: ignore WlShmPool);
delegate_noop!(App: ignore WlBuffer);

/// Draws a buffer of the given size.
fn draw(shm: &WlShm, surface: &WlSurface, handle: &QueueHandle<App>, size: (i32, i32), colour: [u8; 4]) {
    let (width, height) = (size.0.max(1), size.1.max(1));
    let bytes = width.saturating_mul(height).saturating_mul(4);
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(&colour.repeat(usize::try_from(bytes / 4).unwrap()))
        .unwrap();
    let pool = shm.create_pool(file.as_fd(), bytes, handle, ());
    let buffer = pool.create_buffer(
        0,
        width,
        height,
        width.saturating_mul(4),
        wl_shm::Format::Argb8888,
        handle,
        (),
    );
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, width, height);
    surface.commit();
}

#[derive(Debug)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "fixture is shared within this integration test"
)]
pub(super) struct Client {
    commands: mpsc::Sender<[u8; 4]>,
    ready: mpsc::Receiver<()>,
    thread: thread::JoinHandle<()>,
}

impl Client {
    pub(super) fn new(display: &Display<Compositor>, fullscreen: bool) -> Self {
        let (server, stream) = UnixStream::pair().unwrap();
        let _client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let (commands, receive) = mpsc::channel();
        let (send, ready) = mpsc::channel();
        let thread = thread::spawn(move || {
            let connection = Connection::from_socket(stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<App>(&connection).unwrap();
            let handle = queue.handle();
            let compositor: WlCompositor = globals.bind(&handle, 1..=4, ()).unwrap();
            let wm_base: XdgWmBase = globals.bind(&handle, 2..=6, ()).unwrap();
            let shm: WlShm = globals.bind(&handle, 1..=1, ()).unwrap();
            let surface = compositor.create_surface(&handle, ());
            let xdg = wm_base.get_xdg_surface(&surface, &handle, ());
            let toplevel = xdg.get_toplevel(&handle, ());
            toplevel.set_app_id("org.example.Game".into());
            toplevel.set_title("Recording test".into());
            if fullscreen {
                toplevel.set_fullscreen(None);
            }
            surface.commit();
            let mut app = App::default();
            while !app.acked {
                let _events = queue.roundtrip(&mut app).unwrap();
            }
            assert_eq!(
                app.configures
                    .last()
                    .unwrap()
                    .2
                    .contains(&u32::from(xdg_toplevel::State::Fullscreen)),
                fullscreen,
                "the test client must be configured in its requested mode"
            );
            for colour in receive {
                let &(width, height, _) = app.configures.last().unwrap();
                draw(&shm, &surface, &handle, (width, height), colour);
                let _events = queue.roundtrip(&mut app).unwrap();
                send.send(()).unwrap();
            }
        });
        Self {
            commands,
            ready,
            thread,
        }
    }

    pub(super) fn paint(
        &self,
        display: &mut Display<Compositor>,
        compositor: &mut Compositor,
        colour: [u8; 4],
    ) {
        self.commands.send(colour).unwrap();
        let started = Instant::now();
        loop {
            let _events = display.dispatch_clients(compositor).unwrap();
            display.flush_clients().unwrap();
            if self.ready.try_recv().is_ok() {
                compositor.space.refresh();
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "client timed out");
            thread::sleep(Duration::from_millis(1));
        }
    }

    pub(super) fn finish(self) {
        drop(self.commands);
        self.thread.join().unwrap();
    }
}
