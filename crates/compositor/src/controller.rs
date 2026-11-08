//! The controller edge: finds the pad, grabs it, and parses its events into protocol events.

use std::{
    io,
    os::fd::{AsFd, BorrowedFd},
};

use evdev::{AbsoluteAxisCode, Device, EventSummary, KeyCode};
use tehuti_protocol::{ButtonName, ButtonState, Event};
use tracing::warn;

use crate::input::InputSource;

/// Labels pads in path order, counting each model independently.
#[must_use]
pub fn numbered_controllers(
    devices: impl IntoIterator<Item = (std::path::PathBuf, String, u16, u16)>,
) -> Vec<tehuti_core::dispatch::ControllerInfo> {
    let mut devices: Vec<_> = devices.into_iter().collect();
    devices.sort_by(|left, right| left.0.cmp(&right.0));
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    devices
        .into_iter()
        .map(|(_, name, vendor, product)| {
            let model = tehuti_core::pad::Identity::for_device(vendor, product).model_name(&name);
            let count = counts.entry(model.clone()).or_default();
            *count = count.saturating_add(1);
            tehuti_core::dispatch::ControllerInfo {
                name,
                label: format!("{model} #{count}"),
            }
        })
        .collect()
}

/// Opens and grabs every pad with a south face button that no other program holds,
/// so only Tehuti sees its input. Pads that fail to grab are logged and skipped.
#[must_use]
pub fn open_grabbed(
    virtual_pad: Option<&str>,
    attached: &std::collections::BTreeSet<std::path::PathBuf>,
) -> Vec<(std::path::PathBuf, EvdevPad)> {
    pad_candidates(attached)
        .filter(|(path, device)| {
            let sysfs = path
                .file_name()
                .and_then(|node| std::fs::canonicalize(std::path::Path::new("/sys/class/input").join(node)).ok());
            accepts_pad(device.input_id().bus_type(), sysfs.as_deref(), device.name(), virtual_pad)
        })
        .filter(|(_, device)| {
            device
                .supported_keys()
                .is_some_and(|keys| keys.contains(KeyCode::BTN_SOUTH))
        })
        .filter_map(|(path, mut device)| match grab_nonblocking(&mut device) {
            Ok(()) => {
                let mut translator = DpadHat::for_device(device.input_id().vendor(), device.input_id().product());
                if let Ok(axes) = device.get_absinfo() {
                    for (axis, info) in axes {
                        translator.set_trigger_range(axis, info.minimum(), info.maximum());
                    }
                }
                tracing::debug!(
                    name = device.name().unwrap_or("unnamed"),
                    vendor = device.input_id().vendor(),
                    identity = ?translator.identity,
                    "controller face layout selected"
                );
                match Device::open(&path) {
                    Ok(probe) => Some((path, EvdevPad::new(device, probe, translator))),
                    Err(error) => {
                        warn!(%error, "controller probe open failed; skipping it");
                        None
                    }
                }
            }
            Err(error) => {
                warn!(%error, name = device.name().unwrap_or("unnamed"), "controller grab failed; skipping it");
                None
            }
        })
        .collect()
}

/// Opens the input devices that could be pads. Opening and closing a device is slow on some kernels
/// (closing waits for an RCU grace period), and the scan runs every few hundred milliseconds,
/// so devices that sysfs says have no south face button are never opened.
fn pad_candidates(
    attached: &std::collections::BTreeSet<std::path::PathBuf>,
) -> impl Iterator<Item = (std::path::PathBuf, Device)> {
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir("/dev/input")
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|node| node.to_str())
                .is_some_and(|node| node.starts_with("event"))
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter(|path| !attached.contains(path))
        .filter(|path| {
            let capabilities = path.file_name().map(|node| {
                std::path::Path::new("/sys/class/input")
                    .join(node)
                    .join("device/capabilities/key")
            });
            // An unreadable or unparsable capability file falls back to opening the device.
            capabilities
                .and_then(|file| std::fs::read_to_string(file).ok())
                .and_then(|text| key_in_capabilities(&text, usize::from(KeyCode::BTN_SOUTH.code())))
                .unwrap_or(true)
        })
        .filter_map(|path| Device::open(&path).ok().map(|device| (path, device)))
}

/// Whether bit `bit` is set in a sysfs capability bitmap: hexadecimal words, most significant first,
/// each as wide as the kernel's `long`.
fn key_in_capabilities(text: &str, bit: usize) -> Option<bool> {
    let words = text
        .split_whitespace()
        .map(|word| usize::from_str_radix(word, 16).ok())
        .collect::<Option<Vec<usize>>>()?;
    let word_bits = usize::try_from(usize::BITS).ok()?;
    let from_end = bit.checked_div(word_bits)?;
    let word = words
        .len()
        .checked_sub(from_end.checked_add(1)?)
        .and_then(|index| words.get(index))
        .copied()
        .unwrap_or(0);
    Some(word >> bit.checked_rem(word_bits)? & 1 == 1)
}

/// Whether discovery accepts a physical pad or an explicitly named virtual pad.
#[must_use]
pub fn accepts_pad(
    bus: evdev::BusType,
    sysfs: Option<&std::path::Path>,
    name: Option<&str>,
    virtual_pad: Option<&str>,
) -> bool {
    is_physical(bus, sysfs)
        || virtual_pad.is_some_and(|substring| {
            !substring.is_empty() && name.is_some_and(|name| name.contains(substring))
        })
}

/// Whether a device is real hardware rather than a virtual one.
///
/// The sysfs path decides when it is known: virtual devices, such as uinput pads made by
/// other tools, live under `/sys/devices/virtual`.
/// UHID devices also live there, but `BlueZ` uses UHID for real Bluetooth pads,
/// so `/sys/devices/virtual/misc/uhid` counts as physical.
/// The bus type alone is not trusted: the xone driver reports `BUS_VIRTUAL` for real USB pads
/// (<https://github.com/dlundqvist/xone/blob/master/driver/common.c>).
/// Without a sysfs path, any bus other than `BUS_VIRTUAL` counts as physical.
#[must_use]
pub fn is_physical(bus: evdev::BusType, sysfs: Option<&std::path::Path>) -> bool {
    sysfs.map_or(bus != evdev::BusType::BUS_VIRTUAL, |path| {
        !path.starts_with("/sys/devices/virtual") || path.starts_with("/sys/devices/virtual/misc/uhid")
    })
}

/// Device operations needed to acquire exclusive, nonblocking input.
pub trait GrabDevice {
    /// Acquires exclusive input ownership.
    /// # Errors
    /// The kernel refused the grab.
    fn grab(&mut self) -> io::Result<()>;
    /// Enables nonblocking reads.
    /// # Errors
    /// The device flags could not be changed.
    fn set_nonblocking(&mut self, enabled: bool) -> io::Result<()>;
}

impl GrabDevice for Device {
    fn grab(&mut self) -> io::Result<()> {
        Self::grab(self)
    }
    fn set_nonblocking(&mut self, enabled: bool) -> io::Result<()> {
        Self::set_nonblocking(self, enabled)
    }
}

/// Acquires exclusive input and enables nonblocking reads.
/// # Errors
/// Either device operation failed.
pub fn grab_nonblocking(device: &mut impl GrabDevice) -> io::Result<()> {
    device.grab()?;
    device.set_nonblocking(true)
}

/// Watches controller node creation, removal and permission changes.
///
/// # Errors
/// The directory cannot be watched.
pub fn watch_input_directory(path: &std::path::Path) -> io::Result<std::os::fd::OwnedFd> {
    use rustix::fs::inotify;
    let watcher = inotify::init(inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC)?;
    let _watch = inotify::add_watch(
        &watcher,
        path,
        inotify::WatchFlags::CREATE
            | inotify::WatchFlags::DELETE
            | inotify::WatchFlags::ATTRIB
            | inotify::WatchFlags::MOVED_TO
            | inotify::WatchFlags::MOVED_FROM,
    )?;
    Ok(watcher)
}

