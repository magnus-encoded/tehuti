//! A `SecretProvider` backed by the freedesktop Secret Service on the session bus.
//!
//! The service is the default collection of whatever store the desktop runs.
//! Values are fetched over a plain session and handed to the caller as a
//! `SecretValue`; they are never logged and never part of an error.

use core::time::Duration;
use std::thread::sleep;
use std::time::Instant;

use tehuti_core::secret::{SecretProvider, SecretProviderUnavailable, SecretValue};
use tehuti_protocol::SecretReference;
use zbus::blocking::proxy::Builder;
use zbus::blocking::{Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

/// Well-known bus name of the Secret Service.
pub const SERVICE_BUS_NAME: &str = "org.freedesktop.secrets";

/// How long a store prompt may stay unanswered before the call gives up.
pub const DEFAULT_PROMPT_TIMEOUT: Duration = Duration::from_secs(60);

const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE_INTERFACE: &str = "org.freedesktop.Secret.Service";
const COLLECTION_INTERFACE: &str = "org.freedesktop.Secret.Collection";
const ITEM_INTERFACE: &str = "org.freedesktop.Secret.Item";
const PROMPT_INTERFACE: &str = "org.freedesktop.Secret.Prompt";
const SESSION_INTERFACE: &str = "org.freedesktop.Secret.Session";
const NO_OBJECT: &str = "/";
const POLL_INTERVAL: Duration = Duration::from_millis(50);

type Outcome<T> = Result<T, SecretProviderUnavailable>;

/// The `(session, parameters, value, content_type)` struct of `GetSecret`.
type SecretReply = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

/// Reads and locks the default collection of the desktop's secret store.
#[derive(Debug)]
pub struct SecretServiceProvider {
    connection: Connection,
    bus_name: String,
    prompt_timeout: Duration,
}

impl SecretServiceProvider {
    /// Connects to the session bus.
    ///
    /// # Errors
    /// There is no session bus.
    pub fn connect_session_bus() -> Outcome<Self> {
        let connection = Connection::session().map_err(|_| SecretProviderUnavailable)?;
        Ok(Self::new(
            connection,
            SERVICE_BUS_NAME.to_owned(),
            DEFAULT_PROMPT_TIMEOUT,
        ))
    }

    /// Uses an existing connection, a service name and a prompt timeout.
    #[must_use]
    pub const fn new(connection: Connection, bus_name: String, prompt_timeout: Duration) -> Self {
        Self {
            connection,
            bus_name,
            prompt_timeout,
        }
    }

    fn proxy(&self, path: &str, interface: &'static str) -> Outcome<Proxy<'static>> {
        let builder: Builder<'static, Proxy<'static>> = Builder::new(&self.connection);
        builder
            .destination(self.bus_name.clone())
            .and_then(|builder| builder.path(path.to_owned()))
            .and_then(|builder| builder.interface(interface))
            .and_then(|builder| builder.cache_properties(CacheProperties::No).build())
            .map_err(|_| SecretProviderUnavailable)
    }

    fn service(&self) -> Outcome<Proxy<'static>> {
        self.proxy(SERVICE_PATH, SERVICE_INTERFACE)
    }

    fn default_collection(&self) -> Outcome<OwnedObjectPath> {
        let path: OwnedObjectPath = self
            .service()?
            .call("ReadAlias", &("default",))
            .map_err(|_| SecretProviderUnavailable)?;
        if path.as_str() == NO_OBJECT {
            return Err(SecretProviderUnavailable);
        }
        Ok(path)
    }

    fn is_locked(&self, collection: &OwnedObjectPath) -> Outcome<bool> {
        self.proxy(collection.as_str(), COLLECTION_INTERFACE)?
            .get_property("Locked")
            .map_err(|_| SecretProviderUnavailable)
    }

    fn run_prompt(&self, prompt: &OwnedObjectPath) -> Outcome<()> {
        if prompt.as_str() == NO_OBJECT {
            return Ok(());
        }
        self.proxy(prompt.as_str(), PROMPT_INTERFACE)?
            .call::<_, _, ()>("Prompt", &("",))
            .map_err(|_| SecretProviderUnavailable)
    }

    /// Polls because a dismissed prompt and a pending one look the same to the caller.
    fn wait_for_locked(&self, collection: &OwnedObjectPath, wanted: bool) -> Outcome<()> {
        let started = Instant::now();
        loop {
            if self.is_locked(collection)? == wanted {
                return Ok(());
            }
            if started.elapsed() >= self.prompt_timeout {
                return Err(SecretProviderUnavailable);
            }
            sleep(POLL_INTERVAL);
        }
    }

    fn ensure_unlocked(&self, collection: &OwnedObjectPath) -> Outcome<()> {
        if !self.is_locked(collection)? {
            return Ok(());
        }
        let (_unlocked, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = self
            .service()?
            .call("Unlock", &(vec![collection.clone()],))
            .map_err(|_| SecretProviderUnavailable)?;
        self.run_prompt(&prompt)?;
        self.wait_for_locked(collection, false)
    }

    fn labelled_items(&self, collection: &OwnedObjectPath) -> Outcome<Vec<(String, OwnedObjectPath)>> {
        let items: Vec<OwnedObjectPath> = self
            .proxy(collection.as_str(), COLLECTION_INTERFACE)?
            .get_property("Items")
            .map_err(|_| SecretProviderUnavailable)?;
        items
            .into_iter()
            .map(|item| {
                let label: String = self
                    .proxy(item.as_str(), ITEM_INTERFACE)?
                    .get_property("Label")
                    .map_err(|_| SecretProviderUnavailable)?;
                Ok((label, item))
            })
            .collect()
    }

    fn read_item_value(&self, item: &OwnedObjectPath) -> Outcome<SecretValue> {
        let service = self.service()?;
        let (_output, session): (OwnedValue, OwnedObjectPath) = service
            .call("OpenSession", &("plain", Value::from("")))
            .map_err(|_| SecretProviderUnavailable)?;
        let reply: Outcome<(SecretReply,)> = self
            .proxy(item.as_str(), ITEM_INTERFACE)?
            .call("GetSecret", &(&session,))
            .map_err(|_| SecretProviderUnavailable);
        if let Ok(session_proxy) = self.proxy(session.as_str(), SESSION_INTERFACE) {
            drop(session_proxy.call::<_, _, ()>("Close", &()));
        }
        let ((_session, _parameters, value, _content_type),) = reply?;
        let value = String::from_utf8(value).map_err(|_| SecretProviderUnavailable)?;
        Ok(SecretValue::new(value))
    }
}

impl SecretProvider for SecretServiceProvider {
    fn lookup(&mut self, reference: &SecretReference) -> Outcome<SecretValue> {
        let collection = self.default_collection()?;
        self.ensure_unlocked(&collection)?;
        let (_label, item) = self
            .labelled_items(&collection)?
            .into_iter()
            .find(|(label, _item)| *label == reference.entry)
            .ok_or(SecretProviderUnavailable)?;
        self.read_item_value(&item)
    }

    fn list_labels(&mut self) -> Outcome<Vec<String>> {
        let collection = self.default_collection()?;
        self.ensure_unlocked(&collection)?;
        Ok(self
            .labelled_items(&collection)?
            .into_iter()
            .map(|(label, _item)| label)
            .collect())
    }

    fn unlock(&mut self) -> Outcome<()> {
        let collection = self.default_collection()?;
        self.ensure_unlocked(&collection)
    }

    fn lock(&mut self) -> Outcome<()> {
        let collection = self.default_collection()?;
        let (_locked, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = self
            .service()?
            .call("Lock", &(vec![collection.clone()],))
            .map_err(|_| SecretProviderUnavailable)?;
        self.run_prompt(&prompt)?;
        self.wait_for_locked(&collection, true)
    }
}
