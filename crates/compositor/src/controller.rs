//! The controller edge: finds the pad, grabs it, and parses its events into protocol events.

use std::{
    io,
    os::fd::{AsFd, BorrowedFd},
};

use evdev::{AbsoluteAxisCode, Device, EventSummary, KeyCode};
use tehuti_protocol::{ButtonName, ButtonState, Event};
use tracing::warn;

use crate::input::InputSource;

/// One connected pad as the settings view needs it: its node path, kernel name, USB ids and
/// the device's own unique id or physical path (empty when it reports neither).
pub type ConnectedPad = (
    std::path::PathBuf,
    String,
    u16,
    u16,
    String,
    tehuti_core::dispatch::Connection,
);

/// Labels pads in path order, counting each model independently.
///
/// Each pad also gets a key that survives a reconnect: the unique id or physical path
/// when it is the pad's alone, else the kernel name with an index. A pad in `known`
/// (path to key, from the previous call) keeps its key whatever attaches beside it; a new
/// pad takes the first key of its identity that no pad holds.
#[must_use]
pub fn keyed_controllers(
    devices: impl IntoIterator<Item = ConnectedPad>,
    known: &std::collections::BTreeMap<std::path::PathBuf, String>,
) -> Vec<(std::path::PathBuf, tehuti_core::dispatch::ControllerInfo)> {
    let mut devices: Vec<_> = devices.into_iter().collect();
    devices.sort_by(|left, right| left.0.cmp(&right.0));
    let mut taken: std::collections::BTreeSet<String> = devices
        .iter()
        .filter_map(|device| known.get(&device.0).cloned())
        .collect();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    devices
        .into_iter()
        .map(|(path, name, vendor, product, identity, connection)| {
            let model = tehuti_core::pad::Identity::for_node(vendor, product, &name).model_name(&name);
            let count = counts.entry(model.clone()).or_default();
            *count = count.saturating_add(1);
            let key = known.get(&path).cloned().unwrap_or_else(|| {
                let candidates = (1_usize..).map(|index| match (identity.is_empty(), index) {
                    (false, 1) => identity.clone(),
                    (false, _) => format!("{identity}#{index}"),
                    (true, _) => format!("{name}#{index}"),
                });
                let free = candidates
                    .into_iter()
                    .find(|key| !taken.contains(key))
                    .unwrap_or_default();
                let _fresh = taken.insert(free.clone());
                free
            });
            let info = tehuti_core::dispatch::ControllerInfo {
                name,
                key,
                label: format!("{model} #{count}"),
                connection,
            };
            (path, info)
        })
        .collect()
}

/// [`keyed_controllers`] without the node paths.
#[must_use]
pub fn numbered_controllers(
    devices: impl IntoIterator<Item = ConnectedPad>,
    known: &std::collections::BTreeMap<std::path::PathBuf, String>,
) -> Vec<tehuti_core::dispatch::ControllerInfo> {
    keyed_controllers(devices, known)
        .into_iter()
        .map(|(_, info)| info)
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
                let mut translator = DpadHat::for_node(
                    device.input_id().vendor(),
                    device.input_id().product(),
                    device.name().unwrap_or_default(),
                );
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

/// True while a Wii remote's nunchuk extension node exists; it has no south button, so it is
/// never opened as a pad, and its name is read from sysfs instead.
#[must_use]
pub fn nunchuk_present() -> bool {
    std::fs::read_dir("/sys/class/input")
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            std::fs::read_to_string(entry.path().join("name"))
                .is_ok_and(|name| name.trim() == "Nintendo Wii Remote Nunchuk")
        })
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