/// An open kernel evdev controller with context-controlled exclusive ownership.
#[derive(Debug)]
pub struct EvdevPad<D = Device> {
    handles: PadHandles<D>,
    translator: DpadHat,
    grabbed: bool,
    mirror: Option<Mirror>,
    /// The virtual controller kept while hidden, so the application never sees it unplugged.
    parked: Option<Mirror>,
    mirror_failed: bool,
    session: Option<SessionPad>,
    plugin_held: Vec<ButtonName>,
    buffered: Vec<Event>,
}

/// The virtual controller presented to the application, with inputs kept from it.
struct Mirror {
    device: Box<dyn MirrorSink>,
    copy: MirrorCopy,
}

/// Retains the application's virtual controller until the session ends.
#[derive(Clone, Default)]
pub struct SessionPad(std::rc::Rc<core::cell::RefCell<RetainedSink>>);

type RetainedSink = Option<Box<dyn MirrorSink>>;

impl core::fmt::Debug for SessionPad {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("SessionPad")
    }
}

impl MirrorSink for SessionPad {
    fn neutral(&mut self, frame: &[evdev::InputEvent]) -> io::Result<()> {
        match self.0.borrow_mut().as_mut() {
            Some(sink) => sink.neutral(frame),
            None => Err(io::ErrorKind::NotConnected.into()),
        }
    }

    fn emit(&mut self, frame: &[evdev::InputEvent]) -> io::Result<()> {
        match self.0.borrow_mut().as_mut() {
            Some(sink) => sink.emit(frame),
            None => Err(io::ErrorKind::NotConnected.into()),
        }
    }
}

impl core::fmt::Debug for Mirror {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Mirror")
            .field("copy", &self.copy)
            .finish_non_exhaustive()
    }
}

/// Where a virtual controller's frames go.
pub trait MirrorSink {
    /// Sends one frame, ending in `SYN_REPORT`, to the application.
    /// # Errors
    /// The virtual controller could not be written.
    fn emit(&mut self, frame: &[evdev::InputEvent]) -> io::Result<()>;

    /// Emits a release frame whose axes already use virtual calibration.
    /// # Errors
    /// The virtual controller could not be written.
    fn neutral(&mut self, frame: &[evdev::InputEvent]) -> io::Result<()> {
        self.emit(frame)
    }

    /// Selects the physical feeder's identity and calibration.
    fn source(&mut self, _identity: evdev::InputId, _axes: &[(AbsoluteAxisCode, evdev::AbsInfo)]) {}
}

impl MirrorSink for evdev::uinput::VirtualDevice {
    fn emit(&mut self, frame: &[evdev::InputEvent]) -> io::Result<()> {
        Self::emit(self, frame)
    }
}

/// Whether the pad at `path` carries the one virtual pad: only the active pad does, and only while mirroring.
#[must_use]
pub fn mirror_wanted(mirrored: bool, active: Option<&std::path::Path>, path: &std::path::Path) -> bool {
    mirrored && active == Some(path)
}

/// What [`sync_mirrors`] changed.
#[derive(Debug, Default)]
pub struct MirrorSync {
    /// A virtual pad was just created.
    pub started: bool,
    /// Pads whose virtual pad could not be created.
    pub failed: Vec<std::path::PathBuf>,
}

/// Keeps exactly one virtual pad, on the active pad, while mirroring.
///
/// Old mirrors go first, releasing what they hold, so the application never sees two.
pub fn sync_mirrors<D: PadDevice>(
    pads: &mut [(&std::path::Path, &mut EvdevPad<D>)],
    mirrored: bool,
    active: Option<&std::path::Path>,
    consumed: &dyn Fn(ButtonName) -> bool,
) -> MirrorSync {
    let mut sync = MirrorSync::default();
    for (path, pad) in pads.iter_mut() {
        if !mirror_wanted(mirrored, active, path) {
            if let Err(error) = pad.set_mirror(false, consumed) {
                warn!(%error, "virtual pad release failed");
            }
            // Another pad presents the virtual pad now; a parked one would make two.
            if mirrored {
                pad.unpark_mirror();
            }
        }
    }
    for (path, pad) in pads.iter_mut() {
        if !mirror_wanted(mirrored, active, path) {
            continue;
        }
        match pad.set_mirror(true, consumed) {
            Ok(created) => sync.started |= created,
            Err(error) => {
                pad.mirror_failed = true;
                warn!(%error, "virtual pad unavailable");
                sync.failed.push(path.to_path_buf());
            }
        }
    }
    sync
}

/// Which raw events reach the virtual controller, frame by frame.
#[derive(Debug, Default)]
struct MirrorCopy {
    blocked: Vec<(u16, u16)>,
    frame: Vec<evdev::InputEvent>,
    buffered: Vec<Event>,
    /// Keys and hat axes the virtual controller currently shows as held.
    held: Vec<(u16, u16)>,
}

impl MirrorCopy {
    /// Hands over every translated event taken so far, for [`InputSource::drain`].
    fn drain(&mut self) -> Vec<Event> {
        core::mem::take(&mut self.buffered)
    }

    /// Takes one raw event; returns a finished frame to emit at each nonempty `SYN_REPORT`.
    fn take(
        &mut self,
        event: evdev::InputEvent,
        translator: &mut DpadHat,
        consume: &mut dyn FnMut(ButtonName, ButtonState) -> bool,
    ) -> Option<Vec<evdev::InputEvent>> {
        if event.event_type() == evdev::EventType::SYNCHRONIZATION {
            return (event.code() == evdev::SynchronizationCode::SYN_REPORT.0 && !self.frame.is_empty())
                .then(|| core::mem::take(&mut self.frame));
        }
        let translated = translator.translate(&event.destructure());
        let key = (event.event_type().0, event.code());
        let mut kept = self.blocked.contains(&key);
        for input in &translated {
            if let Event::Button { name, state } = input
                && consume(*name, *state)
            {
                kept = true;
                self.blocked.retain(|blocked| *blocked != key);
                if *state == ButtonState::Pressed {
                    self.blocked.push(key);
                }
            }
        }
        if !kept {
            let held_type = event.event_type() == evdev::EventType::KEY
                || event.event_type() == evdev::EventType::ABSOLUTE;
            if held_type {
                self.held.retain(|held| *held != key);
                if event.value() != 0 {
                    self.held.push(key);
                }
            }
            self.frame.push(event);
        }
        self.buffered.extend(translated);
        None
    }

    /// A frame that presses every held key and hat in `state`, marking them held.
    /// Keys whose button is `consumed` by a plugin are blocked instead, so their later release stays kept too.
    fn seed(
        &mut self,
        state: impl IntoIterator<Item = evdev::InputEvent>,
        translator: &mut DpadHat,
        consumed: &dyn Fn(ButtonName) -> bool,
    ) -> Option<Vec<evdev::InputEvent>> {
        let mut frame = Vec::new();
        for event in state.into_iter().filter(|event| event.value() != 0) {
            let key = (event.event_type().0, event.code());
            let kept = translator
                .translate(&event.destructure())
                .into_iter()
                .any(|input| matches!(input, Event::Button { name, .. } if consumed(name)));
            if kept {
                self.blocked.push(key);
            } else {
                self.held.push(key);
                frame.push(event);
            }
        }
        if frame.is_empty() {
            return None;
        }
        frame.push(evdev::InputEvent::new(
            evdev::EventType::SYNCHRONIZATION.0,
            evdev::SynchronizationCode::SYN_REPORT.0,
            0,
        ));
        Some(frame)
    }

    /// A frame that lets go of every key and hat the virtual controller shows as held.
    fn release(&mut self) -> Option<Vec<evdev::InputEvent>> {
        if self.held.is_empty() {
            return None;
        }
        let mut frame: Vec<_> = self
            .held
            .drain(..)
            .map(|(kind, code)| evdev::InputEvent::new(kind, code, 0))
            .collect();
        frame.push(evdev::InputEvent::new(
            evdev::EventType::SYNCHRONIZATION.0,
            evdev::SynchronizationCode::SYN_REPORT.0,
            0,
        ));
        Some(frame)
    }
}

