//! An in-memory secret store behind the provider port, for tests.

use tehuti_protocol::SecretReference;

use crate::secret::{SecretProvider, SecretProviderUnavailable, SecretValue};

/// Entries held in memory; can start locked. Values leave it only as
/// [`SecretValue`].
#[derive(Default)]
pub struct TestSecretStore {
    entries: Vec<(String, Mint)>,
    locked: bool,
}

/// Makes a fresh [`SecretValue`] per lookup; the plaintext lives only in the
/// closure, which has no Debug and zeroizes it on drop.
type Mint = Box<dyn Fn() -> SecretValue + Send>;

impl core::fmt::Debug for TestSecretStore {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TestSecretStore")
            .field("names", &self.entries.iter().map(|(name, _)| name).collect::<Vec<_>>())
            .field("locked", &self.locked)
            .finish()
    }
}

impl TestSecretStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_entry(mut self, name: &str, value: &str) -> Self {
        let value = zeroize::Zeroizing::new(value.to_owned());
        self.entries
            .push((name.to_owned(), Box::new(move || SecretValue::new(String::clone(&value)))));
        self
    }

    #[must_use]
    pub const fn locked(mut self) -> Self {
        self.locked = true;
        self
    }

    #[must_use]
    pub const fn is_locked(&self) -> bool {
        self.locked
    }

    const fn readable(&self) -> Result<(), SecretProviderUnavailable> {
        if self.locked {
            Err(SecretProviderUnavailable)
        } else {
            Ok(())
        }
    }
}

impl SecretProvider for TestSecretStore {
    fn lookup(&mut self, reference: &SecretReference) -> Result<SecretValue, SecretProviderUnavailable> {
        self.readable()?;
        self.entries
            .iter()
            .find(|(name, _)| *name == reference.entry)
            .map(|(_, mint)| mint())
            .ok_or(SecretProviderUnavailable)
    }

    fn list_labels(&mut self) -> Result<Vec<String>, SecretProviderUnavailable> {
        self.readable()?;
        Ok(self.entries.iter().map(|(name, _)| name.clone()).collect())
    }

    fn unlock(&mut self) -> Result<(), SecretProviderUnavailable> {
        self.locked = false;
        Ok(())
    }

    fn lock(&mut self) -> Result<(), SecretProviderUnavailable> {
        self.locked = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::TestSecretStore;
    use crate::secret::SecretProvider;
    use crate::secret_search::SearchContext;
    use tehuti_protocol::SecretReference;

    fn store() -> TestSecretStore {
        TestSecretStore::new()
            .with_entry("Work mail", "pw-one")
            .with_entry("Bank", "pw-two")
    }

    #[test]
    fn search_returns_ranked_names_only() {
        let mut store = store();
        let found = store.search("bank", &SearchContext::default()).expect("search");
        assert_eq!(found, ["Bank"]);
        assert!(!format!("{found:?}").contains("pw-two"));
    }

    #[test]
    fn debug_of_the_store_never_prints_a_value() {
        let text = format!("{:?}", store());
        assert!(!text.contains("pw-one") && !text.contains("pw-two"));
    }

    #[test]
    fn lookup_returns_the_value_redacted_in_debug() {
        let mut store = store();
        let value = store
            .lookup(&SecretReference { entry: "Bank".into() })
            .expect("value");
        let mut delivered = String::new();
        value.deliver_to(|text| delivered.push_str(text));
        assert_eq!(delivered, "pw-two");
        assert!(!format!("{value:?}").contains("pw-two"));
    }

    #[test]
    fn a_locked_store_refuses_search_and_lookup_until_unlocked() {
        let mut store = store().locked();
        let reference = SecretReference { entry: "Bank".into() };
        assert!(store.search("bank", &SearchContext::default()).is_err());
        assert!(store.lookup(&reference).is_err());
        store.unlock().expect("unlock");
        assert!(store.lookup(&reference).is_ok());
        store.lock().expect("lock");
        assert!(store.is_locked());
    }

    #[test]
    fn an_unknown_entry_is_unavailable() {
        let mut store = store();
        assert!(store.lookup(&SecretReference { entry: "nope".into() }).is_err());
    }
}