/// Whether discovery accepts a physical pad or an explicitly named virtual pad, excluding our mirror.
#[must_use]
pub fn accepts_pad(
    bus: evdev::BusType,
    sysfs: Option<&std::path::Path>,
    name: Option<&str>,
    virtual_pad: Option<&str>,
) -> bool {
    name != Some(VIRTUAL_PAD_NAME)
        && (is_physical(bus, sysfs)
            || virtual_pad.is_some_and(|substring| {
                !substring.is_empty() && name.is_some_and(|name| name.contains(substring))
            }))
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

pub(crate) fn timestamp_us(time: std::time::SystemTime) -> u128 {
    time.duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
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
    /// The physical pad failed a read; it feeds nothing until the event loop drops it.
    lost: bool,
    first_emit_after_focus: Option<std::time::Instant>,
    plugin_held: Vec<ButtonName>,
    buffered: Vec<Event>,
}

/// The virtual controller presented to the application, with inputs kept from it.
struct Mirror {
    device: Box<dyn MirrorSink>,
    copy: MirrorCopy,
    /// Stick and trigger axes with their resting values in the virtual pad range.
    centres: Vec<(u16, i32)>,
}

impl Mirror {
    /// A frame that lets go of every held key and hat and rests every other axis.
    fn rest(&mut self) -> Option<Vec<evdev::InputEvent>> {
        let mut frame = self.copy.release().unwrap_or_default();
        let _sync = frame.pop();
        frame.extend(
            self.centres
                .iter()
                .map(|(code, value)| evdev::InputEvent::new(evdev::EventType::ABSOLUTE.0, *code, *value)),
        );
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
}

/// A virtual controller outliving its physical pad, so the application never sees it unplugged.
///
/// It keeps the capabilities of the pad it was made for, even when it later carries another model.
#[derive(Debug)]
pub struct VirtualPad(Mirror);

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

/// Why copying pending input stopped.
enum Copy {
    /// The physical pad could not be read.
    Read(io::Error),
    /// The virtual pad could not be written.
    Write(io::Error),
}

/// Parks `kept` as the spare unless one is already parked; the extra virtual pad is dropped.
pub fn keep_spare(spare: &mut Option<VirtualPad>, kept: VirtualPad) {
    if spare.is_some() {
        warn!("a spare virtual pad is already parked; dropping the extra one");
    } else {
        *spare = Some(kept);
    }
}

/// What [`sync_mirrors`] changed.
#[derive(Debug, Default)]
pub struct MirrorSync {
    /// A virtual pad was just created.
    pub started: bool,
    /// Pads whose virtual pad could not be created.
    pub failed: Vec<std::path::PathBuf>,
}

/// Keeps exactly one virtual pad while any pad is held: mirroring on the active pad, at rest otherwise.
///
/// The pad that holds it is the active one, or the first pad while none is active, so the virtual pad
/// exists before the first event and outside passthrough contexts; a spare is kept for the next pad.
/// Old mirrors go first, releasing what they hold, so the application never sees two.
pub fn sync_mirrors<D: PadDevice>(
    pads: &mut [(&std::path::Path, &mut EvdevPad<D>)],
    mirrored: bool,
    active: Option<&std::path::Path>,
    spare: &mut Option<VirtualPad>,
    consumed: &dyn Fn(ButtonName) -> bool,
) -> MirrorSync {
    let mut sync = MirrorSync::default();
    let holder = pads
        .iter()
        .map(|(path, _)| *path)
        .find(|path| active == Some(*path))
        .or_else(|| pads.first().map(|(path, _)| *path))
        .map(std::path::Path::to_path_buf);
    for (path, pad) in pads.iter_mut() {
        if !mirror_wanted(mirrored, active, path) {
            if let Err(error) = pad.set_mirror(false, consumed) {
                warn!(%error, "virtual pad release failed");
            }
            if holder.as_deref() != Some(*path) {
                // Another pad holds the virtual pad; a parked one would make two.
                pad.unpark_mirror();
                continue;
            }
            if let Some(kept) = spare.take() {
                pad.adopt_virtual_pad(kept);
            }
            match pad.park_virtual_pad() {
                Ok(created) => sync.started |= created,
                Err(error) => {
                    // Physical passthrough follows from `mirror_unavailable` once a passthrough context has focus.
                    pad.mirror_failed = true;
                    warn!(%error, "virtual pad unavailable");
                }
            }
        }
    }
    for (path, pad) in pads.iter_mut() {
        if !mirror_wanted(mirrored, active, path) {
            continue;
        }
        if let Some(kept) = spare.take() {
            pad.adopt_virtual_pad(kept);
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

fn gamepad_key(code: u16) -> bool {
    (KeyCode::BTN_SOUTH.code()..=KeyCode::BTN_THUMBR.code()).contains(&code)
        || (KeyCode::BTN_DPAD_UP.code()..=KeyCode::BTN_DPAD_RIGHT.code()).contains(&code)
}

fn gamepad_event(event: &evdev::InputEvent) -> bool {
    match event.event_type() {
        evdev::EventType::KEY => gamepad_key(event.code()),
        evdev::EventType::ABSOLUTE => true,
        _ => false,
    }
}

/// Which raw events reach the virtual controller, frame by frame.
#[derive(Debug, Default)]
struct MirrorCopy {
    blocked: Vec<(u16, u16)>,
    frame: Vec<evdev::InputEvent>,
    buffered: Vec<Event>,
    /// Keys and hat axes the virtual controller currently shows as held.
    held: Vec<(u16, u16)>,
    /// Menu and View as the physical pad holds them, for the Guide chord.
    menu_down: bool,
    view_down: bool,
    /// The key whose press completed the chord and went to the stream as Guide instead.
    chord_key: Option<(u16, u16)>,
    chord_live: bool,
}

impl MirrorCopy {
    /// Menu + View together reach the stream as Guide, held as long as the chord is; the completing button stays out.
    /// Returns whether `key`'s own event is kept from the stream.
    fn chord(&mut self, name: ButtonName, state: ButtonState, key: (u16, u16)) -> bool {
        if name != ButtonName::Start && name != ButtonName::Select {
            return false;
        }
        let menu = name == ButtonName::Start;
        let guide = (evdev::EventType::KEY.0, KeyCode::BTN_MODE.code());
        let mut kept = self.chord_key == Some(key);
        match state {
            ButtonState::Pressed => {
                if menu {
                    self.menu_down = true;
                } else {
                    self.view_down = true;
                }
                if self.menu_down && self.view_down && self.chord_key.is_none() {
                    self.chord_key = Some(key);
                    self.chord_live = true;
                    self.held.retain(|held| *held != guide);
                    self.held.push(guide);
                    self.frame.push(evdev::InputEvent::new(guide.0, guide.1, 1));
                    kept = true;
                }
            }
            ButtonState::Released => {
                if menu {
                    self.menu_down = false;
                } else {
                    self.view_down = false;
                }
                if self.chord_live {
                    self.chord_live = false;
                    self.held.retain(|held| *held != guide);
                    self.frame.push(evdev::InputEvent::new(guide.0, guide.1, 0));
                }
                if self.chord_key == Some(key) {
                    self.chord_key = None;
                }
            }
        }
        kept
    }

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
                && self.chord(*name, *state, key)
            {
                kept = true;
            } else if let Event::Button { name, state } = input
                && consume(*name, *state)
            {
                kept = true;
                self.blocked.retain(|blocked| *blocked != key);
                if *state == ButtonState::Pressed {
                    self.blocked.push(key);
                }
            }
        }
        if kept && event.value() == 0 && self.held.contains(&key) {
            self.held.retain(|held| *held != key);
            self.frame.push(event);
        }
        if !kept && gamepad_event(&event) {
            let held_type = event.event_type() == evdev::EventType::KEY
                || (event.event_type() == evdev::EventType::ABSOLUTE
                    && (event.code() == AbsoluteAxisCode::ABS_HAT0X.0
                        || event.code() == AbsoluteAxisCode::ABS_HAT0Y.0));
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
        for event in state
            .into_iter()
            .filter(|event| event.value() != 0 && gamepad_event(event))
        {
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
        self.menu_down = false;
        self.view_down = false;
        self.chord_key = None;
        self.chord_live = false;
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

/// The name the virtual controller shows, the same for every source pad.
pub const VIRTUAL_PAD_NAME: &str = "Tehuti virtual pad";

/// The physical path the virtual controller reports, the same for every source pad.
pub const VIRTUAL_PAD_PHYS: &core::ffi::CStr = c"tehuti/virtual-pad";

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

/// The Xbox axes and ranges advertised to applications, whatever the source pad reports.
#[must_use]
pub fn virtual_pad_axes() -> Vec<(AbsoluteAxisCode, evdev::AbsInfo)> {
    XBOX_AXES
        .iter()
        .map(|(axis, minimum, maximum)| (*axis, evdev::AbsInfo::new(0, *minimum, *maximum, 0, 0, 0)))
        .collect()
}

const XBOX_AXES: [(AbsoluteAxisCode, i32, i32); 8] = [
    (AbsoluteAxisCode::ABS_X, -32768, 32767),
    (AbsoluteAxisCode::ABS_Y, -32768, 32767),
    (AbsoluteAxisCode::ABS_Z, 0, 255),
    (AbsoluteAxisCode::ABS_RX, -32768, 32767),
    (AbsoluteAxisCode::ABS_RY, -32768, 32767),
    (AbsoluteAxisCode::ABS_RZ, 0, 255),
    (AbsoluteAxisCode::ABS_HAT0X, -1, 1),
    (AbsoluteAxisCode::ABS_HAT0Y, -1, 1),
];

/// Maps `value` from the `source` range onto the `target` range.
///
/// Minimum, midpoint and maximum land exactly on their targets, the map never decreases,
/// and values outside the source range clamp. An empty or inverted source gives the target midpoint.
#[must_use]
pub fn scale_axis(value: i32, source: (i32, i32), target: (i32, i32)) -> i32 {
    let target_mid = target.0.midpoint(target.1);
    if source.1 <= source.0 {
        return target_mid;
    }
    let source_mid = source.0.midpoint(source.1);
    let value = value.clamp(source.0, source.1);
    // One straight segment each side of the midpoint, in i128 so no product can overflow.
    let segment = |input: (i32, i32), output: (i32, i32)| -> Option<i128> {
        let span = i128::from(input.1).checked_sub(i128::from(input.0))?;
        if span == 0 {
            return Some(i128::from(output.0));
        }
        let offset = i128::from(value).checked_sub(i128::from(input.0))?;
        let reach = i128::from(output.1).checked_sub(i128::from(output.0))?;
        i128::from(output.0).checked_add(offset.checked_mul(reach)?.checked_div(span)?)
    };
    let scaled = if value <= source_mid {
        segment((source.0, source_mid), (target.0, target_mid))
    } else {
        segment((source_mid, source.1), (target_mid, target.1))
    };
    scaled
        .and_then(|scaled| i32::try_from(scaled).ok())
        .map_or(target_mid, |scaled| scaled.clamp(target.0, target.1))
}

/// Scales a source stick or trigger value into the advertised Xbox range.
/// Hats and axes without a valid source range pass through unchanged.
#[must_use]
pub fn virtual_pad_value(
    source: &[(AbsoluteAxisCode, evdev::AbsInfo)],
    event: evdev::InputEvent,
) -> evdev::InputEvent {
    if event.event_type() != evdev::EventType::ABSOLUTE {
        return event;
    }
    let axis = AbsoluteAxisCode(event.code());
    if axis == AbsoluteAxisCode::ABS_HAT0X || axis == AbsoluteAxisCode::ABS_HAT0Y {
        return event;
    }
    let Some((_, minimum, maximum)) = XBOX_AXES.iter().find(|(code, _, _)| *code == axis) else {
        return event;
    };
    let Some((_, info)) = source.iter().find(|(code, _)| *code == axis) else {
        return event;
    };
    if info.maximum() <= info.minimum() {
        return event;
    }
    let value = scale_axis(
        event.value(),
        (info.minimum(), info.maximum()),
        (*minimum, *maximum),
    );
    evdev::InputEvent::new(event.event_type().0, event.code(), value)
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
    /// The source pad's calibration, for scaling into the advertised Xbox ranges.
    axes: Vec<(AbsoluteAxisCode, evdev::AbsInfo)>,
}

impl MirrorSink for VirtualPadSink {
    fn emit(&mut self, frame: &[evdev::InputEvent]) -> io::Result<()> {
        let frame: Vec<_> = frame
            .iter()
            .map(|event| virtual_pad_value(&self.axes, virtual_pad_event(&self.source, *event)))
            .collect();
        self.device.emit(&frame)
    }
}

/// Kernel operations used by a controller client.
pub trait PadDevice {
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
    /// Reads one pending raw batch without translating it; an empty batch means drained.
    /// # Errors
    /// The client could not read events.
    fn fetch_raw(&mut self) -> io::Result<Vec<evdev::InputEvent>> {
        Ok(Vec::new())
    }
}

impl PadDevice for Device {
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
            .name(VIRTUAL_PAD_NAME)
            .input_id(virtual_pad_identity())
            .with_phys(VIRTUAL_PAD_PHYS)
            .map_err(|error| io::Error::new(error.kind(), format!("uinput with_phys: {error}")))?
            .with_keys(&keys)
            .map_err(|error| io::Error::new(error.kind(), format!("uinput with_keys: {error}")))?;
        let source_axes: Vec<_> = Self::get_absinfo(self)?.collect();
        for (axis, info) in virtual_pad_axes() {
            builder = builder
                .with_absolute_axis(&evdev::UinputAbsSetup::new(axis, info))
                .map_err(|error| io::Error::new(error.kind(), format!("uinput axis {axis:?}: {error}")))?;
        }
        Ok(Box::new(VirtualPadSink {
            device: builder
                .build()
                .map_err(|error| io::Error::new(error.kind(), format!("uinput build: {error}")))?,
            source: self.input_id(),
            axes: source_axes,
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

    /// The unique id the pad reports, else its physical path, else nothing.
    #[must_use]
    pub fn device_identity(&self) -> String {
        let events = &self.handles.events;
        events
            .unique_name()
            .filter(|text| !text.is_empty())
            .or_else(|| events.physical_path().filter(|text| !text.is_empty()))
            .unwrap_or_default()
            .to_owned()
    }

    /// How the pad reaches the machine: the bus it reports, or a known radio dongle.
    #[must_use]
    pub fn connection(&self) -> tehuti_core::dispatch::Connection {
        use tehuti_core::dispatch::Connection;
        let input_id = self.handles.events.input_id();
        // The Xbox wireless adapter's product ids.
        let dongle = input_id.vendor() == 0x045e && matches!(input_id.product(), 0x02e6 | 0x02fe | 0x02f9);
        match input_id.bus_type().0 {
            0x05 => Connection::Bluetooth,
            _ if dongle => Connection::Dongle,
            _ => Connection::Usb,
        }
    }

    #[must_use]
    pub fn usb_id(&self) -> (u16, u16) {
        let input_id = self.handles.events.input_id();
        (input_id.vendor(), input_id.product())
    }

    #[must_use]
    pub const fn identity(&self) -> tehuti_core::pad::Identity {
        self.translator.identity
    }

    /// Tells a Wii remote whether another pad or a nunchuk is attached beside it.
    pub const fn set_companion(&mut self, companion: bool) {
        self.translator.set_companion(companion);
    }
}

impl<D: PadDevice> EvdevPad<D> {
    /// Presents or removes the virtual controller; true when it was just created.
    /// A new one starts out holding what the physical pad holds.
    /// Removal first releases whatever the virtual controller still holds.
    /// `consumed` asks, without changing anything, whether a plugin holds a button; those are not shown.
    /// # Errors
    /// The kernel refused to create the uinput device.
    pub fn set_mirror(&mut self, present: bool, consumed: &dyn Fn(ButtonName) -> bool) -> io::Result<bool> {
        if present != self.mirror.is_some() {
            tracing::debug!(target: "pad_latency", stage = "focus", at_us = timestamp_us(std::time::SystemTime::now()), present);
            self.first_emit_after_focus = present.then(std::time::Instant::now);
        }
        if !present {
            if let Some(mut mirror) = self.mirror.take() {
                // Outside passthrough the virtual pad shows rest: nothing held, sticks centred.
                if let Some(frame) = mirror.rest() {
                    mirror.device.emit(&frame)?;
                }
                self.parked = Some(mirror);
            }
            return Ok(false);
        }
        if self.mirror.is_some() || self.mirror_failed || self.lost {
            return Ok(false);
        }
        let (device, centres) = if let Some(parked) = self.parked.take() {
            (parked.device, parked.centres)
        } else {
            (self.handles.events.virtual_pad()?, Self::centres())
        };
        let mut mirror = Mirror {
            device,
            copy: MirrorCopy::default(),
            centres,
        };
        // The press that made this pad active was read before the mirror existed; show what is held now.
        let probe = self.handles.probe();
        let keys = probe
            .get_key_state()?
            .into_iter()
            .map(|code| evdev::InputEvent::new(evdev::EventType::KEY.0, code, 1));
        let hats = probe
            .get_absinfo()?
            .into_iter()
            .filter(|(axis, _)| *axis == AbsoluteAxisCode::ABS_HAT0X || *axis == AbsoluteAxisCode::ABS_HAT0Y)
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

    /// Creates the virtual controller hidden and at rest, unless it exists, failed or the pad is lost;
    /// true when it was just created.
    /// # Errors
    /// The kernel refused to create the uinput device.
    pub fn park_virtual_pad(&mut self) -> io::Result<bool> {
        if self.mirror.is_some() || self.parked.is_some() || self.mirror_failed || self.lost {
            return Ok(false);
        }
        let mut mirror = Mirror {
            device: self.handles.events.virtual_pad()?,
            copy: MirrorCopy::default(),
            centres: Self::centres(),
        };
        if let Some(frame) = mirror.rest() {
            mirror.device.emit(&frame)?;
        }
        self.parked = Some(mirror);
        Ok(true)
    }

    /// Stick and trigger axes of the virtual pad with their resting values.
    fn centres() -> Vec<(u16, i32)> {
        XBOX_AXES
            .iter()
            .filter(|(axis, _, _)| {
                *axis != AbsoluteAxisCode::ABS_HAT0X && *axis != AbsoluteAxisCode::ABS_HAT0Y
            })
            .map(|(axis, minimum, maximum)| (axis.0, resting_value(*axis, *minimum, *maximum)))
            .collect()
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

    /// Hands over the virtual controller when the physical pad goes away.
    /// It lets go of every held button and rests every axis first, so nothing stays pressed.
    pub fn detach_virtual_pad(&mut self) -> Option<VirtualPad> {
        let mut mirror = self.mirror.take().or_else(|| self.parked.take())?;
        if let Some(frame) = mirror.rest()
            && let Err(error) = mirror.device.emit(&frame)
        {
            warn!(%error, "virtual pad release failed");
        }
        Some(VirtualPad(mirror))
    }

    /// Takes a virtual controller from a pad that went away, so the next mirror reuses it.
    /// A pad that already has one keeps its own, and the offered one is dropped.
    pub fn adopt_virtual_pad(&mut self, kept: VirtualPad) {
        if self.mirror.is_none() && self.parked.is_none() {
            self.parked = Some(kept.0);
        }
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
    ///
    /// The first failure loses the physical pad: the virtual pad rests, is kept for the next pad,
    /// plugin buttons are released, and later calls do nothing, so the failure is reported once.
    pub fn pump_mirror(
        &mut self,
        consume: &mut dyn FnMut(ButtonName, ButtonState) -> bool,
    ) -> io::Result<()> {
        if self.lost {
            return Ok(());
        }
        match self.copy_pending(&mut *consume) {
            Ok(()) => Ok(()),
            Err(Copy::Read(error)) => {
                self.lose(consume);
                Err(error)
            }
            Err(Copy::Write(error)) => {
                // The physical pad still works; only the virtual one is suspect, so it is not kept.
                warn!(%error, "virtual pad write failed; passing the physical pad through");
                self.mirror = None;
                self.mirror_failed = true;
                for button in core::mem::take(&mut self.plugin_held) {
                    let _kept = consume(button, ButtonState::Released);
                }
                Ok(())
            }
        }
    }

    /// Whether a failed read lost this physical pad.
    #[must_use]
    pub const fn lost(&self) -> bool {
        self.lost
    }

    fn lose(&mut self, consume: &mut dyn FnMut(ButtonName, ButtonState) -> bool) {
        self.lost = true;
        if let Some(mut mirror) = self.mirror.take() {
            if let Some(frame) = mirror.rest()
                && let Err(error) = mirror.device.emit(&frame)
            {
                warn!(%error, "virtual pad release failed");
            }
            self.parked = Some(mirror);
        }
        for button in core::mem::take(&mut self.plugin_held) {
            let _kept = consume(button, ButtonState::Released);
        }
    }

    fn copy_pending(&mut self, consume: &mut dyn FnMut(ButtonName, ButtonState) -> bool) -> Result<(), Copy> {
        if self.mirror.is_none() && self.plugin_held.is_empty() {
            return Ok(());
        }
        loop {
            let raw = self.handles.events().fetch_raw().map_err(Copy::Read)?;
            if raw.is_empty() {
                break;
            }
            let read_at = std::time::SystemTime::now();
            for event in raw {
                tracing::debug!(target: "pad_latency", stage = "read", at_us = timestamp_us(read_at), physical_us = timestamp_us(event.timestamp()));
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
                        mirror.device.emit(&frame).map_err(Copy::Write)?;
                        let first_after_focus = self.first_emit_after_focus.take();
                        tracing::debug!(target: "pad_latency", stage = "emit", at_us = timestamp_us(std::time::SystemTime::now()), read_us = timestamp_us(read_at), physical_us = timestamp_us(event.timestamp()), first_after_focus = first_after_focus.is_some(), since_focus_us = first_after_focus.map_or(0, |focus| focus.elapsed().as_micros()));
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
            lost: false,
            first_emit_after_focus: None,
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

/// Where an axis rests untouched: triggers at their minimum, sticks in the middle.
#[must_use]
pub fn resting_value(axis: AbsoluteAxisCode, minimum: i32, maximum: i32) -> i32 {
    let trigger = [
        AbsoluteAxisCode::ABS_Z,
        AbsoluteAxisCode::ABS_RZ,
        AbsoluteAxisCode::ABS_GAS,
        AbsoluteAxisCode::ABS_BRAKE,
    ]
    .contains(&axis);
    if trigger {
        minimum
    } else {
        minimum.midpoint(maximum)
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

    /// Like [`Self::for_device`], with the node name that tells a Wii remote's buttons node from
    /// the extension nodes that share its id.
    #[must_use]
    pub fn for_node(vendor: u16, product: u16, name: &str) -> Self {
        Self {
            identity: tehuti_core::pad::Identity::for_node(vendor, product, name),
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

    /// Switches a Wii remote between its alone and paired layouts; other pads ignore it.
    pub const fn set_companion(&mut self, companion: bool) {
        self.identity = self.identity.with_companion(companion);
    }

    /// Moves a key onto the position its pad family reads it from.
    ///
    /// The Wii remote is held sideways, so its d-pad turns a quarter turn anticlockwise (its
    /// up arm points left). Alone, 1 confirms, 2 goes back and A opens the keyboard; with a
    /// companion A confirms, B goes back and 1 opens the keyboard. + is push-to-talk and − View
    /// either way. The leftover buttons map to a code nothing reads.
    const fn face_code(&self, code: KeyCode) -> KeyCode {
        use tehuti_core::pad::PadFamily;
        let unmapped = KeyCode::BTN_TRIGGER_HAPPY1;
        let paired = match self.identity.family {
            PadFamily::Remote => false,
            PadFamily::RemotePaired => true,
            PadFamily::Letters | PadFamily::Symbols | PadFamily::Nintendo => {
                return match (self.identity.swap_north_west, code) {
                    (true, KeyCode::BTN_NORTH) => KeyCode::BTN_WEST,
                    (true, KeyCode::BTN_WEST) => KeyCode::BTN_NORTH,
                    _ => code,
                };
            }
        };
        match code {
            KeyCode::KEY_UP => KeyCode::BTN_DPAD_LEFT,
            KeyCode::KEY_RIGHT => KeyCode::BTN_DPAD_UP,
            KeyCode::KEY_DOWN => KeyCode::BTN_DPAD_RIGHT,
            KeyCode::KEY_LEFT => KeyCode::BTN_DPAD_DOWN,
            KeyCode::KEY_NEXT => KeyCode::KEY_RECORD,
            KeyCode::KEY_PREVIOUS => KeyCode::BTN_SELECT,
            KeyCode::BTN_1 if paired => KeyCode::BTN_NORTH,
            KeyCode::BTN_1 => KeyCode::BTN_SOUTH,
            KeyCode::BTN_2 if paired => unmapped,
            KeyCode::BTN_2 => KeyCode::BTN_EAST,
            KeyCode::BTN_SOUTH if !paired => KeyCode::BTN_NORTH,
            KeyCode::BTN_EAST if !paired => unmapped,
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

/// Back paddles: `BTN_GRIPL`, `BTN_GRIPR`, `BTN_GRIPL2` and `BTN_GRIPR2` (0x224-0x227).
const PADDLE_1: KeyCode = KeyCode::new(0x224);
const PADDLE_2: KeyCode = KeyCode::new(0x225);
const PADDLE_3: KeyCode = KeyCode::new(0x226);
const PADDLE_4: KeyCode = KeyCode::new(0x227);

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
        PADDLE_1 => ButtonName::Paddle1,
        PADDLE_2 => ButtonName::Paddle2,
        PADDLE_3 => ButtonName::Paddle3,
        PADDLE_4 => ButtonName::Paddle4,
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

    struct Counted(std::rc::Rc<core::cell::Cell<usize>>);

    impl super::MirrorSink for Counted {
        fn emit(&mut self, _frame: &[InputEvent]) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.set(self.0.get().saturating_add(1));
        }
    }

    fn virtual_pad(drops: &std::rc::Rc<core::cell::Cell<usize>>) -> super::VirtualPad {
        super::VirtualPad(super::Mirror {
            device: Box::new(Counted(std::rc::Rc::clone(drops))),
            copy: MirrorCopy::default(),
            centres: Vec::new(),
        })
    }

    #[test]
    fn parking_a_spare_keeps_the_one_already_parked() {
        let drops = std::rc::Rc::default();
        let mut spare = Some(virtual_pad(&drops));
        super::keep_spare(&mut spare, virtual_pad(&drops));
        assert!(spare.is_some());
        assert_eq!(drops.get(), 1, "the extra virtual pad is dropped");
        super::keep_spare(&mut None, virtual_pad(&drops));
        assert_eq!(drops.get(), 2);
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

    #[test]
    fn release_forgets_a_half_held_chord() {
        let mut copy = MirrorCopy::default();
        let mut translator = DpadHat::default();
        let mut consume = |_: ButtonName, _: ButtonState| false;
        for event in [key(KeyCode::BTN_START, 1), sync()] {
            let _frame = copy.take(event, &mut translator, &mut consume);
        }
        let _released = copy.release();
        let sent: Vec<_> = [key(KeyCode::BTN_SELECT, 1), sync()]
            .into_iter()
            .filter_map(|event| copy.take(event, &mut translator, &mut consume))
            .flatten()
            .collect();
        assert!(
            sent.iter().all(|event| event.code() != KeyCode::BTN_MODE.code()),
            "a lone View press after a release is no chord: {sent:?}"
        );
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
        let pads = super::numbered_controllers(
            [
                (
                    "/dev/input/event9".into(),
                    "Xbox Wireless Controller".into(),
                    0x045e,
                    0x0b13,
                    String::new(),
                    tehuti_core::dispatch::Connection::Usb,
                ),
                (
                    "/dev/input/event2".into(),
                    "Wireless Controller".into(),
                    0x054c,
                    0x09cc,
                    String::new(),
                    tehuti_core::dispatch::Connection::Usb,
                ),
            ],
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(pads[0].name, "Wireless Controller");
        assert_eq!(pads[0].label, "DUALSHOCK 4 #1");
        assert_eq!(pads[1].name, "Xbox Wireless Controller");
        assert_eq!(pads[1].label, "XBOX CONTROLLER #1");
    }

    #[test]
    fn a_pads_connection_reaches_its_tile_info() {
        use tehuti_core::dispatch::Connection;
        let pads = super::numbered_controllers(
            [(
                "/dev/input/event9".into(),
                "Xbox Wireless Controller".into(),
                0x045e,
                0x0b13,
                String::new(),
                Connection::Bluetooth,
            )],
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(pads[0].connection, Connection::Bluetooth);
    }

    fn pad(path: &str, name: &str, identity: &str) -> super::ConnectedPad {
        (
            path.into(),
            name.into(),
            0x045e,
            0x0b13,
            identity.into(),
            tehuti_core::dispatch::Connection::Usb,
        )
    }

    #[test]
    fn pads_of_different_names_sharing_an_identity_get_different_keys() {
        let pads = super::numbered_controllers(
            [
                pad("/dev/input/event1", "Pad A", "shared"),
                pad("/dev/input/event2", "Pad B", "shared"),
            ],
            &std::collections::BTreeMap::new(),
        );
        assert_ne!(pads[0].key, pads[1].key);
    }

    #[test]
    fn a_duplicate_attaching_never_rekeys_the_pad_already_there() {
        let first = [pad("/dev/input/event5", "Pad", "shared")];
        let before = super::keyed_controllers(first.clone(), &std::collections::BTreeMap::new());
        let known: std::collections::BTreeMap<_, _> = before
            .iter()
            .map(|(path, info)| (path.clone(), info.key.clone()))
            .collect();
        // The newcomer's node sorts before the original's.
        let after = super::keyed_controllers(
            [pad("/dev/input/event2", "Pad", "shared"), first[0].clone()],
            &known,
        );
        let original = after
            .iter()
            .find(|(path, _)| path.ends_with("event5"))
            .map(|(_, info)| &info.key);
        assert_eq!(original, Some(&before[0].1.key));
        assert_ne!(after[0].1.key, after[1].1.key);
    }

    #[test]
    fn matching_models_are_numbered_by_device_path() {
        let pads = super::numbered_controllers(
            [
                (
                    "/dev/input/event9".into(),
                    "Wireless Controller".into(),
                    0x054c,
                    0x05c4,
                    String::new(),
                    tehuti_core::dispatch::Connection::Usb,
                ),
                (
                    "/dev/input/event2".into(),
                    "Wireless Controller".into(),
                    0x054c,
                    0x09cc,
                    String::new(),
                    tehuti_core::dispatch::Connection::Usb,
                ),
            ],
            &std::collections::BTreeMap::new(),
        );
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

#[cfg(test)]
mod remote_tests {
    use super::DpadHat;
    use evdev::{EventType, InputEvent, KeyCode};
    use tehuti_protocol::{ButtonName, ButtonState, Event};

    fn remote() -> DpadHat {
        DpadHat::for_node(0x057e, 0x0306, "Nintendo Wii Remote")
    }

    fn press(hat: &mut DpadHat, code: KeyCode) -> Vec<Event> {
        hat.translate(&InputEvent::new(EventType::KEY.0, code.code(), 1).destructure())
    }

    #[test]
    fn back_paddles_map_from_their_evdev_codes() {
        for (code, expected) in [
            (0x224, "Paddle1"),
            (0x225, "Paddle2"),
            (0x226, "Paddle3"),
            (0x227, "Paddle4"),
        ] {
            assert_eq!(
                format!("{:?}", super::parse_button(KeyCode::new(code), 1)),
                format!("Some(Button {{ name: {expected}, state: Pressed }})"),
                "{code:#x}"
            );
        }
    }

    #[test]
    fn the_sideways_remote_reads_its_buttons_from_the_standard_positions() {
        let mut hat = remote();
        for (code, name) in [
            (KeyCode::BTN_1, ButtonName::South),
            (KeyCode::BTN_2, ButtonName::East),
            (KeyCode::BTN_SOUTH, ButtonName::North),
            (KeyCode::KEY_NEXT, ButtonName::Record),
            (KeyCode::KEY_PREVIOUS, ButtonName::Select),
            (KeyCode::BTN_MODE, ButtonName::Guide),
        ] {
            assert_eq!(
                press(&mut hat, code),
                vec![Event::Button {
                    name,
                    state: ButtonState::Pressed
                }],
                "{code:?}"
            );
        }
    }

    #[test]
    fn with_a_companion_the_remote_confirms_on_a_and_goes_back_on_b() {
        let mut hat = remote();
        hat.set_companion(true);
        for (code, name) in [
            (KeyCode::BTN_SOUTH, ButtonName::South),
            (KeyCode::BTN_EAST, ButtonName::East),
            (KeyCode::BTN_1, ButtonName::North),
            (KeyCode::KEY_NEXT, ButtonName::Record),
            (KeyCode::KEY_PREVIOUS, ButtonName::Select),
        ] {
            assert_eq!(
                press(&mut hat, code),
                vec![Event::Button {
                    name,
                    state: ButtonState::Pressed
                }],
                "{code:?}"
            );
        }
        assert_eq!(press(&mut hat, KeyCode::BTN_2), Vec::<Event>::new());
        hat.set_companion(false);
        assert_eq!(press(&mut hat, KeyCode::BTN_EAST), Vec::<Event>::new());
    }

    #[test]
    fn the_sideways_remote_turns_its_dpad_a_quarter_turn_anticlockwise() {
        let mut hat = remote();
        for (code, name) in [
            (KeyCode::KEY_UP, ButtonName::DpadLeft),
            (KeyCode::KEY_RIGHT, ButtonName::DpadUp),
            (KeyCode::KEY_DOWN, ButtonName::DpadRight),
            (KeyCode::KEY_LEFT, ButtonName::DpadDown),
        ] {
            assert_eq!(
                press(&mut hat, code),
                vec![Event::Button {
                    name,
                    state: ButtonState::Pressed
                }],
                "{code:?}"
            );
        }
    }

    #[test]
    fn a_classic_controller_on_the_remote_keeps_its_own_buttons() {
        let mut hat = DpadHat::for_node(0x057e, 0x0306, "Nintendo Wii Remote Classic Controller");
        assert_eq!(
            press(&mut hat, KeyCode::BTN_SOUTH),
            vec![Event::Button {
                name: ButtonName::South,
                state: ButtonState::Pressed
            }]
        );
    }
}