/// The identity used by the virtual controller builder, independent of the source pad.
#[must_use]
pub fn virtual_pad_identity() -> evdev::InputId {
    evdev::InputId::new(evdev::BusType::BUS_VIRTUAL, 0x045e, 0x028e, 0x0001)
}

/// The Xbox 360 button capabilities advertised to applications.
#[must_use]
pub fn virtual_pad_keys() -> evdev::AttributeSet<KeyCode> {
    [
        KeyCode::BTN_SOUTH,
        KeyCode::BTN_EAST,
        KeyCode::BTN_NORTH,
        KeyCode::BTN_WEST,
        KeyCode::BTN_TL,
        KeyCode::BTN_TR,
        KeyCode::BTN_SELECT,
        KeyCode::BTN_START,
        KeyCode::BTN_MODE,
        KeyCode::BTN_THUMBL,
        KeyCode::BTN_THUMBR,
    ]
    .into_iter()
    .collect()
}

/// The Xbox axes, retaining the source's calibration where available.
#[must_use]
pub fn virtual_pad_axes(
    source: &[(AbsoluteAxisCode, evdev::AbsInfo)],
) -> Vec<(AbsoluteAxisCode, evdev::AbsInfo)> {
    use AbsoluteAxisCode as Axis;
    [
        (Axis::ABS_X, -32768, 32767),
        (Axis::ABS_Y, -32768, 32767),
        (Axis::ABS_Z, 0, 255),
        (Axis::ABS_RX, -32768, 32767),
        (Axis::ABS_RY, -32768, 32767),
        (Axis::ABS_RZ, 0, 255),
        (Axis::ABS_HAT0X, -1, 1),
        (Axis::ABS_HAT0Y, -1, 1),
    ]
    .into_iter()
    .map(|(axis, minimum, maximum)| {
        let info = source.iter().find(|(code, _)| *code == axis).map_or_else(
            || evdev::AbsInfo::new(0, minimum, maximum, 0, 0, 0),
            |(_, info)| {
                let range = info.maximum().checked_sub(info.minimum());
                if info.maximum() < info.minimum() || range.is_some_and(|range| info.flat() > range) {
                    evdev::AbsInfo::new(0, minimum, maximum, 0, 0, 0)
                } else {
                    *info
                }
            },
        );
        (axis, info)
    })
    .collect()
}

/// Converts source face positions to the Xbox kernel button order.
#[must_use]
pub fn virtual_pad_event(source: &evdev::InputId, event: evdev::InputEvent) -> evdev::InputEvent {
    if event.event_type() == evdev::EventType::KEY
        && !tehuti_core::pad::Identity::for_device(source.vendor(), source.product()).swap_north_west
    {
        let code = match KeyCode::new(event.code()) {
            KeyCode::BTN_NORTH => KeyCode::BTN_WEST,
            KeyCode::BTN_WEST => KeyCode::BTN_NORTH,
            _ => return event,
        };
        return evdev::InputEvent::new(event.event_type().0, code.code(), event.value());
    }
    event
}

struct VirtualPadSink {
    device: evdev::uinput::VirtualDevice,
    source: evdev::InputId,
    axes: Vec<(AbsoluteAxisCode, evdev::AbsInfo)>,
}

impl MirrorSink for VirtualPadSink {
    fn neutral(&mut self, frame: &[evdev::InputEvent]) -> io::Result<()> {
        let frame: Vec<_> = frame
            .iter()
            .map(|event| virtual_pad_event(&self.source, *event))
            .collect();
        self.device.emit(&frame)
    }

    fn source(&mut self, identity: evdev::InputId, axes: &[(AbsoluteAxisCode, evdev::AbsInfo)]) {
        self.source = identity;
        self.axes = virtual_pad_axes(axes);
    }

    fn emit(&mut self, frame: &[evdev::InputEvent]) -> io::Result<()> {
        let frame: Vec<_> = frame
            .iter()
            .filter_map(|event| {
                let mut event = virtual_pad_event(&self.source, *event);
                if event.event_type() == evdev::EventType::KEY
                    && !virtual_pad_keys().contains(KeyCode::new(event.code()))
                {
                    return None;
                }
                if event.event_type() == evdev::EventType::ABSOLUTE {
                    let defaults = virtual_pad_axes(&[]);
                    let (_, target) = defaults.iter().find(|(axis, _)| axis.0 == event.code())?;
                    let (_, source) = self.axes.iter().find(|(axis, _)| axis.0 == event.code())?;
                    let span = i64::from(source.maximum()).saturating_sub(i64::from(source.minimum()));
                    let value = if span > 0 {
                        let offset = i64::from(event.value().clamp(source.minimum(), source.maximum()))
                            .saturating_sub(i64::from(source.minimum()));
                        i64::from(target.minimum()).saturating_add(
                            offset
                                .saturating_mul(
                                    i64::from(target.maximum()).saturating_sub(i64::from(target.minimum())),
                                )
                                .checked_div(span)
                                .unwrap_or(0),
                        )
                    } else {
                        0
                    };
                    event = evdev::InputEvent::new(
                        event.event_type().0,
                        event.code(),
                        i32::try_from(value).unwrap_or(0),
                    );
                }
                Some(event)
            })
            .collect();
        self.device.emit(&frame)
    }
}

/// Kernel operations used by a controller client.
pub trait PadDevice {
    /// Identifies the feeder when attaching to a retained virtual controller.
    fn input_id(&self) -> evdev::InputId {
        virtual_pad_identity()
    }

    /// Acquires exclusive input ownership.
    /// # Errors
    /// The kernel refused the grab.
    fn grab(&mut self) -> io::Result<()>;
    /// Reads held keys.
    /// # Errors
    /// The client could not query key state.
    fn get_key_state(&self) -> io::Result<Vec<u16>>;
    /// Reads calibrated absolute axes.
    /// # Errors
    /// The client could not query axis state.
    fn get_absinfo(&self) -> io::Result<Vec<(AbsoluteAxisCode, evdev::AbsInfo)>>;
    /// Reads pending protocol events.
    /// # Errors
    /// The client could not read events.
    fn drain(&mut self, hat: &mut DpadHat) -> io::Result<Vec<Event>>;
    /// Releases exclusive input ownership.
    /// # Errors
    /// The client could not release its grab.
    fn ungrab(&mut self) -> io::Result<()>;
    /// Creates a virtual controller with the Xbox layout and this controller's calibration.
    /// # Errors
    /// The kernel refused to create it; clients without uinput always refuse.
    fn virtual_pad(&self) -> io::Result<Box<dyn MirrorSink>> {
        Err(io::ErrorKind::Unsupported.into())
    }
    /// Reads pending raw events without translating them.
    /// # Errors
    /// The client could not read events.
    fn fetch_raw(&mut self) -> io::Result<Vec<evdev::InputEvent>> {
        Ok(Vec::new())
    }
}

impl PadDevice for Device {
    fn input_id(&self) -> evdev::InputId {
        Self::input_id(self)
    }

    fn grab(&mut self) -> io::Result<()> {
        Self::grab(self)
    }
    fn get_key_state(&self) -> io::Result<Vec<u16>> {
        Ok(Self::get_key_state(self)?.iter().map(KeyCode::code).collect())
    }

    fn get_absinfo(&self) -> io::Result<Vec<(AbsoluteAxisCode, evdev::AbsInfo)>> {
        Ok(Self::get_absinfo(self)?.collect())
    }

    fn drain(&mut self, hat: &mut DpadHat) -> io::Result<Vec<Event>> {
        drain_device(self, hat)
    }

    fn ungrab(&mut self) -> io::Result<()> {
        Self::ungrab(self)
    }

