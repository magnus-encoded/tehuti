//! Secret lookup authorization is one-use and tied to one focus lifetime.

#[cfg(test)]
mod secret_confirmation {
    use core::time::Duration;

    use proptest::prelude::*;
    use tehuti_core::secret::{
        DenySecretProvider, SecretConfirmationGate, SecretDestination, SecretProvider,
    };
    use tehuti_protocol::SecretReference;

    fn reference(entry: &str) -> SecretReference {
        SecretReference { entry: entry.into() }
    }

    const TARGET: SecretDestination = SecretDestination {
        surface: 17,
        focus_generation: 4,
    };

    #[test]
    fn fresh_confirmation_authorizes_one_lookup_for_the_named_destination() {
        let mut gate = SecretConfirmationGate::new();
        let confirmation_id = gate
            .request(reference("mail"), TARGET, Duration::ZERO, Duration::from_secs(10))
            .unwrap();

        let authorization = gate
            .confirm(confirmation_id, Duration::from_secs(1), TARGET)
            .unwrap();
        assert_eq!(authorization.reference.entry, "mail");
        assert_eq!(authorization.destination, TARGET);
        assert!(
            gate.confirm(confirmation_id, Duration::from_secs(1), TARGET)
                .is_none()
        );
    }

    #[test]
    fn replacing_request_invalidates_old_confirmation() {
        let mut gate = SecretConfirmationGate::new();
        let old = gate
            .request(reference("old"), TARGET, Duration::ZERO, Duration::from_secs(10))
            .unwrap();
        let new = gate
            .request(reference("new"), TARGET, Duration::ZERO, Duration::from_secs(10))
            .unwrap();

        assert!(gate.confirm(old, Duration::from_secs(1), TARGET).is_none());
        assert_eq!(
            gate.confirm(new, Duration::from_secs(1), TARGET)
                .unwrap()
                .reference
                .entry,
            "new"
        );
    }

    proptest! {
        #[test]
        fn stale_confirmations_never_consume_the_latest_replacement(replacements in 1usize..32) {
            let mut gate = SecretConfirmationGate::new();
            let mut stale_ids = Vec::new();
            for index in 0..replacements {
                stale_ids.push(
                    gate.request(
                        reference(&format!("old-{index}")),
                        TARGET,
                        Duration::ZERO,
                        Duration::from_secs(10),
                    )
                    .unwrap(),
                );
            }
            let latest_id = gate
                .request(reference("latest"), TARGET, Duration::ZERO, Duration::from_secs(10))
                .unwrap();

            for stale_id in stale_ids {
                prop_assert!(gate.confirm(stale_id, Duration::from_secs(1), TARGET).is_none());
                prop_assert!(gate.pending(Duration::from_secs(1), TARGET).is_some());
            }
            let authorized = gate.confirm(latest_id, Duration::from_secs(1), TARGET).unwrap();
            prop_assert_eq!(authorized.reference.entry, "latest");
        }
    }

    #[test]
    fn focus_change_expiry_and_cancel_fail_closed() {
        let mut gate = SecretConfirmationGate::new();
        let stale_confirmation_id = gate
            .request(reference("mail"), TARGET, Duration::ZERO, Duration::from_secs(10))
            .unwrap();
        let returned_focus = SecretDestination {
            focus_generation: 5,
            ..TARGET
        };

        assert!(gate.pending(Duration::from_secs(1), returned_focus).is_none());
        assert!(
            gate.confirm(stale_confirmation_id, Duration::from_secs(1), returned_focus)
                .is_none()
        );

        let expired_confirmation_id = gate
            .request(reference("mail"), TARGET, Duration::ZERO, Duration::from_secs(1))
            .unwrap();
        assert!(
            gate.confirm(expired_confirmation_id, Duration::from_secs(1), TARGET)
                .is_none()
        );
        let cancelled_confirmation_id = gate
            .request(reference("mail"), TARGET, Duration::ZERO, Duration::from_secs(10))
            .unwrap();
        gate.cancel();
        assert!(
            gate.confirm(cancelled_confirmation_id, Duration::from_secs(1), TARGET)
                .is_none()
        );
    }

    #[test]
    fn provider_is_unavailable_until_a_backend_is_selected() {
        let mut provider = DenySecretProvider;
        assert!(provider.lookup(&reference("mail")).is_err());
    }
}
