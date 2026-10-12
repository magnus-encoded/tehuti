//! One-use confirmation for secret lookups, independent of the store backend.

use core::time::Duration;

use tehuti_protocol::SecretReference;

/// Identifies one compositor focus lifetime, not an application name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretDestination {
    pub surface: u64,
    pub focus_generation: u64,
}

/// Opaque handle presented by the controller confirmation path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmationId(u64);

/// Information safe to show in the confirmation overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSecretDelivery {
    id: ConfirmationId,
    pub reference: SecretReference,
    pub destination: SecretDestination,
    expires_at: Duration,
}

impl PendingSecretDelivery {
    #[must_use]
    pub const fn confirmation_id(&self) -> ConfirmationId {
        self.id
    }
}

/// A confirmed reference bound to one destination lifetime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedSecretLookup {
    pub reference: SecretReference,
    pub destination: SecretDestination,
}

/// A fail-closed confirmation gate. It performs no store or compositor I/O.
#[derive(Debug)]
pub struct SecretConfirmationGate {
    next_id: u64,
    pending: Option<PendingSecretDelivery>,
}

impl SecretConfirmationGate {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_id: 1,
            pending: None,
        }
    }

    /// Starts a confirmation window, replacing any earlier request.
    pub fn request(
        &mut self,
        reference: SecretReference,
        destination: SecretDestination,
        now: Duration,
        lifetime: Duration,
    ) -> Option<ConfirmationId> {
        let id = ConfirmationId(self.next_id);
        self.next_id = self.next_id.checked_add(1)?;
        let expires_at = now.checked_add(lifetime)?;
        self.pending = Some(PendingSecretDelivery {
            id,
            reference,
            destination,
            expires_at,
        });
        Some(id)
    }

    #[must_use]
    pub fn pending(&self, now: Duration, destination: SecretDestination) -> Option<&PendingSecretDelivery> {
        self.pending
            .as_ref()
            .filter(|pending| pending.expires_at > now && pending.destination == destination)
    }

    /// Consumes one fresh controller confirmation or fails closed.
    pub fn confirm(
        &mut self,
        id: ConfirmationId,
        now: Duration,
        destination: SecretDestination,
    ) -> Option<AuthorizedSecretLookup> {
        let pending = self.pending.take()?;
        if pending.id != id || pending.expires_at <= now || pending.destination != destination {
            return None;
        }
        Some(AuthorizedSecretLookup {
            reference: pending.reference,
            destination: pending.destination,
        })
    }

    pub fn cancel(&mut self) {
        self.pending = None;
    }
}

impl Default for SecretConfirmationGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Backend lookup failures intentionally carry no provider or secret text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretProviderUnavailable;

/// Secret-store seam. Implementations must keep values inside the delivery effect.
pub trait SecretProvider: core::fmt::Debug + Send {
    fn lookup(&mut self, reference: &SecretReference) -> Result<SecretValue, SecretProviderUnavailable>;
}

/// A provider result that cannot be cloned, serialized, or formatted for logs.
pub struct SecretValue(String);

impl SecretValue {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Borrows the value only for the duration of the delivery callback.
    pub fn with_value<T>(&self, deliver: impl FnOnce(&str) -> T) -> T {
        deliver(&self.0)
    }
}

/// Default provider used until the owner selects a backend in ADR 0002.
#[derive(Debug, Default)]
pub struct DenySecretProvider;

impl SecretProvider for DenySecretProvider {
    fn lookup(&mut self, _reference: &SecretReference) -> Result<SecretValue, SecretProviderUnavailable> {
        Err(SecretProviderUnavailable)
    }
}