    fn virtual_pad(&self) -> io::Result<Box<dyn MirrorSink>> {
        let keys = virtual_pad_keys();
        let mut builder = evdev::uinput::VirtualDevice::builder()
            .map_err(|error| io::Error::new(error.kind(), format!("uinput open: {error}")))?
            .name("Tehuti virtual pad")
            .input_id(virtual_pad_identity())
            .with_phys(c"tehuti/virtual-pad")
            .map_err(|error| io::Error::new(error.kind(), format!("uinput with_phys: {error}")))?
            .with_keys(&keys)
            .map_err(|error| io::Error::new(error.kind(), format!("uinput with_keys: {error}")))?;
        let source_axes: Vec<_> = Self::get_absinfo(self)?.collect();
        for (axis, info) in virtual_pad_axes(&[]) {
            builder = builder
                .with_absolute_axis(&evdev::UinputAbsSetup::new(axis, info))
                .map_err(|error| io::Error::new(error.kind(), format!("uinput axis {axis:?}: {error}")))?;
        }
        Ok(Box::new(VirtualPadSink {
            device: builder
                .build()
                .map_err(|error| io::Error::new(error.kind(), format!("uinput build: {error}")))?,
            source: self.input_id(),
            axes: virtual_pad_axes(&source_axes),
        }))
    }

    fn fetch_raw(&mut self) -> io::Result<Vec<evdev::InputEvent>> {
        match self.fetch_events() {
            Ok(events) => Ok(events.collect()),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }
}

/// Separate event delivery and state-query clients for one controller.
#[derive(Debug)]
pub struct PadHandles<D> {
    events: D,
    probe: D,
}

impl<D> PadHandles<D> {
    /// Uses independently opened clients for events and state queries.
    #[must_use]
    pub const fn new(events: D, probe: D) -> Self {
        Self { events, probe }
    }

    /// Borrows the event client for draining or releasing the grab.
    pub const fn events(&mut self) -> &mut D {
        &mut self.events
    }

    /// Borrows the ungrabbed client for state queries.
    #[must_use]
    pub const fn probe(&self) -> &D {
        &self.probe
    }
}

impl EvdevPad {
    #[must_use]
    pub fn name(&self) -> &str {
        self.handles.events.name().unwrap_or("unnamed")
    }

    #[must_use]
    pub fn usb_id(&self) -> (u16, u16) {
        let input_id = self.handles.events.input_id();
        (input_id.vendor(), input_id.product())
    }

    #[must_use]
    pub fn identity(&self) -> tehuti_core::pad::Identity {
        let input_id = self.handles.events.input_id();
        tehuti_core::pad::Identity::for_device(input_id.vendor(), input_id.product())
    }
}

impl<D: PadDevice> EvdevPad<D> {
    /// Attaches this physical feeder to the session's retained virtual controller.
    pub fn set_session(&mut self, session: &SessionPad) {
        self.session = Some(session.clone());
    }

    /// Presents or removes the virtual controller; true when it was just created.
    /// A new one starts out holding what the physical pad holds.
    /// Removal first releases whatever the virtual controller still holds.
    /// `consumed` asks, without changing anything, whether a plugin holds a button; those are not shown.
    /// # Errors
    /// The kernel refused to create the uinput device.
    pub fn set_mirror(&mut self, present: bool, consumed: &dyn Fn(ButtonName) -> bool) -> io::Result<bool> {
        if !present {
            if let Some(mut mirror) = self.mirror.take() {
                if let Some(frame) = mirror.copy.release() {
                    mirror.device.neutral(&frame)?;
                }
                self.parked = Some(mirror);
            }
            return Ok(false);
        }
        if self.mirror.is_some() || self.mirror_failed {
            return Ok(false);
        }
        let mut mirror = Mirror {
            device: match self.parked.take() {
                Some(parked) => parked.device,
                None => {
                    if let Some(session) = &self.session {
                        let mut sink = session.0.borrow_mut();
                        if sink.is_none() {
                            *sink = Some(self.handles.events.virtual_pad()?);
                        }
                        Box::new(session.clone())
                    } else {
                        self.handles.events.virtual_pad()?
                    }
                }
            },
            copy: MirrorCopy::default(),
        };
        if let Some(session) = &self.session
            && let Some(sink) = session.0.borrow_mut().as_mut()
        {
            sink.source(
                self.handles.events.input_id(),
                &self.handles.probe().get_absinfo()?,
            );
        }
        // The press that made this pad active was read before the mirror existed; show what is held now.
        let probe = self.handles.probe();
        let keys = probe
            .get_key_state()?
            .into_iter()
            .map(|code| evdev::InputEvent::new(evdev::EventType::KEY.0, code, 1));
        let hats = probe
            .get_absinfo()?
            .into_iter()
            .map(|(axis, info)| evdev::InputEvent::new(evdev::EventType::ABSOLUTE.0, axis.0, info.value()));
        // A fresh translator reads the held state as presses; the pad's own one has seen them already.
        let mut fresh = DpadHat {
            identity: self.translator.identity,
            ..DpadHat::default()
        };
        if let Some(frame) = mirror.copy.seed(keys.chain(hats), &mut fresh, consumed) {
            mirror.device.emit(&frame)?;
        }
        self.mirror = Some(mirror);
        Ok(true)
    }

    /// Removes the hidden virtual controller, so the application sees it unplugged.
    pub fn unpark_mirror(&mut self) {
        self.parked = None;
    }

    /// Whether this source pad must use physical passthrough after a mirror failure.
    #[must_use]
    pub const fn mirror_unavailable(&self) -> bool {
        self.mirror_failed
    }

    /// Whether the virtual controller is presented.
    #[must_use]
    pub const fn mirrored(&self) -> bool {
        self.mirror.is_some()
    }

    /// Copies pending physical input to the virtual controller, except buttons `consume` keeps.
    /// After removal, releases of plugin buttons still held reach `consume`.
    /// Every translated event stays buffered for [`InputSource::drain`].
    /// # Errors
    /// The physical pad could not be read or the virtual one written.
    pub fn pump_mirror(
        &mut self,
        consume: &mut dyn FnMut(ButtonName, ButtonState) -> bool,
    ) -> io::Result<()> {
        if self.mirror.is_none() && self.plugin_held.is_empty() {
            return Ok(());
        }
        let raw = match self.handles.events().fetch_raw() {
            Ok(raw) => raw,
            Err(error) => {
                let _release = self.set_mirror(false, &|_| false);
                return Err(error);
            }
        };
        for event in raw {
            if let Some(mirror) = &mut self.mirror {
                let plugin_held = &mut self.plugin_held;
                let mut track = |button, state| {
                    let kept = consume(button, state);
                    plugin_held.retain(|held| *held != button);
                    if kept && state == ButtonState::Pressed {
                        plugin_held.push(button);
                    }
                    kept
                };
                if let Some(frame) = mirror.copy.take(event, &mut self.translator, &mut track) {
                    mirror.device.emit(&frame)?;
                }
            } else {
                let translated = self.translator.translate(&event.destructure());
                for input in &translated {
                    if let Event::Button {
                        name,
                        state: ButtonState::Released,
                    } = input
                        && self.plugin_held.contains(name)
                    {
                        let _kept = consume(*name, ButtonState::Released);
                        self.plugin_held.retain(|held| held != name);
                    }
                }
                self.buffered.extend(translated);
            }
        }
        Ok(())
    }

    /// Matches exclusive ownership to the focused context, keeping the event client open.
    /// # Errors
    /// The kernel refused the ownership change.
    pub fn set_passthrough(&mut self, passthrough: bool) -> io::Result<()> {
        let grabbed = !passthrough;
        if grabbed != self.grabbed {
            if grabbed {
                self.handles.events().grab()?;
            } else {
                self.handles.events().ungrab()?;
            }
            self.grabbed = grabbed;
        }
        Ok(())
    }

    /// Uses a grabbed event client and an independently opened probe with the selected layout.
    #[must_use]
    pub const fn new(events: D, probe: D, translator: DpadHat) -> Self {
        Self {
            handles: PadHandles::new(events, probe),
            translator,
            grabbed: true,
            mirror: None,
            parked: None,
            mirror_failed: false,
            session: None,
            plugin_held: Vec::new(),
            buffered: Vec::new(),
        }
    }

