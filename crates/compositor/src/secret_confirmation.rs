//! Confirmation, provider lookup and delivery for one focus lifetime.

use core::time::Duration;

use tehuti_core::secret_flow::{SecretEffect, SecretFlow, SecretInput};

struct RedactedSecretProvider(Box<dyn tehuti_core::secret::SecretProvider>);

impl core::fmt::Debug for RedactedSecretProvider {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("SecretProvider(<redacted>)")
    }
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
    flow: SecretFlow,
    provider: Option<RedactedSecretProvider>,
    lookup: Option<SecretLookupReceiver>,
    focus_generation: u64,
    surface_id: u64,
    focus_surface: Option<S>,
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
            flow: SecretFlow::new(),
            provider: Some(RedactedSecretProvider(Box::new(
                tehuti_core::secret::DenySecretProvider,
            ))),
            lookup: None,
            focus_generation: 0,
            surface_id: 0,
            focus_surface: None,
        }
    }

    pub fn set_provider(&mut self, provider: Box<dyn tehuti_core::secret::SecretProvider>) {
        self.provider = Some(RedactedSecretProvider(provider));
    }

    pub fn focus_changed(&mut self, focus: Option<S>) {
        self.focus_generation = self.focus_generation.wrapping_add(1);
        if self.focus_surface != focus {
            self.surface_id = self.surface_id.wrapping_add(1);
            self.focus_surface = focus;
        }
        // The destination used here is irrelevant: this input ignores it.
        let _effects = self.flow.step(SecretInput::FocusChanged, Duration::ZERO, None);
    }

    pub fn confirm_pressed(&mut self, pressed: bool) {
        let _effects = self
            .flow
            .step(SecretInput::ButtonHeld(pressed), Duration::ZERO, None);
    }

    pub fn frame_submitted(&mut self, focus: Option<&S>, now: Duration) {
        let current = self.current_secret_destination(focus);
        let _effects = self.flow.step(SecretInput::FramePresented, now, current);
    }

    pub fn delivery_unavailable(&mut self, now: Duration) {
        let _effects = self.flow.step(SecretInput::DeliveryFailed, now, None);
    }

    #[must_use]
    pub fn unavailable(&self, now: Duration) -> bool {
        self.flow.notice_active(now)
    }

    pub fn request(
        &mut self,
        reference: tehuti_protocol::SecretReference,
        target: &str,
        focus: Option<&S>,
        now: Duration,
    ) -> bool {
        let current = self.current_secret_destination(focus);
        let effects = self.flow.step(
            SecretInput::Request {
                reference,
                target: target.into(),
            },
            now,
            current,
        );
        !effects.contains(&SecretEffect::Refused)
    }

    #[must_use]
    pub fn text(&self, focus: Option<&S>, now: Duration) -> Option<String> {
        let prompt = self.flow.prompt(now, self.current_secret_destination(focus))?;
        Some(format!(
            "CONFIRM SECRET: {}\nTARGET: {}",
            prompt.entry, prompt.target
        ))
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
        let current = self.current_secret_destination(focus);
        let effects = self.flow.step(SecretInput::Button(state), now, current);
        let mut consumed = false;
        for effect in effects {
            match effect {
                SecretEffect::ConsumeButton => consumed = true,
                SecretEffect::StartLookup(authorization) => self.start_lookup(authorization, now),
                SecretEffect::Deliver(_) | SecretEffect::Refused => {}
            }
        }
        consumed
    }

    /// Runs the provider lookup off-thread; a failure to start counts as a
    /// failed lookup.
    fn start_lookup(&mut self, authorization: tehuti_core::secret::AuthorizedSecretLookup, now: Duration) {
        let Some(mut provider) = self.provider.take() else {
            self.finish_lookup(false, now, None);
            return;
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
            self.finish_lookup(false, now, None);
            return;
        }
        self.lookup = Some(receiver);
    }

    fn finish_lookup(
        &mut self,
        found: bool,
        now: Duration,
        current: Option<tehuti_core::secret::SecretDestination>,
    ) -> bool {
        self.flow
            .step(SecretInput::LookupFinished { found }, now, current)
            .iter()
            .any(|effect| matches!(effect, SecretEffect::Deliver(_)))
    }

    #[must_use]
    pub fn poll(&mut self, focus: Option<&S>, now: Duration) -> Option<Delivery> {
        let receiver = self.lookup.as_ref()?;
        let lookup = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.lookup = None;
                self.finish_lookup(false, now, None);
                return None;
            }
        };
        self.lookup = None;
        self.provider = Some(lookup.provider);
        let current = self.current_secret_destination(focus);
        let secret = lookup.value.ok();
        let deliver = self.finish_lookup(secret.is_some(), now, current);
        let secret = secret.filter(|_| deliver)?;
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
