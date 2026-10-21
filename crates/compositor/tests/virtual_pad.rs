#![cfg(test)]

use std::path::Path;

use evdev::BusType;
use tehuti::controller::accepts_pad;

#[test]
fn virtual_pad_requires_a_nonempty_matching_name_opt_in() {
    let sysfs = Some(Path::new("/sys/devices/virtual/input/input42/event20"));
    let name = Some("Sunshine (libvirtualhid) X-Box Series Controller");
    for bus in [BusType::BUS_USB, BusType::BUS_VIRTUAL] {
        assert!(!accepts_pad(bus, sysfs, name, None));
        assert!(!accepts_pad(bus, sysfs, name, Some("")));
        assert!(!accepts_pad(bus, sysfs, name, Some("Other pad")));
        assert!(!accepts_pad(bus, sysfs, name, Some("sunshine")));
        assert!(!accepts_pad(bus, sysfs, None, Some("Sunshine")));
        assert!(accepts_pad(bus, sysfs, name, Some("Sunshine")));
    }
}

#[test]
fn physical_pads_remain_accepted_regardless_of_name_opt_in() {
    for opt_in in [None, Some(""), Some("Sunshine")] {
        assert!(accepts_pad(BusType::BUS_USB, None, None, opt_in));
        assert!(accepts_pad(
            BusType::BUS_BLUETOOTH,
            Some(Path::new("/sys/devices/virtual/misc/uhid/input42/event20")),
            Some("Wireless Controller"),
            opt_in,
        ));
    }
}