    /// Samples the current calibrated stick axes, including a 15 percent deadzone.
    /// # Errors
    /// The kernel could not read the controller axis state.
    pub fn sticks(&self) -> io::Result<tehuti_core::dispatch::Sticks> {
        use evdev::AbsoluteAxisCode;
        let mut sticks = tehuti_core::dispatch::Sticks::default();
        for (axis, info) in self.handles.probe().get_absinfo()? {
            let value = stick_axis(info.value(), info.minimum(), info.maximum());
            match axis {
                AbsoluteAxisCode::ABS_X => sticks.left.0 = value,
                AbsoluteAxisCode::ABS_Y => sticks.left.1 = value,
                AbsoluteAxisCode::ABS_RX => sticks.right.0 = value,
                AbsoluteAxisCode::ABS_RY => sticks.right.1 = value,
                _ => {}
            }
        }
        Ok(sticks)
    }
    /// Fingerprints the kernel's view of held keys and D-pad hat positions.
    ///
    /// The kernel updates this state even when no event reaches our file descriptor,
    /// so a changing fingerprint without routed events means our read path is dead.
    /// # Errors
    /// The kernel could not read the controller state, as when the node is gone.
    pub fn activity(&self) -> io::Result<u64> {
        let keys = self.handles.probe().get_key_state()?;
        let axes: Vec<_> = self
            .handles
            .probe()
            .get_absinfo()?
            .into_iter()
            .map(|(axis, info)| (axis.0, info.value()))
            .collect();
        Ok(fingerprint(&keys, &axes))
    }
}

/// Hashes held keys and D-pad hats, excluding sampled sticks and triggers.
#[must_use]
pub fn fingerprint(keys: &[u16], axes: &[(u16, i32)]) -> u64 {
    use core::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    keys.hash(&mut hasher);
    for (axis, value) in axes {
        if *axis == AbsoluteAxisCode::ABS_HAT0X.0 || *axis == AbsoluteAxisCode::ABS_HAT0Y.0 {
            (axis, value).hash(&mut hasher);
        }
    }
    hasher.finish()
}

impl<D> Drop for EvdevPad<D> {
    fn drop(&mut self) {
        if let Some(mirror) = &mut self.mirror
            && let Some(frame) = mirror.copy.release()
            && let Err(error) = mirror.device.neutral(&frame)
        {
            warn!(%error, "virtual pad release failed");
        }
    }
}

impl AsFd for EvdevPad {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.handles.events.as_fd()
    }
}

impl<D: PadDevice> InputSource for EvdevPad<D> {
    fn release(&mut self) -> io::Result<()> {
        self.handles.events().ungrab()
    }

    fn drain(&mut self) -> io::Result<Vec<Event>> {
        let mut events = core::mem::take(&mut self.buffered);
        if let Some(mirror) = &mut self.mirror {
            events.extend(mirror.copy.drain());
        } else {
            events.extend(self.handles.events().drain(&mut self.translator)?);
        }
        Ok(events)
    }
}

/// Reads every pending event from `device`. Stops at `WouldBlock`.
fn drain_device(device: &mut Device, hat: &mut DpadHat) -> io::Result<Vec<Event>> {
    match device.fetch_events() {
        Ok(events) => Ok(events
            .flat_map(|event| hat.translate(&event.destructure()))
            .collect()),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

/// Translates controller keys into physical button positions, d-pad hat axes
/// (`ABS_HAT0X`, `ABS_HAT0Y`) into d-pad button events, and trigger axes
/// (`ABS_Z`, `ABS_RZ`) into trigger button events.
#[derive(Debug, Default)]
pub struct DpadHat {
    identity: tehuti_core::pad::Identity,
    horizontal: i32,
    vertical: i32,
    left_trigger: Trigger,
    right_trigger: Trigger,
}

/// One analogue trigger read as a button, with hysteresis so a resting finger does not chatter.
#[derive(Debug, Clone, Copy)]
struct Trigger {
    minimum: i32,
    maximum: i32,
    held: bool,
}

impl Default for Trigger {
    /// The 8-bit range most pads report until the device's own range is known.
    fn default() -> Self {
        Self {
            minimum: 0,
            maximum: 255,
            held: false,
        }
    }
}

impl Trigger {
    /// Percent of travel that presses the trigger.
    const PRESS_PERCENT: i64 = 40;
    /// Percent of travel below which a held trigger releases.
    const RELEASE_PERCENT: i64 = 20;

    /// Steps the held state for a new axis value and names the edge, if any.
    fn step(&mut self, value: i32) -> Option<ButtonState> {
        let span = i64::from(self.maximum)
            .saturating_sub(i64::from(self.minimum))
            .max(1);
        let percent_of_span = i64::from(value)
            .saturating_sub(i64::from(self.minimum))
            .saturating_mul(100);
        let past = |percent: i64| percent_of_span >= span.saturating_mul(percent);
        let next = if self.held {
            past(Self::RELEASE_PERCENT)
        } else {
            past(Self::PRESS_PERCENT)
        };
        if next == self.held {
            return None;
        }
        self.held = next;
        Some(if next {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        })
    }
}

impl DpadHat {
    #[must_use]
    pub fn for_vendor(vendor: u16) -> Self {
        Self::for_device(vendor, 0)
    }

    #[must_use]
    pub fn for_device(vendor: u16, product: u16) -> Self {
        Self {
            identity: tehuti_core::pad::Identity::for_device(vendor, product),
            ..Self::default()
        }
    }

    /// Records the device's reported range for a trigger axis; other axes are ignored.
    pub const fn set_trigger_range(&mut self, axis: AbsoluteAxisCode, minimum: i32, maximum: i32) {
        let trigger = match axis {
            AbsoluteAxisCode::ABS_Z => &mut self.left_trigger,
            AbsoluteAxisCode::ABS_RZ => &mut self.right_trigger,
            _ => return,
        };
        if maximum > minimum {
            trigger.minimum = minimum;
            trigger.maximum = maximum;
        }
    }

    const fn face_code(&self, code: KeyCode) -> KeyCode {
        match (self.identity.swap_north_west, code) {
            (true, KeyCode::BTN_NORTH) => KeyCode::BTN_WEST,
            (true, KeyCode::BTN_WEST) => KeyCode::BTN_NORTH,
            _ => code,
        }
    }

    /// Translates one evdev event: keys parse as buttons, hat axes as d-pad presses and releases.
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "only keys, the d-pad hat and the trigger axes matter; every other event kind is ignored"
    )]
    pub fn translate(&mut self, event: &EventSummary) -> Vec<Event> {
        match *event {
            EventSummary::Key(_, code, value) => {
                parse_button(self.face_code(code), value).into_iter().collect()
            }
            EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_HAT0X, value) => Self::step(
                &mut self.horizontal,
                value,
                ButtonName::DpadLeft,
                ButtonName::DpadRight,
            ),
            EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_HAT0Y, value) => Self::step(
                &mut self.vertical,
                value,
                ButtonName::DpadUp,
                ButtonName::DpadDown,
            ),
            EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_Z, value) => {
                trigger_event(&mut self.left_trigger, value, ButtonName::LeftTrigger)
            }
            EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_RZ, value) => {
                trigger_event(&mut self.right_trigger, value, ButtonName::RightTrigger)
            }
            _ => Vec::new(),
        }
    }

    fn step(held: &mut i32, value: i32, negative: ButtonName, positive: ButtonName) -> Vec<Event> {
        let next = value.signum();
        let name_of = |direction: i32| match direction {
            -1 => Some(negative),
            1 => Some(positive),
            _ => None,
        };
        let previous = core::mem::replace(held, next);
        if previous == next {
            return Vec::new();
        }
        let released = name_of(previous).map(|name| Event::Button {
            name,
            state: ButtonState::Released,
        });
        let pressed = name_of(next).map(|name| Event::Button {
            name,
            state: ButtonState::Pressed,
        });
        released.into_iter().chain(pressed).collect()
    }
}

fn trigger_event(trigger: &mut Trigger, value: i32, name: ButtonName) -> Vec<Event> {
    trigger
        .step(value)
        .map(|state| Event::Button { name, state })
        .into_iter()
        .collect()
}

