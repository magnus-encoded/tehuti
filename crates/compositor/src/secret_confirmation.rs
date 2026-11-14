//! Confirmation, provider lookup and delivery for one focus lifetime.

use core::time::Duration;

struct RedactedSecretProvider(Box<dyn tehuti_core::secret::SecretProvider>);

impl core::fmt::Debug for RedactedSecretProvider {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("SecretProvider(<redacted>)")
    }
}

#[derive(Debug, Default)]
struct SecretConfirmationState {
    frame_submitted: bool,
    armed: bool,
    confirm_pressed: bool,
}

#[derive(Debug)]
struct SecretLookupResult {
    provider: RedactedSecretProvider,
    authorization: tehuti_core::secret::AuthorizedSecretLookup,
    value: Result<tehuti_core::secret::SecretValue, tehuti_core::secret::SecretProviderUnavailable>,
}

type SecretLookupReceiver = std::sync::mpsc::Receiver<SecretLookupResult>;

#[derive(Debug)]
pub struct SecretConfirmation<S> {
    gate: tehuti_core::secret::SecretConfirmationGate,
    provider: Option<RedactedSecretProvider>,
    lookup: Option<SecretLookupReceiver>,
    focus_generation: u64,
    surface_id: u64,
    focus_surface: Option<S>,
    confirmation_label: Option<(String, String)>,
    confirmation: SecretConfirmationState,
    notice_until: Option<Duration>,
}

