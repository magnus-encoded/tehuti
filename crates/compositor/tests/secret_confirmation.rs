//! Secret confirmation traces use generic surface keys and explicit timestamps.

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::{
        collections::HashMap,
        sync::mpsc::{self, Receiver, Sender},
        time::Instant,
    };

    use tehuti::secret_confirmation::{Delivery, SecretConfirmation};
    use tehuti_core::secret::{SecretProvider, SecretProviderUnavailable, SecretValue};
    use tehuti_protocol::{ButtonState, SecretReference};

    const NOW: Duration = Duration::from_secs(1);
    const TARGET: u8 = 7;

    type ProviderResult = Result<SecretValue, SecretProviderUnavailable>;

    struct ControlledProvider {
        started: Sender<()>,
        results: Receiver<ProviderResult>,
    }

    impl SecretProvider for ControlledProvider {
        fn lookup(&mut self, _: &SecretReference) -> ProviderResult {
            self.started.send(()).map_err(|_| SecretProviderUnavailable)?;
            self.results.recv().map_err(|_| SecretProviderUnavailable)?
        }
    }

    struct Flow {
        confirmation: SecretConfirmation<u8>,
        started: Receiver<()>,
        results: Sender<ProviderResult>,
    }

    impl Flow {
        fn new() -> Self {
            let (started_sender, started) = mpsc::channel();
            let (results, result_receiver) = mpsc::channel();
            let mut confirmation = SecretConfirmation::new();
            confirmation.set_provider(Box::new(ControlledProvider {
                started: started_sender,
                results: result_receiver,
            }));
            confirmation.focus_changed(Some(TARGET));
            Self {
                confirmation,
                started,
                results,
            }
        }

        fn request(&mut self, now: Duration) -> bool {
            self.confirmation.request(
                SecretReference {
                    entry: "entry".into(),
                },
                "window",
                Some(&TARGET),
                now,
            )
        }

        fn press(&mut self, state: ButtonState, now: Duration) -> bool {
            self.confirmation.button(state, Some(&TARGET), now)
        }

        fn authorize(&mut self) {
            assert!(self.request(NOW));
            self.confirmation.frame_submitted();
            assert!(self.press(ButtonState::Pressed, NOW));
            self.started.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(self.confirmation.text(Some(&TARGET), NOW).is_none());
        }

        fn complete(&mut self, focus: Option<&u8>, now: Duration) -> Option<Delivery> {
            let started = Instant::now();
            loop {
                let delivery = self.confirmation.poll(focus, now);
                if delivery.is_some() || self.confirmation.unavailable(now) || self.request(now) {
                    return delivery;
                }
                assert!(
                    started.elapsed() < Duration::from_secs(5),
                    "provider result was not polled"
                );
                std::thread::yield_now();
            }
        }
    }

    fn deliver(delivery: Delivery) -> String {
        let keys: HashMap<_, _> = "value".chars().map(|character| (character, character)).collect();
        let mut typed = String::new();
        assert!(delivery.deliver(&keys, |key| typed.push(key)));
        typed
    }

    #[test]
    fn a_displayed_notice_and_a_fresh_press_authorize_one_lookup() {
        let mut flow = Flow::new();
        assert!(flow.request(NOW));
        assert_eq!(
            flow.confirmation.text(Some(&TARGET), NOW).as_deref(),
            Some("CONFIRM SECRET: entry\nTARGET: window")
        );
        assert!(flow.press(ButtonState::Pressed, NOW));
        assert!(flow.started.try_recv().is_err());
        flow.confirmation.frame_submitted();
        assert!(flow.press(ButtonState::Pressed, NOW));
        assert!(flow.started.try_recv().is_err());
        assert!(flow.press(ButtonState::Released, NOW));
        assert!(flow.press(ButtonState::Pressed, NOW));
        flow.started.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!flow.press(ButtonState::Pressed, NOW));
        assert!(!flow.request(NOW));
        flow.results.send(Ok(SecretValue::new("value".into()))).unwrap();
        assert_eq!(deliver(flow.complete(Some(&TARGET), NOW).unwrap()), "value");
        assert!(flow.confirmation.poll(Some(&TARGET), NOW).is_none());
    }

    #[test]
    fn a_button_held_before_the_request_must_be_released_after_the_frame() {
        let mut flow = Flow::new();
        flow.confirmation.confirm_pressed(true);
        assert!(flow.request(NOW));
        flow.confirmation.frame_submitted();
        assert!(flow.press(ButtonState::Pressed, NOW));
        assert!(flow.started.try_recv().is_err());
        assert!(flow.press(ButtonState::Released, NOW));
        assert!(flow.press(ButtonState::Pressed, NOW));
        flow.started.recv_timeout(Duration::from_secs(5)).unwrap();
        flow.results.send(Ok(SecretValue::new("value".into()))).unwrap();
        assert_eq!(deliver(flow.complete(Some(&TARGET), NOW).unwrap()), "value");
    }

    #[test]
    fn focus_change_focus_loss_and_cancel_invalidate_a_pending_confirmation() {
        for focus in [Some(8), None, Some(TARGET)] {
            let mut flow = Flow::new();
            assert!(flow.request(NOW));
            flow.confirmation.frame_submitted();
            flow.confirmation.focus_changed(focus);
            assert!(flow.confirmation.text(focus.as_ref(), NOW).is_none());
            assert!(
                !flow
                    .confirmation
                    .button(ButtonState::Pressed, focus.as_ref(), NOW)
            );
            assert!(flow.started.try_recv().is_err());
        }
    }

    #[test]
    fn timeout_at_the_deadline_hides_the_notice_and_rejects_confirmation() {
        let mut flow = Flow::new();
        assert!(flow.request(NOW));
        flow.confirmation.frame_submitted();
        assert!(
            flow.confirmation
                .text(Some(&TARGET), NOW + Duration::from_secs(19))
                .is_some()
        );
        let deadline = NOW + Duration::from_secs(20);
        assert!(flow.confirmation.text(Some(&TARGET), deadline).is_none());
        assert!(!flow.press(ButtonState::Pressed, deadline));
        assert!(flow.started.try_recv().is_err());
    }

    #[test]
    fn returning_to_the_same_surface_during_lookup_does_not_restore_authorization() {
        for focus in [Some(8), None, Some(TARGET)] {
            let mut flow = Flow::new();
            flow.authorize();
            flow.confirmation.focus_changed(focus);
            flow.confirmation.focus_changed(Some(TARGET));
            flow.results.send(Ok(SecretValue::new("value".into()))).unwrap();
            assert!(flow.complete(Some(&TARGET), NOW).is_none());
            assert!(!flow.confirmation.unavailable(NOW));
        }
    }

    #[test]
    fn an_unreported_focus_change_or_dead_surface_cannot_receive_the_lookup_result() {
        for focus in [Some(8), None] {
            let mut flow = Flow::new();
            flow.authorize();
            flow.results.send(Ok(SecretValue::new("value".into()))).unwrap();
            assert!(flow.complete(focus.as_ref(), NOW).is_none());
            assert!(!flow.confirmation.unavailable(NOW));
        }
    }

    #[test]
    fn a_confirmed_lookup_outlives_the_confirmation_timeout() {
        let mut flow = Flow::new();
        flow.authorize();
        flow.results.send(Ok(SecretValue::new("value".into()))).unwrap();
        assert_eq!(
            deliver(
                flow.complete(Some(&TARGET), NOW + Duration::from_secs(21))
                    .unwrap()
            ),
            "value"
        );
    }

    #[test]
    fn provider_failure_sends_nothing_and_shows_a_three_second_notice_then_allows_retry() {
        let mut flow = Flow::new();
        flow.authorize();
        flow.results.send(Err(SecretProviderUnavailable)).unwrap();
        assert!(flow.complete(Some(&TARGET), NOW).is_none());
        assert!(flow.confirmation.unavailable(NOW));
        assert!(!flow.confirmation.unavailable(NOW + Duration::from_secs(3)));
        assert!(flow.request(NOW + Duration::from_secs(3)));
        flow.confirmation.confirm_pressed(false);
        flow.confirmation.frame_submitted();
        assert!(flow.press(ButtonState::Pressed, NOW + Duration::from_secs(3)));
        flow.started.recv_timeout(Duration::from_secs(5)).unwrap();
        flow.results.send(Ok(SecretValue::new("value".into()))).unwrap();
        assert_eq!(
            deliver(
                flow.complete(Some(&TARGET), NOW + Duration::from_secs(3))
                    .unwrap()
            ),
            "value"
        );
    }

    #[test]
    fn replacement_requests_need_their_own_displayed_frame() {
        let mut flow = Flow::new();
        assert!(flow.request(NOW));
        flow.confirmation.frame_submitted();
        assert!(flow.request(NOW));
        assert!(flow.press(ButtonState::Pressed, NOW));
        assert!(flow.started.try_recv().is_err());
    }

    #[test]
    fn delivery_rejects_an_unrepresentable_value_without_sending_a_prefix() {
        let mut flow = Flow::new();
        flow.authorize();
        flow.results.send(Ok(SecretValue::new("value\n".into()))).unwrap();
        let delivery = flow.complete(Some(&TARGET), NOW).unwrap();
        let keys: HashMap<_, _> = "value".chars().map(|character| (character, character)).collect();
        let mut typed = String::new();
        assert!(!delivery.deliver(&keys, |key| typed.push(key)));
        assert_eq!(typed, "");
    }
}