/// Parses an evdev key event. Autorepeat (value 2) and unknown codes give `None`.
const fn parse_button(code: KeyCode, value: i32) -> Option<Event> {
    let state = match value {
        1 => ButtonState::Pressed,
        0 => ButtonState::Released,
        _ => return None,
    };
    let name = match code {
        KeyCode::BTN_SOUTH => ButtonName::South,
        KeyCode::BTN_EAST => ButtonName::East,
        KeyCode::BTN_NORTH => ButtonName::North,
        KeyCode::BTN_WEST => ButtonName::West,
        KeyCode::BTN_TL => ButtonName::LeftBumper,
        KeyCode::BTN_TR => ButtonName::RightBumper,
        KeyCode::BTN_SELECT => ButtonName::Select,
        KeyCode::KEY_RECORD => ButtonName::Record,
        KeyCode::BTN_START => ButtonName::Start,
        KeyCode::BTN_MODE => ButtonName::Guide,
        KeyCode::BTN_THUMBL => ButtonName::LeftStick,
        KeyCode::BTN_THUMBR => ButtonName::RightStick,
        KeyCode::BTN_DPAD_UP => ButtonName::DpadUp,
        KeyCode::BTN_DPAD_DOWN => ButtonName::DpadDown,
        KeyCode::BTN_DPAD_LEFT => ButtonName::DpadLeft,
        KeyCode::BTN_DPAD_RIGHT => ButtonName::DpadRight,
        _ => return None,
    };
    Some(Event::Button { name, state })
}

#[cfg(test)]
mod mirror_tests {
    use super::{DpadHat, MirrorCopy};
    use evdev::{EventType, InputEvent, KeyCode, SynchronizationCode};
    use tehuti_protocol::{ButtonName, ButtonState, Event};

    fn key(code: KeyCode, value: i32) -> InputEvent {
        InputEvent::new(EventType::KEY.0, code.code(), value)
    }

    #[test]
    fn one_frame_keeps_matched_inputs_and_copies_the_rest() {
        let mut copy = MirrorCopy::default();
        let mut translator = DpadHat::default();
        let mut consume = |name: ButtonName, _: ButtonState| name == ButtonName::South;
        let frame = [
            key(KeyCode::BTN_TL, 1),
            key(KeyCode::BTN_SOUTH, 1),
            key(KeyCode::BTN_EAST, 1),
            InputEvent::new(EventType::SYNCHRONIZATION.0, SynchronizationCode::SYN_REPORT.0, 0),
        ];
        let emitted: Vec<_> = frame
            .into_iter()
            .filter_map(|event| copy.take(event, &mut translator, &mut consume))
            .collect();
        let codes: Vec<Vec<u16>> = emitted
            .iter()
            .map(|sent| sent.iter().map(InputEvent::code).collect())
            .collect();
        assert_eq!(
            codes,
            [vec![KeyCode::BTN_TL.code(), KeyCode::BTN_EAST.code()]],
            "South is kept; the bumper and East are copied in order"
        );
        let pressed = |name| Event::Button {
            name,
            state: ButtonState::Pressed,
        };
        assert_eq!(
            copy.drain(),
            [
                pressed(ButtonName::LeftBumper),
                pressed(ButtonName::South),
                pressed(ButtonName::East)
            ],
            "the core still sees every press, the kept one included"
        );
    }

    fn sync() -> InputEvent {
        InputEvent::new(EventType::SYNCHRONIZATION.0, SynchronizationCode::SYN_REPORT.0, 0)
    }

