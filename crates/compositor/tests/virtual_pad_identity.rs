#[cfg(test)]
mod tests {
    use evdev::BusType;
    use tehuti::controller::{is_physical, virtual_pad_identity};

    #[test]
    fn virtual_pad_has_a_fixed_xbox_identity_and_is_not_physical() {
        let identity = virtual_pad_identity();
        assert_eq!(identity.bus_type(), BusType::BUS_VIRTUAL);
        assert_eq!(identity.vendor(), 0x045e);
        assert_eq!(identity.product(), 0x028e);
        assert_eq!(identity.version(), 0x0001);
        assert!(!is_physical(identity.bus_type(), None));
        assert!(!is_physical(
            identity.bus_type(),
            Some(std::path::Path::new("/sys/devices/virtual/input/input42/event20")),
        ));
    }
}
