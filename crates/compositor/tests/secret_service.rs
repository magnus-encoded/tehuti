//! The Secret Service provider against an in-process fake service, never a real keyring.

#[cfg(test)]
mod secret_service {
    use core::time::Duration;
    use std::collections::HashSet;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};
    use std::thread;

    use proptest::prelude::*;
    use tehuti::secret_service::{SERVICE_BUS_NAME, SecretServiceProvider};
    use tehuti_core::secret::{SecretProvider, SecretValue};
    use tehuti_protocol::SecretReference;
    use zbus::blocking::{Connection, connection::Builder};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
    use zbus::{Guid, fdo};

    type SecretReply = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

    const COLLECTION_PATH: &str = "/org/freedesktop/secrets/collection/default";
    const PROMPT_PATH: &str = "/org/freedesktop/secrets/prompt/u1";
    const SHORT_PROMPT_TIMEOUT: Duration = Duration::from_millis(300);

    struct Store {
        entries: Vec<(String, String)>,
        locked: bool,
        needs_prompt: bool,
        prompt_dismissed: bool,
    }

    #[derive(Clone)]
    struct Shared(Arc<Mutex<Store>>);

    impl Shared {
        fn with<R>(&self, read: impl FnOnce(&mut Store) -> R) -> R {
            read(&mut self.0.lock().expect("fake store lock"))
        }
    }

    fn object_path(path: &str) -> OwnedObjectPath {
        OwnedObjectPath::try_from(path).expect("valid object path")
    }

    fn assert_one_collection(objects: Vec<OwnedObjectPath>) {
        let requested: [OwnedObjectPath; 1] = objects.try_into().expect("one object");
        assert_eq!(requested[0].as_str(), COLLECTION_PATH);
    }

    fn item_path(index: usize) -> String {
        format!("{COLLECTION_PATH}/{index}")
    }

    struct FakeService(Shared);

    #[zbus::interface(name = "org.freedesktop.Secret.Service")]
    #[expect(
        clippy::unused_self,
        reason = "the macro generates methods that do not need the store"
    )]
    impl FakeService {
        fn open_session(
            &self,
            algorithm: &str,
            input: Value<'_>,
        ) -> fdo::Result<(OwnedValue, OwnedObjectPath)> {
            if algorithm != "plain" {
                return Err(fdo::Error::NotSupported(String::new()));
            }
            drop(input);
            let output = Value::from("")
                .try_to_owned()
                .map_err(|error| fdo::Error::Failed(error.to_string()))?;
            Ok((output, object_path("/org/freedesktop/secrets/session/s1")))
        }

        fn read_alias(&self, name: &str) -> OwnedObjectPath {
            if name == "default" {
                object_path(COLLECTION_PATH)
            } else {
                object_path("/")
            }
        }

        fn unlock(&self, objects: Vec<OwnedObjectPath>) -> (Vec<OwnedObjectPath>, OwnedObjectPath) {
            assert_one_collection(objects);
            self.0.with(|store| {
                if store.needs_prompt {
                    (Vec::new(), object_path(PROMPT_PATH))
                } else {
                    store.locked = false;
                    (vec![object_path(COLLECTION_PATH)], object_path("/"))
                }
            })
        }

        fn lock(&self, objects: Vec<OwnedObjectPath>) -> (Vec<OwnedObjectPath>, OwnedObjectPath) {
            assert_one_collection(objects);
            self.0.with(|store| store.locked = true);
            (vec![object_path(COLLECTION_PATH)], object_path("/"))
        }
    }

    struct FakeCollection(Shared);

    #[zbus::interface(name = "org.freedesktop.Secret.Collection")]
    impl FakeCollection {
        #[zbus(property)]
        fn items(&self) -> Vec<OwnedObjectPath> {
            let count = self.0.with(|store| store.entries.len());
            (0..count).map(|index| object_path(&item_path(index))).collect()
        }

        #[zbus(property)]
        fn locked(&self) -> bool {
            self.0.with(|store| store.locked)
        }
    }

    struct FakeItem(Shared, usize);

    #[zbus::interface(name = "org.freedesktop.Secret.Item")]
    impl FakeItem {
        #[zbus(property)]
        fn label(&self) -> String {
            self.0
                .with(|store| store.entries.get(self.1).map_or_default(|entry| entry.0.clone()))
        }

        fn get_secret(&self, session: OwnedObjectPath) -> fdo::Result<(SecretReply,)> {
            self.0.with(|store| {
                if store.locked {
                    return Err(fdo::Error::AccessDenied("locked".to_owned()));
                }
                let value = store
                    .entries
                    .get(self.1)
                    .map_or_default(|entry| entry.1.clone().into_bytes());
                Ok(((session, Vec::new(), value, "text/plain".to_owned()),))
            })
        }
    }

    struct FakePrompt(Shared);

    #[zbus::interface(name = "org.freedesktop.Secret.Prompt")]
    impl FakePrompt {
        fn prompt(&self, window_id: &str) {
            assert_eq!(window_id, "");
            self.0.with(|store| {
                if !store.prompt_dismissed {
                    store.locked = false;
                }
            });
        }
    }

    struct Fixture {
        provider: SecretServiceProvider,
        store: Shared,
        _server: Connection,
    }

    #[expect(
        deprecated,
        reason = "the replacement takes an async-io stream and would add a direct dependency"
    )]
    fn start(entries: &[(String, String)], locked: bool, serve: bool) -> Fixture {
        let store = Shared(Arc::new(Mutex::new(Store {
            entries: entries.to_vec(),
            locked,
            needs_prompt: false,
            prompt_dismissed: false,
        })));
        let (server_stream, client_stream) = UnixStream::pair().expect("socket pair");
        let shared = store.clone();
        let count = entries.len();
        let serving = thread::spawn(move || {
            let mut builder = Builder::unix_stream(server_stream)
                .p2p()
                .server(Guid::generate())
                .expect("server builder");
            if serve {
                builder = builder
                    .serve_at("/org/freedesktop/secrets", FakeService(shared.clone()))
                    .and_then(|builder| builder.serve_at(COLLECTION_PATH, FakeCollection(shared.clone())))
                    .and_then(|builder| builder.serve_at(PROMPT_PATH, FakePrompt(shared.clone())))
                    .expect("serve objects");
                for index in 0..count {
                    builder = builder
                        .serve_at(item_path(index), FakeItem(shared.clone(), index))
                        .expect("serve item");
                }
            } else {
                builder = builder
                    .serve_at("/unrelated", FakePrompt(shared.clone()))
                    .expect("serve unrelated object");
            }
            builder.build().expect("server connection")
        });
        let client = Builder::unix_stream(client_stream)
            .p2p()
            .method_timeout(Duration::from_secs(10))
            .build()
            .expect("client connection");
        let server = serving.join().expect("server thread");
        Fixture {
            provider: SecretServiceProvider::new(client, SERVICE_BUS_NAME.to_owned(), SHORT_PROMPT_TIMEOUT),
            store,
            _server: server,
        }
    }

    fn sample() -> Vec<(String, String)> {
        vec![
            ("mail account".to_owned(), "v-first-value".to_owned()),
            ("backup disk".to_owned(), "v-second-value".to_owned()),
        ]
    }

    fn reference(entry: &str) -> SecretReference {
        SecretReference {
            entry: entry.to_owned(),
        }
    }

    fn reveal(value: &SecretValue) -> String {
        let mut revealed = String::new();
        value.deliver_to(|text| revealed.push_str(text));
        revealed
    }

    #[test]
    fn lookup_returns_the_value_of_the_entry_with_that_label() {
        let mut fixture = start(&sample(), false, true);
        let value = fixture.provider.lookup(&reference("backup disk")).expect("found");
        assert_eq!(reveal(&value), "v-second-value");
    }

    #[test]
    fn lookup_of_an_unknown_label_is_unavailable() {
        let mut fixture = start(&sample(), false, true);
        assert!(fixture.provider.lookup(&reference("no such entry")).is_err());
    }

    #[test]
    fn lookup_unlocks_a_locked_collection_first() {
        let mut fixture = start(&sample(), true, true);
        let value = fixture
            .provider
            .lookup(&reference("mail account"))
            .expect("found");
        assert_eq!(reveal(&value), "v-first-value");
        assert!(!fixture.store.with(|store| store.locked));
    }

    #[test]
    fn list_labels_on_a_locked_collection_unlocks_and_returns_labels_only() {
        let mut fixture = start(&sample(), true, true);
        let labels = fixture.provider.list_labels().expect("labels");
        assert_eq!(labels, vec!["mail account".to_owned(), "backup disk".to_owned()]);
        assert!(!fixture.store.with(|store| store.locked));
    }

    #[test]
    fn unlock_waits_for_the_store_prompt_to_be_accepted() {
        let mut fixture = start(&sample(), true, true);
        fixture.store.with(|store| store.needs_prompt = true);
        assert!(fixture.provider.unlock().is_ok());
        assert!(!fixture.store.with(|store| store.locked));
    }

    #[test]
    fn unlock_fails_when_the_store_prompt_is_dismissed() {
        let mut fixture = start(&sample(), true, true);
        fixture.store.with(|store| {
            store.needs_prompt = true;
            store.prompt_dismissed = true;
        });
        assert!(fixture.provider.unlock().is_err());
        assert!(fixture.store.with(|store| store.locked));
    }

    #[test]
    fn lock_locks_the_default_collection() {
        let mut fixture = start(&sample(), false, true);
        assert!(fixture.provider.lock().is_ok());
        assert!(fixture.store.with(|store| store.locked));
    }

    #[test]
    fn a_missing_service_is_unavailable_for_every_operation() {
        let mut fixture = start(&sample(), false, false);
        assert!(fixture.provider.lookup(&reference("mail account")).is_err());
        assert!(fixture.provider.list_labels().is_err());
        assert!(fixture.provider.unlock().is_err());
        assert!(fixture.provider.lock().is_err());
    }

    fn distinct_entries() -> impl Strategy<Value = Vec<(String, String)>> {
        proptest::collection::hash_set("[a-z]{3,10}", 1..5).prop_flat_map(|labels| {
            let labels: Vec<String> = labels.into_iter().collect();
            let count = labels.len();
            proptest::collection::vec("v[0-9]{12,20}", count..=count)
                .prop_map(move |values| labels.iter().cloned().zip(values).collect())
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(12))]

        #[test]
        fn lookup_finds_each_generated_entry_and_nothing_else_leaks(
            entries in distinct_entries(),
            locked in any::<bool>(),
        ) {
            let mut fixture = start(&entries, locked, true);
            for (label, expected) in &entries {
                let value = fixture.provider.lookup(&reference(label)).expect("found");
                prop_assert_eq!(&reveal(&value), expected);
                let shown = format!("{value:?}");
                prop_assert!(!shown.contains(expected.as_str()));
            }
            let missing = fixture.provider.lookup(&reference("0 missing"));
            let failure = format!("{missing:?}");
            for (label, expected) in &entries {
                prop_assert!(!failure.contains(expected.as_str()));
                prop_assert!(!failure.contains(label.as_str()));
            }
        }

        #[test]
        fn list_labels_returns_exactly_the_labels(
            entries in distinct_entries(),
            locked in any::<bool>(),
        ) {
            let mut fixture = start(&entries, locked, true);
            let labels = fixture.provider.list_labels().expect("labels");
            let expected: HashSet<&String> = entries.iter().map(|entry| &entry.0).collect();
            let listed: HashSet<&String> = labels.iter().collect();
            prop_assert_eq!(listed, expected);
            for (_, value) in &entries {
                prop_assert!(!labels.contains(value));
            }
        }
    }
}