    proptest::proptest! {
        #[test]
        fn at_most_the_active_pad_is_mirrored(
            pads in proptest::collection::btree_set(0_u8..8, 0..6),
            active in proptest::option::of(0_u8..10),
            mirrored in proptest::bool::ANY,
        ) {
            let path = |number: u8| std::path::PathBuf::from(format!("/dev/input/event{number}"));
            let active = active.map(path);
            let wanted = pads
                .iter()
                .filter(|number| super::mirror_wanted(mirrored, active.as_deref(), &path(**number)))
                .count();
            proptest::prop_assert!(wanted <= 1, "the application sees one virtual pad at most");
            if active.is_none() {
                proptest::prop_assert_eq!(wanted, 0, "a pad that never sent input gets no mirror");
            }
        }

        #[test]
        fn release_lets_go_of_exactly_what_the_mirror_still_holds(
            series in proptest::collection::vec(
                (proptest::sample::select(vec![
                    (EventType::KEY.0, KeyCode::BTN_SOUTH.code()),
                    (EventType::KEY.0, KeyCode::BTN_EAST.code()),
                    (EventType::KEY.0, KeyCode::BTN_TL.code()),
                    (EventType::ABSOLUTE.0, evdev::AbsoluteAxisCode::ABS_HAT0X.0),
                    (EventType::ABSOLUTE.0, evdev::AbsoluteAxisCode::ABS_X.0),
                ]), -1_i32..=1, proptest::bool::ANY),
                0..40,
            ),
        ) {
            let mut copy = MirrorCopy::default();
            let mut translator = DpadHat::default();
            let mut consume = |name: ButtonName, _: ButtonState| name == ButtonName::South;
            let mut shown = std::collections::BTreeMap::new();
            let apply = |frame: Vec<InputEvent>, state: &mut std::collections::BTreeMap<(u16, u16), i32>| {
                for event in frame {
                    if event.event_type() != EventType::SYNCHRONIZATION {
                        let _previous = state.insert((event.event_type().0, event.code()), event.value());
                    }
                }
            };
            for ((kind, code), value, report) in series {
                let value = if kind == EventType::KEY.0 { value.abs() } else { value };
                if let Some(frame) = copy.take(InputEvent::new(kind, code, value), &mut translator, &mut consume) {
                    apply(frame, &mut shown);
                }
                if report && let Some(frame) = copy.take(sync(), &mut translator, &mut consume) {
                    apply(frame, &mut shown);
                }
            }
            // Whatever was taken but not yet reported is flushed first, as the next report would.
            if let Some(frame) = copy.take(sync(), &mut translator, &mut consume) {
                apply(frame, &mut shown);
            }
            if let Some(frame) = copy.release() {
                apply(frame, &mut shown);
            }
            let stuck: Vec<_> = shown
                .iter()
                .filter(|((kind, code), value)| {
                    **value != 0 && (*kind == EventType::KEY.0 || *code == evdev::AbsoluteAxisCode::ABS_HAT0X.0)
                })
                .collect();
            proptest::prop_assert!(stuck.is_empty(), "still held after release: {stuck:?}");
            proptest::prop_assert_eq!(copy.release(), None);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn usb_ids_give_distinct_models_and_keep_kernel_names() {
        let pads = super::numbered_controllers([
            (
                "/dev/input/event9".into(),
                "Xbox Wireless Controller".into(),
                0x045e,
                0x0b13,
            ),
            (
                "/dev/input/event2".into(),
                "Wireless Controller".into(),
                0x054c,
                0x09cc,
            ),
        ]);
        assert_eq!(pads[0].name, "Wireless Controller");
        assert_eq!(pads[0].label, "DUALSHOCK 4 #1");
        assert_eq!(pads[1].name, "Xbox Wireless Controller");
        assert_eq!(pads[1].label, "XBOX CONTROLLER #1");
    }

    #[test]
    fn matching_models_are_numbered_by_device_path() {
        let pads = super::numbered_controllers([
            (
                "/dev/input/event9".into(),
                "Wireless Controller".into(),
                0x054c,
                0x05c4,
            ),
            (
                "/dev/input/event2".into(),
                "Wireless Controller".into(),
                0x054c,
                0x09cc,
            ),
        ]);
        assert_eq!(
            pads.iter().map(|pad| pad.label.as_str()).collect::<Vec<_>>(),
            ["DUALSHOCK 4 #1", "DUALSHOCK 4 #2"]
        );
    }

    #[test]
    fn native_capability_bitmaps_distinguish_pads_from_keyboards() {
        let (pad, keyboard) = if usize::BITS == 64 {
            (
                "7fdb000000000000 0 0 0 0",
                "4 20000023803078 f800d001feffffdf ffefffffffffffff fffffffe",
            )
        } else {
            (
                "7fdb0000 0 0 0 0 0 0 0 0 0",
                "4 2000002 3803078 f800d001 feffffdf ffefffff ffffffff fffffffe",
            )
        };
        let south = usize::from(KeyCode::BTN_SOUTH.code());
        assert_eq!(key_in_capabilities(pad, south), Some(true));
        assert_eq!(key_in_capabilities(keyboard, south), Some(false));
        assert_eq!(key_in_capabilities("0", south), Some(false));
    }

    #[test]
    fn capability_bitmap_finds_the_south_face_button() {
        let south = usize::from(KeyCode::BTN_SOUTH.code());
        // A 32-bit `long` DualShock 4 pad and the keyboard of the same machine, as sysfs prints them.
        let pad = "7fdb0000 0 0 0 0 0 0 0 0 0";
        let keyboard = "4 2000002 3803078 f800d001 feffffdf ffefffff ffffffff fffffffe";
        if usize::BITS == 32 {
            assert_eq!(key_in_capabilities(pad, south), Some(true));
            assert_eq!(key_in_capabilities(keyboard, south), Some(false));
        }
        assert_eq!(key_in_capabilities("not hex", south), None);
        assert_eq!(key_in_capabilities("", south), Some(false));
    }

    use super::*;
    use proptest::prelude::*;

    #[test]
    fn vendor_layout_maps_face_buttons_to_physical_positions() {
        for (vendor, code, name) in [
            (0x045e, KeyCode(0x133), ButtonName::West),
            (0x045e, KeyCode(0x134), ButtonName::North),
            (0x054c, KeyCode::BTN_NORTH, ButtonName::North),
            (0x054c, KeyCode::BTN_WEST, ButtonName::West),
            (0xffff, KeyCode::BTN_NORTH, ButtonName::North),
            (0xffff, KeyCode::BTN_WEST, ButtonName::West),
        ] {
            let mut pad = DpadHat::for_vendor(vendor);
            for (value, state) in [(1, ButtonState::Pressed), (0, ButtonState::Released)] {
                let event = evdev::InputEvent::new(evdev::EventType::KEY.0, code.code(), value);
                assert_eq!(pad.translate(&event.destructure()), [button(name, state)]);
            }
        }
    }

    proptest! {
        #[test]
        fn stick_and_trigger_drift_leaves_the_fingerprint_unchanged(
            keys in prop::collection::vec(any::<u16>(), 0..8),
            hat in (-1_i32..=1, -1_i32..=1),
            drift in prop::collection::vec((0_u16..6, any::<i32>()), 1..12),
        ) {
            let hats = [(AbsoluteAxisCode::ABS_HAT0X.0, hat.0), (AbsoluteAxisCode::ABS_HAT0Y.0, hat.1)];
            let mut drifted = drift;
            drifted.extend(hats);
            prop_assert_eq!(super::fingerprint(&keys, &hats), super::fingerprint(&keys, &drifted));
        }

        #[test]
        fn vendor_layout_preserves_every_other_raw_key(
            code in any::<u16>().prop_filter("face codes have vendor layouts", |code| {
                *code != KeyCode::BTN_NORTH.code() && *code != KeyCode::BTN_WEST.code()
            }),
            value in any::<i32>(),
            vendor in any::<u16>(),
        ) {
            let event = evdev::InputEvent::new(evdev::EventType::KEY.0, code, value);
            prop_assert_eq!(
                DpadHat::for_vendor(vendor).translate(&event.destructure()),
                DpadHat::default().translate(&event.destructure())
            );
        }
    }

    #[test]
    fn south_button_press_is_a_pressed() {
        assert_eq!(
            parse_button(KeyCode::BTN_SOUTH, 1),
            Some(Event::Button {
                name: ButtonName::South,
                state: ButtonState::Pressed
            }),
            "xpad reports A as BTN_SOUTH"
        );
    }

    fn hat(axis: AbsoluteAxisCode, value: i32) -> EventSummary {
        evdev::InputEvent::new(evdev::EventType::ABSOLUTE.0, axis.0, value).destructure()
    }

    fn button(name: ButtonName, state: ButtonState) -> Event {
        Event::Button { name, state }
    }

    #[test]
    fn hat_axes_press_and_release_dpad_buttons() {
        let mut pad = DpadHat::default();
        let x = AbsoluteAxisCode::ABS_HAT0X;
        let y = AbsoluteAxisCode::ABS_HAT0Y;
        assert_eq!(
            pad.translate(&hat(x, -1)),
            [button(ButtonName::DpadLeft, ButtonState::Pressed)]
        );
        assert_eq!(
            pad.translate(&hat(x, 0)),
            [button(ButtonName::DpadLeft, ButtonState::Released)]
        );
        assert_eq!(
            pad.translate(&hat(y, -1)),
            [button(ButtonName::DpadUp, ButtonState::Pressed)]
        );
        assert_eq!(
            pad.translate(&hat(y, 0)),
            [button(ButtonName::DpadUp, ButtonState::Released)]
        );
        assert_eq!(
            pad.translate(&hat(y, 1)),
            [button(ButtonName::DpadDown, ButtonState::Pressed)]
        );
        assert_eq!(pad.translate(&hat(y, 1)), [], "repeated value is not a new press");
        assert_eq!(
            pad.translate(&hat(y, 0)),
            [button(ButtonName::DpadDown, ButtonState::Released)]
        );
        assert_eq!(pad.translate(&hat(x, 0)), [], "release without press");
        assert_eq!(
            pad.translate(&hat(x, 5)),
            [button(ButtonName::DpadRight, ButtonState::Pressed)],
            "any positive value presses"
        );
    }

    fn trigger_pad(maximum: i32) -> DpadHat {
        let mut pad = DpadHat::default();
        pad.set_trigger_range(AbsoluteAxisCode::ABS_Z, 0, maximum);
        pad.set_trigger_range(AbsoluteAxisCode::ABS_RZ, 0, maximum);
        pad
    }

    #[test]
    fn trigger_presses_past_the_threshold_and_releases_below_the_lower_one() {
        let mut pad = trigger_pad(1000);
        let z = AbsoluteAxisCode::ABS_Z;
        assert_eq!(pad.translate(&hat(z, 399)), [], "below the press threshold");
        assert_eq!(
            pad.translate(&hat(z, 400)),
            [button(ButtonName::LeftTrigger, ButtonState::Pressed)]
        );
        assert_eq!(pad.translate(&hat(z, 1000)), [], "deeper is still one press");
        assert_eq!(pad.translate(&hat(z, 250)), [], "hysteresis keeps it held");
        assert_eq!(
            pad.translate(&hat(z, 199)),
            [button(ButtonName::LeftTrigger, ButtonState::Released)]
        );
        assert_eq!(
            pad.translate(&hat(AbsoluteAxisCode::ABS_RZ, 999)),
            [button(ButtonName::RightTrigger, ButtonState::Pressed)]
        );
    }

    #[test]
    fn triggers_use_the_device_range() {
        let mut small = trigger_pad(255);
        assert_eq!(
            small.translate(&hat(AbsoluteAxisCode::ABS_Z, 110)),
            [button(ButtonName::LeftTrigger, ButtonState::Pressed)]
        );
        let mut large = trigger_pad(1023);
        assert_eq!(large.translate(&hat(AbsoluteAxisCode::ABS_Z, 110)), []);
    }

    proptest! {
        /// Presses and releases alternate, a press only follows a value past the upper threshold,
        /// a release only follows one below the lower threshold, and noise between them is silent.
        #[test]
        fn trigger_events_alternate_and_respect_the_thresholds(
            maximum in 1_i32..4096,
            values in prop::collection::vec(0_u16..=4096, 0..64),
        ) {
            let mut pad = trigger_pad(maximum);
            let mut held = false;
            for raw in values {
                let value = i32::from(raw).min(maximum);
                let events = pad.translate(&hat(AbsoluteAxisCode::ABS_RZ, value));
                let past = |percent: i32| value.saturating_mul(100) >= maximum.saturating_mul(percent);
                let silent = if held { past(20) } else { !past(40) };
                match events.as_slice() {
                    [] => prop_assert!(silent),
                    [Event::Button { name: ButtonName::RightTrigger, state }] => {
                        prop_assert_eq!(*state == ButtonState::Pressed, !held);
                        prop_assert!(!silent);
                        held = !held;
                    }
                    other => prop_assert!(false, "unexpected events {:?}", other),
                }
            }
        }
    }

    #[test]
    fn hat_direct_flip_releases_old_then_presses_new() {
        let mut pad = DpadHat::default();
        let x = AbsoluteAxisCode::ABS_HAT0X;
        drop(pad.translate(&hat(x, -1)));
        assert_eq!(
            pad.translate(&hat(x, 1)),
            [
                button(ButtonName::DpadLeft, ButtonState::Released),
                button(ButtonName::DpadRight, ButtonState::Pressed)
            ]
        );
    }

    #[test]
    fn dpad_key_buttons_still_work_through_translate() {
        let key = evdev::InputEvent::new(evdev::EventType::KEY.0, KeyCode::BTN_DPAD_UP.0, 1).destructure();
        assert_eq!(
            DpadHat::default().translate(&key),
            [button(ButtonName::DpadUp, ButtonState::Pressed)]
        );
    }

    #[test]
    fn virtual_devices_are_not_physical() {
        use evdev::BusType;
        use std::path::Path;
        let uinput = Path::new("/sys/devices/virtual/input/input42/event20");
        let usb = Path::new("/sys/devices/pci0000:00/0000:00:14.0/usb1/1-2/1-2:1.0/input/input5/event3");
        assert!(
            !is_physical(BusType::BUS_USB, Some(uinput)),
            "uinput pad spoofing USB"
        );
        let xone = Path::new(
            "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-3/1-3:1.0/gip0/gip0.0/input/input25/event18",
        );
        assert!(
            is_physical(BusType::BUS_VIRTUAL, Some(xone)),
            "xone pad reports BUS_VIRTUAL under a real USB device"
        );
        assert!(
            !is_physical(BusType::BUS_VIRTUAL, Some(uinput)),
            "uinput pad such as Sunshine libvirtualhid"
        );
        assert!(
            !is_physical(BusType::BUS_VIRTUAL, None),
            "virtual bus without sysfs"
        );
        assert!(is_physical(BusType::BUS_USB, Some(usb)), "real Letters pad");
        assert!(is_physical(BusType::BUS_BLUETOOTH, None), "no sysfs");
        let uhid = Path::new("/sys/devices/virtual/misc/uhid/0005:045E:0B13.0003/input/input30/event21");
        assert!(
            is_physical(BusType::BUS_BLUETOOTH, Some(uhid)),
            "Bluetooth pad via BlueZ UHID"
        );
    }

    #[test]
    fn stick_calibration_handles_deadzone_extremes_and_invalid_ranges() {
        assert_eq!(stick_axis(0, -32768, 32767), 0);
        assert_eq!(stick_axis(4000, -32768, 32767), 0);
        assert_eq!(stick_axis(-32768, -32768, 32767), -1000);
        assert_eq!(stick_axis(32767, -32768, 32767), 1000);
        assert_eq!(stick_axis(128, 0, 255), 0);
        assert_eq!(stick_axis(0, 0, 255), -1000);
        assert_eq!(stick_axis(255, 0, 255), 1000);
        assert_eq!(stick_axis(10, 10, 10), 0);
        assert_eq!(stick_axis(10, 20, 10), 0);
    }

    #[test]
    fn autorepeat_and_unknown_codes_are_dropped() {
        assert_eq!(parse_button(KeyCode::BTN_SOUTH, 2), None, "autorepeat");
        assert_eq!(parse_button(KeyCode::KEY_A, 1), None, "keyboard key");
    }
}

/// Controller discovery state, including the next periodic scan.
#[derive(Debug)]
pub enum Discovery {
    Attached { retry_at: core::time::Duration },
    Searching { retry_at: core::time::Duration },
}

/// Inputs from the clock, directory watcher and device registration.
#[derive(Debug)]
pub enum DiscoveryInput {
    Poll { now: core::time::Duration, rescan: bool },
    Disconnected { now: core::time::Duration },
    Found,
}

/// Effects performed by the event loop.
#[derive(Debug, PartialEq, Eq)]
pub enum DiscoveryEffect {
    Scan,
    Reconnected,
}

pub const RETRY_INTERVAL: core::time::Duration = core::time::Duration::from_millis(250);

/// Schedules discovery and announces successful attachment after a disconnect.
#[must_use]
pub fn discover(state: Discovery, input: DiscoveryInput) -> (Discovery, Option<DiscoveryEffect>) {
    match (state, input) {
        (Discovery::Attached { .. }, DiscoveryInput::Disconnected { now }) => {
            (Discovery::Searching { retry_at: now }, None)
        }
        (state, DiscoveryInput::Poll { now, rescan }) => {
            let retry_at = match state {
                Discovery::Attached { retry_at } | Discovery::Searching { retry_at } => retry_at,
            };
            if !rescan && now < retry_at {
                return (state, None);
            }
            let next_retry_at = now.saturating_add(RETRY_INTERVAL);
            let next = match state {
                Discovery::Attached { .. } => Discovery::Attached {
                    retry_at: next_retry_at,
                },
                Discovery::Searching { .. } => Discovery::Searching {
                    retry_at: next_retry_at,
                },
            };
            (next, Some(DiscoveryEffect::Scan))
        }
        (Discovery::Searching { retry_at }, DiscoveryInput::Found) => (
            Discovery::Attached { retry_at },
            Some(DiscoveryEffect::Reconnected),
        ),
        (state, _) => (state, None),
    }
}

/// How long kernel-side activity may go without a routed event before the pad is re-opened.
pub const SILENCE_LIMIT: core::time::Duration = core::time::Duration::from_secs(1);

/// Notices a grabbed pad whose kernel state changes while none of its events reach us.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Watchdog {
    fingerprint: Option<u64>,
    unanswered_since: Option<core::time::Duration>,
    reopen: bool,
}

impl Watchdog {
    /// Asks the event loop to drop and re-open the pad, as after a failed read.
    pub const fn request_reopen(&mut self) {
        self.reopen = true;
    }