impl<S: PartialEq> Default for SecretConfirmation<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: PartialEq> SecretConfirmation<S> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            gate: tehuti_core::secret::SecretConfirmationGate::new(),
            provider: Some(RedactedSecretProvider(Box::new(
                tehuti_core::secret::DenySecretProvider,
            ))),
            lookup: None,
            focus_generation: 0,
            surface_id: 0,
            focus_surface: None,
            confirmation_label: None,
            confirmation: SecretConfirmationState::default(),
            notice_until: None,
        }
    }

    pub fn set_provider(&mut self, provider: Box<dyn tehuti_core::secret::SecretProvider>) {
        self.provider = Some(RedactedSecretProvider(provider));
    }

    pub fn focus_changed(&mut self, focus: Option<S>) {
        self.focus_generation = self.focus_generation.wrapping_add(1);
        self.gate.cancel();
        self.confirmation_label = None;
        self.confirmation.frame_submitted = false;
        self.confirmation.armed = false;
        if self.focus_surface != focus {
            self.surface_id = self.surface_id.wrapping_add(1);
            self.focus_surface = focus;
        }
    }

    pub const fn confirm_pressed(&mut self, pressed: bool) {
        self.confirmation.confirm_pressed = pressed;
    }

    pub const fn frame_submitted(&mut self) {
        self.confirmation.frame_submitted = true;
        self.confirmation.armed = !self.confirmation.confirm_pressed;
    }

    pub const fn delivery_unavailable(&mut self, now: Duration) {
        self.notice_until = now.checked_add(Duration::from_secs(3));
    }

    #[must_use]
    pub fn unavailable(&self, now: Duration) -> bool {
        self.notice_until.is_some_and(|until| now < until)
    }

    pub fn request(
        &mut self,
        reference: tehuti_protocol::SecretReference,
        target: &str,
        focus: Option<&S>,
        now: Duration,
    ) -> bool {
        if self.lookup.is_some() || self.provider.is_none() {
            return false;
        }
        let Some(destination) = self.current_secret_destination(focus) else {
            return false;
        };
        let entry_label = sanitize_confirmation_label(&reference.entry, 21);
        let requested = self
            .gate
            .request(reference, destination, now, core::time::Duration::from_secs(20))
            .is_some();
        if requested {
            self.confirmation_label = Some((entry_label, sanitize_confirmation_label(target, 29)));
            self.confirmation.frame_submitted = false;
            self.confirmation.armed = false;
        }
        requested
    }

    #[must_use]
    pub fn text(&self, focus: Option<&S>, now: Duration) -> Option<String> {
        let (entry, target) = self.confirmation_label.as_ref()?;
        let destination = self.current_secret_destination(focus)?;
        let _pending = self.gate.pending(now, destination)?;
        Some(format!("CONFIRM SECRET: {entry}\nTARGET: {target}"))
    }

    fn current_secret_destination(
        &self,
        focus: Option<&S>,
    ) -> Option<tehuti_core::secret::SecretDestination> {
        let focus = focus?;
        if self.focus_surface.as_ref() != Some(focus) {
            return None;
        }
        Some(tehuti_core::secret::SecretDestination {
            surface: self.surface_id,
            focus_generation: self.focus_generation,
        })
    }

    pub fn button(&mut self, state: tehuti_protocol::ButtonState, focus: Option<&S>, now: Duration) -> bool {
        let Some(destination) = self.current_secret_destination(focus) else {
            return false;
        };
        if self.gate.pending(now, destination).is_none() {
            return false;
        }
        match state {
            tehuti_protocol::ButtonState::Released => {
                self.confirmation.confirm_pressed = false;
                if self.confirmation.frame_submitted {
                    self.confirmation.armed = true;
                }
            }
            tehuti_protocol::ButtonState::Pressed if self.confirmation.armed => {
                self.confirmation.confirm_pressed = true;
                self.confirmation.armed = false;
                let _consumed = self.confirm_pending_secret(focus, now);
            }
            tehuti_protocol::ButtonState::Pressed => self.confirmation.confirm_pressed = true,
        }
        true
    }

    fn confirm_pending_secret(&mut self, focus: Option<&S>, now: Duration) -> bool {
        let Some(destination) = self.current_secret_destination(focus) else {
            return false;
        };
        let Some(confirmation_id) = self
            .gate
            .pending(now, destination)
            .map(tehuti_core::secret::PendingSecretDelivery::confirmation_id)
        else {
            return false;
        };
        let Some(authorization) = self.gate.confirm(confirmation_id, now, destination) else {
            return true;
        };
        self.confirmation_label = None;
        let Some(mut provider) = self.provider.take() else {
            self.notice_until = now.checked_add(Duration::from_secs(3));
            return true;
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("tehuti-secret-lookup".into())
            .spawn(move || {
                let value = provider.0.lookup(&authorization.reference);
                let _sent = sender.send(SecretLookupResult {
                    provider,
                    authorization,
                    value,
                });
            });
        if spawned.is_err() {
            self.notice_until = now.checked_add(Duration::from_secs(3));
            return true;
        }
        self.lookup = Some(receiver);
        true
    }

    #[must_use]
    pub fn poll(&mut self, focus: Option<&S>, now: Duration) -> Option<Delivery> {
        let Some(receiver) = &self.lookup else {
            return None;
        };
        let lookup_result = match receiver.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.lookup = None;
                self.notice_until = now.checked_add(Duration::from_secs(3));
                return None;
            }
        };
        let lookup = lookup_result?;
        self.lookup = None;
        self.provider = Some(lookup.provider);
        let Some(secret) = lookup.value.ok() else {
            self.notice_until = now.checked_add(Duration::from_secs(3));
            return None;
        };
        if self.current_secret_destination(focus) != Some(lookup.authorization.destination) {
            return None;
        }
        Some(Delivery {
            destination: lookup.authorization.destination,
            secret,
        })
    }
}

/// A secret bound to the live focus at the most recent poll.
/// Deliver synchronously to that surface through the direct keyboard path.
#[derive(Debug)]
pub struct Delivery {
    destination: tehuti_core::secret::SecretDestination,
    secret: tehuti_core::secret::SecretValue,
}

impl Delivery {
    /// Sends the complete value or returns false without sending a prefix.
    /// The sink must send directly to the surface supplied to `poll`.
    #[must_use]
    pub fn deliver<K: Copy, H: core::hash::BuildHasher>(
        self,
        keys: &std::collections::HashMap<char, K, H>,
        send: impl FnMut(K),
    ) -> bool {
        crate::secret_delivery::deliver_if_current(
            self.destination,
            Some(self.destination),
            &self.secret,
            keys,
            send,
        )
    }
}

fn sanitize_confirmation_label(value: &str, maximum: usize) -> String {
    let mut label: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_graphic() || character == ' ' {
                character
            } else {
                '?'
            }
        })
        .take(maximum)
        .collect();
    if value.chars().count() > maximum {
        label.push_str("...");
    }
    if label.is_empty() {
        label.push_str("unnamed");
    }
    label
}
