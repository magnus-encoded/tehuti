//! The only route that may carry a secret value to a Wayland client.

use std::collections::HashMap;

use tehuti_core::secret::{SecretDestination, SecretValue};

/// Sends a secret only after validating the entire value and its focus lifetime.
///
/// `send` must target the compositor's direct keyboard path. It must not use
/// text-input, plugins, clipboard selection, logging, or ordinary text actions.
pub fn deliver_if_current<K: Copy>(
    authorized: SecretDestination,
    current: Option<SecretDestination>,
    secret: &SecretValue,
    keys: &HashMap<char, K>,
    mut send: impl FnMut(K),
) -> bool {
    if current != Some(authorized) {
        return false;
    }
    secret.with_value(|value| {
        if value.chars().any(char::is_control) {
            return false;
        }
        let Some(prepared) = value
            .chars()
            .map(|character| keys.get(&character).copied())
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        for key in prepared {
            send(key);
        }
        true
    })
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use tehuti_core::secret::{
        DenySecretProvider, SecretConfirmationGate, SecretDestination, SecretProvider, SecretValue,
    };
    use tehuti_protocol::SecretReference;

    use super::deliver_if_current;

    const TARGET: SecretDestination = SecretDestination {
        surface: 23,
        focus_generation: 8,
    };

    #[derive(Debug)]
    struct FakeProvider;

    impl SecretProvider for FakeProvider {
        fn lookup(
            &mut self,
            _reference: &SecretReference,
        ) -> Result<SecretValue, tehuti_core::secret::SecretProviderUnavailable> {
            Ok(SecretValue::new("s3ntinel".into()))
        }
    }

    #[test]
    fn confirmed_fake_secret_uses_only_the_named_destination_and_never_sends_a_prefix() {
        let mut gate = SecretConfirmationGate::new();
        let reference = SecretReference {
            entry: "test-entry".into(),
        };
        let request = gate
            .request(reference.clone(), TARGET, Duration::ZERO, Duration::from_secs(5))
            .unwrap();
        let authorization = gate.confirm(request, Duration::from_secs(1), TARGET).unwrap();
        let mut provider = FakeProvider;
        let secret = provider.lookup(&authorization.reference).unwrap();
        let keys: HashMap<_, _> = "s3ntiel"
            .chars()
            .map(|character| (character, character))
            .collect();
        let mut sent = Vec::new();

        assert!(!deliver_if_current(
            authorization.destination,
            Some(TARGET),
            &secret,
            &keys,
            |key| sent.push(key)
        ));
        assert!(
            sent.is_empty(),
            "unrepresentable secret must not send a partial prefix"
        );

        let keys: HashMap<_, _> = "s3ntinel"
            .chars()
            .map(|character| (character, character))
            .collect();
        let mut sent = Vec::new();
        assert!(!deliver_if_current(
            authorization.destination,
            Some(SecretDestination {
                focus_generation: 9,
                ..TARGET
            }),
            &secret,
            &keys,
            |key| sent.push(key),
        ));
        assert!(sent.is_empty(), "stale focus must receive zero characters");

        assert!(deliver_if_current(
            authorization.destination,
            Some(TARGET),
            &secret,
            &keys,
            |key| sent.push(key)
        ));
        assert_eq!(sent.into_iter().collect::<String>(), "s3ntinel");

        let mut denied = DenySecretProvider;
        assert!(denied.lookup(&authorization.reference).is_err());
    }
}
