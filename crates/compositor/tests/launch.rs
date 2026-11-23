//! The launch owner's behaviour through its public seam, with the fake spawner.

#[cfg(test)]
mod launch {
    use core::time::Duration;
    use proptest::prelude::*;
    use std::time::Instant;
    use tehuti::launch::{Exit, Failure, FakeSpawner, Launch, Launches};

    fn launches(exit: Option<Exit>) -> Launches<FakeSpawner> {
        Launches::new(FakeSpawner {
            next_exit: exit,
            ..FakeSpawner::default()
        })
    }

    proptest! {
        #[test]
        fn a_failed_child_is_reported_once_with_its_app_id(
            code in 1..255_i32,
            app_id in "[a-z][a-z.]{0,12}",
            late in 0..30_u64,
        ) {
            let now = Instant::now();
            let mut owner = launches(Some(Exit::Code(code)));
            owner.start(&Launch::new("term", "Term").with_app_id(app_id.clone()), now).unwrap();
            let failures = owner.poll(now.checked_add(Duration::from_secs(late)).unwrap());
            prop_assert_eq!(failures.len(), 1);
            prop_assert_eq!(failures[0].app_id.as_deref(), Some(app_id.as_str()));
            prop_assert_eq!(failures[0].code, Some(code));
            prop_assert_eq!(failures[0].early, late < 5);
            prop_assert_eq!(owner.poll(now), Vec::new());
            prop_assert_eq!(owner.running(), 0);
        }

        #[test]
        fn a_successful_child_is_reaped_without_a_failure(count in 1..16_usize) {
            let now = Instant::now();
            let mut owner = launches(Some(Exit::Success));
            for _ in 0..count {
                owner.start(&Launch::new("app", "App"), now).unwrap();
            }
            prop_assert_eq!(owner.poll(now), Vec::new());
            prop_assert_eq!(owner.running(), 0);
        }
    }

    #[test]
    fn a_running_child_stays_owned_until_it_ends() {
        let now = Instant::now();
        let mut owner = launches(None);
        owner.start(&Launch::new("app", "App"), now).unwrap();
        assert_eq!(owner.poll(now), Vec::new());
        assert_eq!(owner.running(), 1);
    }

    #[test]
    fn a_crash_without_an_exit_code_still_notifies() {
        let now = Instant::now();
        let mut owner = launches(Some(Exit::Crashed));
        owner.start(&Launch::new("app", "App"), now).unwrap();
        let failures = owner.poll(now);
        assert_eq!(
            Failure::summary(&failures).as_deref(),
            Some("App closed right away")
        );
    }

    #[test]
    fn a_program_that_cannot_start_is_an_error_and_not_owned() {
        let mut owner = Launches::new(FakeSpawner {
            refuse: true,
            ..FakeSpawner::default()
        });
        assert!(
            owner
                .start(&Launch::new("missing", "Missing"), Instant::now())
                .is_err()
        );
        assert_eq!(owner.running(), 0);
    }

    #[test]
    fn launches_beyond_capacity_are_refused() {
        let now = Instant::now();
        let mut owner = launches(None);
        for _ in 0..16 {
            owner.start(&Launch::new("app", "App"), now).unwrap();
        }
        assert_eq!(
            owner.start(&Launch::new("app", "App"), now).unwrap_err(),
            "too many pending launches"
        );
    }

    #[test]
    fn several_early_failures_share_one_notice() {
        let now = Instant::now();
        let mut owner = launches(Some(Exit::Code(1)));
        owner.start(&Launch::new("a", "First"), now).unwrap();
        owner.start(&Launch::new("b", "Second"), now).unwrap();
        assert_eq!(
            Failure::summary(&owner.poll(now)).as_deref(),
            Some("2 apps closed right away: First, Second")
        );
        assert_eq!(Failure::summary(&[]), None);
    }

    #[test]
    fn the_launch_carries_its_label_and_app_id() {
        let launch = Launch::new("kitty", "Agent")
            .arg("--class")
            .arg("agent")
            .with_app_id("agent");
        assert_eq!(launch.label(), "Agent");
        assert_eq!(launch.app_id(), Some("agent"));
    }

    #[test]
    fn a_late_failure_is_left_out_of_the_early_exit_notice() {
        let now = Instant::now();
        let after = |seconds| now.checked_add(Duration::from_secs(seconds)).unwrap();
        let mut owner = launches(Some(Exit::Code(1)));
        owner.start(&Launch::new("a", "Slow"), now).unwrap();
        owner.start(&Launch::new("b", "First"), after(59)).unwrap();
        owner.start(&Launch::new("c", "Second"), after(58)).unwrap();
        let failures = owner.poll(after(60));
        assert_eq!(failures.len(), 3);
        assert_eq!(
            Failure::summary(&failures).as_deref(),
            Some("2 apps closed right away: First, Second")
        );
    }
    #[test]
    fn applications_outlive_the_launch_owner() {
        use std::{cell::RefCell, io, rc::Rc};
        use tehuti::launch::{Process, Spawner};

        struct Stub(Rc<RefCell<std::process::Child>>);
        impl Process for Stub {
            fn try_wait(&mut self) -> io::Result<Option<Exit>> {
                Process::try_wait(&mut *self.0.borrow_mut())
            }
            fn terminate(&mut self) {
                Process::terminate(&mut *self.0.borrow_mut());
            }
        }
        struct StubSpawner(Rc<RefCell<std::process::Child>>);
        impl Spawner for StubSpawner {
            type Process = Stub;
            fn spawn(&mut self, _: &Launch) -> io::Result<Stub> {
                Ok(Stub(Rc::clone(&self.0)))
            }
        }
        let child = Rc::new(RefCell::new(
            Launch::new("/bin/sleep", "stub application")
                .arg("30")
                .command()
                .spawn()
                .unwrap(),
        ));
        let mut owner = Launches::new(StubSpawner(Rc::clone(&child)));
        owner
            .start(&Launch::new("stub", "stub application"), Instant::now())
            .unwrap();
        drop(owner);
        let survived = child.borrow_mut().try_wait().unwrap().is_none();
        let _killed = child.borrow_mut().kill();
        let _status = child.borrow_mut().wait().unwrap();
        assert!(
            survived,
            "completing the restore helper must leave its applications running"
        );
    }

}