    /// Takes a pending re-open request.
    pub const fn take_reopen(&mut self) -> bool {
        core::mem::replace(&mut self.reopen, false)
    }
}

/// What the watchdog observes from the event loop.
#[derive(Debug, Clone, Copy)]
pub enum WatchInput {
    /// The pad's activity fingerprint, sampled at `now`.
    Sample {
        now: core::time::Duration,
        fingerprint: u64,
    },
    /// At least one event was read from the pad.
    Events,
}

/// Whether the pad must be dropped and re-opened.
#[derive(Debug, PartialEq, Eq)]
pub enum WatchVerdict {
    Healthy,
    Reopen,
}

/// Advances the watchdog. Performs no I/O.
#[must_use]
pub fn watch(mut state: Watchdog, input: WatchInput) -> (Watchdog, WatchVerdict) {
    match input {
        WatchInput::Events => {
            state.unanswered_since = None;
            state.fingerprint = None;
            (state, WatchVerdict::Healthy)
        }
        WatchInput::Sample { now, fingerprint } => {
            let previous = state.fingerprint.replace(fingerprint);
            if previous.is_some_and(|previous| previous != fingerprint) {
                let _first = state.unanswered_since.get_or_insert(now);
            }
            let verdict = if !state.reopen
                && state
                    .unanswered_since
                    .is_some_and(|since| now.saturating_sub(since) >= SILENCE_LIMIT)
            {
                WatchVerdict::Reopen
            } else {
                WatchVerdict::Healthy
            };
            (state, verdict)
        }
    }
}

fn stick_axis(value: i32, minimum: i32, maximum: i32) -> i16 {
    let range = i64::from(maximum).saturating_sub(i64::from(minimum));
    if range <= 0 {
        return 0;
    }
    let normalized = (i64::from(value)
        .saturating_sub(i64::from(minimum))
        .saturating_mul(2000)
        .checked_div(range)
        .unwrap_or_default()
        .saturating_sub(1000))
    .clamp(-1000, 1000);
    let travel = normalized.abs().saturating_sub(150).max(0).saturating_mul(1000) / 850;
    i16::try_from(travel.saturating_mul(normalized.signum())).unwrap_or_default()
}
