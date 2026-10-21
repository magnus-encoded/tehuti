#[cfg(test)]
mod system_menu {
    use tehuti::system_menu::Config;
    use tehuti_core::menu::Command;

    #[test]
    fn commands_use_only_configured_arguments() {
        let config = Config::parse(r#"{"reboot":["example-session","restart","an argument"]}"#).unwrap();
        config
            .run(Command::Reboot, |program, args| {
                assert_eq!(program, "example-session");
                assert_eq!(args, ["restart", "an argument"]);
                Ok(())
            })
            .unwrap();
        assert!(
            config
                .run(Command::LogOut, |_, _| panic!("unconfigured command ran"))
                .is_err()
        );
    }

    #[test]
    fn invalid_config_and_empty_commands_cannot_run() {
        for text in [
            r#"{"unknown":["command"]}"#,
            r#"{"reboot":[""]}"#,
            r#"{"reboot":[1]}"#,
            r#"{"reboot":["bad\u0000name"]}"#,
        ] {
            assert!(Config::parse(text).is_err());
        }
        let config = Config::parse(r#"{"shut_down":[]}"#).unwrap();
        assert!(
            config
                .run(Command::ShutDown, |_, _| panic!("disabled command ran"))
                .is_err()
        );
    }
}
