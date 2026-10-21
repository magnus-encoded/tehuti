//! The controller edge: finds the pad, grabs it, and parses its events into protocol events.

use std::{
    io,
    os::fd::{AsFd, BorrowedFd},
};

use evdev::{AbsoluteAxisCode, Device, EventSummary, KeyCode};
use tehuti_protocol::{ButtonName, ButtonState, Event};
use tracing::warn;

use crate::input::InputSource;

/// Opens and grabs the first pad with a south face button that no other program holds,
/// so only Tehuti sees its input. Pads that fail to grab are logged and skipped.
#[must_use]
pub fn open_grabbed() -> Option<EvdevPad> {
    evdev::enumerate()
        .filter(|(path, device)| {
            let sysfs = path
                .file_name()
                .and_then(|node| std::fs::canonicalize(std::path::Path::new("/sys/class/input").join(node)).ok());
            is_physical(device.input_id().bus_type(), sysfs.as_deref())
        })
        .filter(|(_, device)| {
            device
                .supported_keys()
                .is_some_and(|keys| keys.contains(KeyCode::BTN_SOUTH))
        })
        .find_map(|(path, mut device)| match grab_nonblocking(&mut device) {
            Ok(()) => {
                let translator = DpadHat::for_vendor(device.input_id().vendor());
                tracing::debug!(
                    name = device.name().unwrap_or("unnamed"),
                    vendor = device.input_id().vendor(),
                    face_layout = ?translator.face_layout,
                    "controller face layout selected"
                );
                match Device::open(&path) {
                    Ok(probe) => Some(EvdevPad(PadHandles::new(device, probe), translator)),
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
}

/// Whether a device is real hardware rather than a virtual one.
///
/// Virtual devices, such as uinput pads made by
/// other tools, report `BUS_VIRTUAL` or live under `/sys/devices/virtual`.
/// UHID devices also live there, but `BlueZ` uses UHID for real Bluetooth pads,
/// so `/sys/devices/virtual/misc/uhid` counts as physical.
/// An unknown sysfs path counts as physical so a sysfs-less system still finds pads.
#[must_use]
pub fn is_physical(bus: evdev::BusType, sysfs: Option<&std::path::Path>) -> bool {
    bus != evdev::BusType::BUS_VIRTUAL
        && !sysfs.is_some_and(|path| {
            path.starts_with("/sys/devices/virtual") && !path.starts_with("/sys/devices/virtual/misc/uhid")
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

/// A grabbed kernel evdev controller, such as an Xbox pad on xpad.
#[derive(Debug)]
pub struct EvdevPad(PadHandles<Device>, DpadHat);

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
        self.0.events.name().unwrap_or("unnamed")
    }
    /// Samples the current calibrated stick axes, including a 15 percent deadzone.
    /// # Errors
    /// The kernel could not read the controller axis state.
    pub fn sticks(&self) -> io::Result<tehuti_core::dispatch::Sticks> {
        use evdev::AbsoluteAxisCode;
        let mut sticks = tehuti_core::dispatch::Sticks::default();
        for (axis, info) in self.0.probe().get_absinfo()? {
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
        let keys: Vec<_> = self
            .0
            .probe()
            .get_key_state()?
            .iter()
            .map(KeyCode::code)
            .collect();
        let axes: Vec<_> = self
            .0
            .probe()
            .get_absinfo()?
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

impl AsFd for EvdevPad {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.events.as_fd()
    }
}

impl InputSource for EvdevPad {
    fn release(&mut self) -> io::Result<()> {
        self.0.events().ungrab()
    }

    fn drain(&mut self) -> io::Result<Vec<Event>> {
        drain_device(self.0.events(), &mut self.1)
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

/// Translates controller keys into physical button positions and d-pad hat axes
/// (`ABS_HAT0X`, `ABS_HAT0Y`) into d-pad button events.
#[derive(Debug, Default)]
pub struct DpadHat {
    face_layout: FaceLayout,
    horizontal: i32,
    vertical: i32,
}

#[derive(Debug, Default)]
enum FaceLayout {
    #[default]
    Positional,
    Xbox,
}

impl DpadHat {
    /// Selects the face layout for a controller's USB vendor ID.
    #[must_use]
    pub fn for_vendor(vendor: u16) -> Self {
        Self {
            face_layout: match vendor {
                0x045e => FaceLayout::Xbox,
                _ => FaceLayout::Positional,
            },
            ..Self::default()
        }
    }

    const fn face_code(&self, code: KeyCode) -> KeyCode {
        match (&self.face_layout, code) {
            (FaceLayout::Xbox, KeyCode::BTN_NORTH) => KeyCode::BTN_WEST,
            (FaceLayout::Xbox, KeyCode::BTN_WEST) => KeyCode::BTN_NORTH,
            _ => code,
        }
    }

    /// Translates one evdev event: keys parse as buttons, hat axes as d-pad presses and releases.
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "only keys and the d-pad hat axes matter; every other event kind is ignored"
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
mod tests {
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
        assert!(!is_physical(BusType::BUS_VIRTUAL, Some(usb)), "virtual bus");
        assert!(is_physical(BusType::BUS_USB, Some(usb)), "real Xbox pad");
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

/// Controller discovery state; an attached controller cannot schedule another open.
#[derive(Debug)]
pub enum Discovery {
    Attached,
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
        (Discovery::Attached, DiscoveryInput::Disconnected { now }) => {
            (Discovery::Searching { retry_at: now }, None)
        }
        (Discovery::Searching { retry_at }, DiscoveryInput::Poll { now, rescan })
            if rescan || now >= retry_at =>
        {
            (
                Discovery::Searching {
                    retry_at: now.saturating_add(RETRY_INTERVAL),
                },
                Some(DiscoveryEffect::Scan),
            )
        }
        (Discovery::Searching { .. }, DiscoveryInput::Found) => {
            (Discovery::Attached, Some(DiscoveryEffect::Reconnected))
        }
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
