#[cfg(target_os = "linux")]
use std::collections::HashMap;

use crate::error::{Error, Result};

pub const SERVICE: &str = "maskrun";
pub const NOTE_MAX_LEN: usize = 200;

/// Platform label and free-text note attached to a secret. Neither is a
/// secret: both are stored unmasked and shown in an agent session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecretMeta {
    pub platform: Option<String>,
    pub note: Option<String>,
}

impl SecretMeta {
    fn merge(&self, update: &MetaUpdate) -> SecretMeta {
        SecretMeta {
            platform: update.platform.resolve(self.platform.clone()),
            note: update.note.resolve(self.note.clone()),
        }
    }
}

/// One field's requested change: left alone, explicitly cleared (`--for ""`),
/// or set to a new value. Plain `Option<String>` can't tell "not given" apart
/// from "given empty", and both are meaningful here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FieldUpdate {
    #[default]
    Keep,
    Clear,
    Set(String),
}

impl FieldUpdate {
    fn resolve(&self, existing: Option<String>) -> Option<String> {
        match self {
            FieldUpdate::Keep => existing,
            FieldUpdate::Clear => None,
            FieldUpdate::Set(v) => Some(v.clone()),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MetaUpdate {
    pub platform: FieldUpdate,
    pub note: FieldUpdate,
}

pub trait Backend {
    fn name(&self) -> &'static str;
    fn put(&self, secret: &str, value: &str, meta: &MetaUpdate) -> Result<()>;
    fn get(&self, secret: &str) -> Result<Option<String>>;
    fn delete(&self, secret: &str) -> Result<()>;
    fn list(&self) -> Result<Vec<String>>;

    // Absence is a valid, common state, not an error — callers that already
    // know a secret exists (list/status) shouldn't have to special-case it.
    fn get_meta(&self, _secret: &str) -> Result<SecretMeta> {
        Ok(SecretMeta::default())
    }

    fn list_meta(&self) -> Result<Vec<(String, SecretMeta)>> {
        self.list()?
            .into_iter()
            .map(|name| {
                let meta = self.get_meta(&name)?;
                Ok((name, meta))
            })
            .collect()
    }

    // Relabel without touching the stored value. The default (read value,
    // re-`put` it) is the only option on a backend with no attribute-only
    // update call; Linux and macOS override this with one that never reads
    // the secret at all.
    fn set_meta(&self, secret: &str, update: &MetaUpdate) -> Result<()> {
        let value = self
            .get(secret)?
            .ok_or_else(|| Error::msg(format!("no such secret: {secret}")))?;
        self.put(secret, &value, update)
    }

    // Locked looks identical to empty to get()/list() on Secret Service;
    // default false since Keychain/Credential Manager expose no comparable
    // queryable lock.
    fn is_locked(&self) -> Result<bool> {
        Ok(false)
    }
}

// `^[A-Za-z0-9][A-Za-z0-9._-]*$`, written by hand instead of with `regex` —
// `regex` is kept for mask.rs/guard.rs, not needed for one fixed pattern.
// Also used for platform labels: same charset, same reasoning to keep them
// safe to embed in Windows' encoded Comment field without escaping.
pub fn check_name(secret: &str) -> Result<&str> {
    let mut chars = secret.chars();
    let ok = match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        }
        _ => false,
    };
    if !ok {
        return Err(Error::msg(format!(
            "invalid secret name '{secret}' — use letters, digits, dot, dash and \
             underscore, starting with a letter or digit."
        )));
    }
    Ok(secret)
}

// No newlines: a note shares the Windows Comment encoding's one line with
// the platform label (see windows::encode_meta), and a control character
// there would corrupt the split back into fields. The length cap is a
// tripwire against pasting an actual secret into `--note` by accident.
pub fn check_note(note: &str) -> Result<&str> {
    if note.chars().any(|c| c.is_control()) {
        return Err(Error::msg(
            "note may not contain control characters (including newlines) — keep it \
             to one line.",
        ));
    }
    if note.chars().count() > NOTE_MAX_LEN {
        return Err(Error::msg(format!(
            "note is too long ({} chars, max {NOTE_MAX_LEN}) — this is a label, not a \
             place to paste a secret.",
            note.chars().count()
        )));
    }
    Ok(note)
}

pub fn pick_backend() -> Result<Box<dyn Backend>> {
    let override_raw = std::env::var("MASKRUN_BACKEND").unwrap_or_default();
    let override_val = override_raw.trim().to_lowercase();
    if !override_val.is_empty() {
        return match override_val.as_str() {
            "secret-service" => secret_service_backend(),
            "keychain" => keychain_backend(),
            "dpapi" => credential_manager_backend(),
            // Test-only, undocumented on purpose (see the `memory` module):
            // reachable only by setting *both* this and MASKRUN_STORE_DIR —
            // no cfg(target_os) branch or default ever selects it, so a real
            // interactive session never touches it by accident.
            "memory" => memory_backend(),
            other => Err(Error::msg(format!(
                "unknown MASKRUN_BACKEND='{other}' (expected secret-service, keychain \
                 or dpapi)"
            ))),
        };
    }

    if cfg!(target_os = "macos") {
        keychain_backend()
    } else if cfg!(target_os = "windows") {
        credential_manager_backend()
    } else {
        secret_service_backend()
    }
}

#[cfg(target_os = "linux")]
fn secret_service_backend() -> Result<Box<dyn Backend>> {
    Ok(Box::new(linux::SecretServiceBackend::connect()?))
}
#[cfg(not(target_os = "linux"))]
fn secret_service_backend() -> Result<Box<dyn Backend>> {
    Err(Error::msg(
        "the secret-service backend is only available on Linux",
    ))
}

#[cfg(target_os = "macos")]
fn keychain_backend() -> Result<Box<dyn Backend>> {
    Ok(Box::new(macos::KeychainBackend::new()))
}
#[cfg(not(target_os = "macos"))]
fn keychain_backend() -> Result<Box<dyn Backend>> {
    Err(Error::msg(
        "the keychain backend is only available on macOS",
    ))
}

#[cfg(target_os = "windows")]
fn credential_manager_backend() -> Result<Box<dyn Backend>> {
    Ok(Box::new(windows::CredentialManagerBackend::new()))
}
#[cfg(not(target_os = "windows"))]
fn credential_manager_backend() -> Result<Box<dyn Backend>> {
    Err(Error::msg(
        "the dpapi/credential-manager backend is only available on Windows",
    ))
}

// No cfg gate — this one has to build and run on every platform's `cargo
// test`/pty harness alike, unlike the three above.
fn memory_backend() -> Result<Box<dyn Backend>> {
    let dir = std::env::var("MASKRUN_STORE_DIR").map_err(|_| {
        Error::msg(
            "MASKRUN_BACKEND=memory requires MASKRUN_STORE_DIR=<directory>. This backend \
             exists only so interactive tests never run against a real keyring — there is \
             deliberately no default location for it to fall back to.",
        )
    })?;
    Ok(Box::new(memory::MemoryBackend::new(dir.into())))
}

// A JSON-file-backed `Backend` that never touches a real OS keyring —
// written after a TUI pty test navigated into the wrong platform group and
// deleted two real secrets from the actual Secret Service (see
// Sessions/2026-09-21-tui.md). "Don't touch real secrets" was a note in a
// brief, not something the test process was structurally prevented from
// doing; this module is the fix. Reached only through `memory_backend()`
// above, itself reached only by the explicit `MASKRUN_BACKEND=memory`
// string — no cfg(target_os) branch, and no default in `pick_backend`,
// ever selects it. File-backed rather than purely in-process so a setup
// step (`maskrun put` in one process) and a driven TUI (a second process,
// as a real pty test spawns one) can share the same throwaway store via
// one `MASKRUN_STORE_DIR`, the same way tests/cli.rs already spawns the
// compiled binary fresh per assertion.
mod memory {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use serde::{Deserialize, Serialize};

    use super::{Backend, Error, MetaUpdate, Result, SecretMeta};

    #[derive(Clone, Default, Serialize, Deserialize)]
    struct Entry {
        value: String,
        platform: Option<String>,
        note: Option<String>,
    }

    impl Entry {
        fn meta(&self) -> SecretMeta {
            SecretMeta {
                platform: self.platform.clone(),
                note: self.note.clone(),
            }
        }
    }

    pub struct MemoryBackend {
        path: PathBuf,
    }

    impl MemoryBackend {
        pub fn new(dir: PathBuf) -> Self {
            MemoryBackend {
                path: dir.join("maskrun-test-store.json"),
            }
        }

        // Missing or unreadable file reads as an empty store rather than an
        // error: the first `put` against a fresh MASKRUN_STORE_DIR has
        // nothing to load yet, same as a fresh real keyring has nothing to
        // list.
        fn load(&self) -> HashMap<String, Entry> {
            std::fs::read(&self.path)
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default()
        }

        fn save(&self, store: &HashMap<String, Entry>) -> Result<()> {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let bytes = serde_json::to_vec_pretty(store)?;
            std::fs::write(&self.path, bytes)?;
            Ok(())
        }
    }

    impl Backend for MemoryBackend {
        fn name(&self) -> &'static str {
            "memory (test-only)"
        }

        fn put(&self, secret: &str, value: &str, update: &MetaUpdate) -> Result<()> {
            let mut store = self.load();
            let merged = store
                .get(secret)
                .map(Entry::meta)
                .unwrap_or_default()
                .merge(update);
            store.insert(
                secret.to_string(),
                Entry {
                    value: value.to_string(),
                    platform: merged.platform,
                    note: merged.note,
                },
            );
            self.save(&store)
        }

        fn get(&self, secret: &str) -> Result<Option<String>> {
            Ok(self.load().get(secret).map(|e| e.value.clone()))
        }

        fn get_meta(&self, secret: &str) -> Result<SecretMeta> {
            Ok(self.load().get(secret).map(Entry::meta).unwrap_or_default())
        }

        fn set_meta(&self, secret: &str, update: &MetaUpdate) -> Result<()> {
            let mut store = self.load();
            let Some(existing) = store.get(secret).cloned() else {
                return Err(Error::msg(format!("no such secret: {secret}")));
            };
            let merged = existing.meta().merge(update);
            store.insert(
                secret.to_string(),
                Entry {
                    value: existing.value,
                    platform: merged.platform,
                    note: merged.note,
                },
            );
            self.save(&store)
        }

        fn delete(&self, secret: &str) -> Result<()> {
            let mut store = self.load();
            store.remove(secret);
            self.save(&store)
        }

        fn list(&self) -> Result<Vec<String>> {
            let mut names: Vec<String> = self.load().into_keys().collect();
            names.sort();
            Ok(names)
        }

        fn list_meta(&self) -> Result<Vec<(String, SecretMeta)>> {
            let mut out: Vec<(String, SecretMeta)> = self
                .load()
                .into_iter()
                .map(|(name, entry)| (name, entry.meta()))
                .collect();
            out.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(out)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn put_get_delete_roundtrip_through_the_file() {
            let dir = tempfile::tempdir().unwrap();
            let backend = MemoryBackend::new(dir.path().to_path_buf());
            let update = MetaUpdate {
                platform: crate::keyring::FieldUpdate::Set("github".into()),
                note: crate::keyring::FieldUpdate::Keep,
            };
            backend.put("s1", "v1", &update).unwrap();
            assert_eq!(backend.get("s1").unwrap().as_deref(), Some("v1"));
            assert_eq!(
                backend.get_meta("s1").unwrap().platform.as_deref(),
                Some("github")
            );
            backend.delete("s1").unwrap();
            assert_eq!(backend.get("s1").unwrap(), None);
        }

        #[test]
        fn a_second_backend_instance_over_the_same_dir_sees_the_same_data() {
            let dir = tempfile::tempdir().unwrap();
            let a = MemoryBackend::new(dir.path().to_path_buf());
            a.put("s1", "v1", &MetaUpdate::default()).unwrap();
            let b = MemoryBackend::new(dir.path().to_path_buf());
            assert_eq!(b.get("s1").unwrap().as_deref(), Some("v1"));
        }

        #[test]
        fn missing_store_file_reads_as_empty_not_an_error() {
            let dir = tempfile::tempdir().unwrap();
            let backend = MemoryBackend::new(dir.path().to_path_buf());
            assert_eq!(backend.list().unwrap(), Vec::<String>::new());
        }
    }
}

// Not the `keyring` crate: its default (v1/zbus) Secret Service store sets
// attributes `service` + `username`, confirmed by reading
// zbus-secret-service-keyring-store-1.0.1/src/cred.rs (search_attributes())
// before writing this file. That schema is invisible to
// `secret-tool lookup service maskrun name <secret>` and to entries other
// tools already read that way. `dbus-secret-service` exposes the raw
// attribute map instead, so we set exactly what `secret-tool` expects.
#[cfg(target_os = "linux")]
pub mod linux {
    use super::*;
    use dbus_secret_service::{EncryptionType, Item, SecretService};

    pub struct SecretServiceBackend {
        ss: SecretService,
    }

    impl SecretServiceBackend {
        pub fn connect() -> Result<Self> {
            // Plain: the transport is the caller's own D-Bus session bus,
            // already scoped to this user via a unix socket. EncryptionType::Dh
            // would need a DH key exchange, pulling in a num-bigint-based
            // stack (~30 extra transitive crates) to harden a channel the OS
            // already restricts to this user.
            //
            // A bounded prompt timeout, not the crate's plain connect(): that
            // blocks *indefinitely* on an unlock prompt nobody may be there
            // to answer (headless CI, SSH) — its own way of reproducing the
            // incident this file exists to fix.
            let ss = SecretService::connect_with_max_prompt_timeout(
                EncryptionType::Plain,
                Self::prompt_timeout_secs(),
            )
            .map_err(|e| {
                Error::msg(format!(
                    "could not reach the Secret Service ({e}). A running secret \
                     service (gnome-keyring or KeePassXC) is required."
                ))
            })?;
            Ok(Self { ss })
        }

        // 0 = cancel an unlock prompt instead of ever showing it (the
        // crate's own documented behaviour): the explicit opt-out, or no
        // DISPLAY/WAYLAND_DISPLAY for the graphical prompt to render on in
        // the first place. 30s otherwise — enough to type a password,
        // short enough not to leave `run`/`get`/`list` hanging for good.
        fn prompt_timeout_secs() -> u64 {
            if std::env::var("MASKRUN_NO_UNLOCK").as_deref() == Ok("1") {
                return 0;
            }
            let has_display = std::env::var_os("DISPLAY").is_some()
                || std::env::var_os("WAYLAND_DISPLAY").is_some();
            if has_display {
                30
            } else {
                0
            }
        }

        fn search_attrs(secret: &str) -> HashMap<&str, &str> {
            HashMap::from([("service", SERVICE), ("name", secret)])
        }

        fn meta_from_attrs(attrs: &HashMap<String, String>) -> SecretMeta {
            let get = |k: &str| attrs.get(k).filter(|s| !s.is_empty()).cloned();
            SecretMeta {
                platform: get("platform"),
                note: get("note"),
            }
        }

        // Covers a dismissed prompt and one never shown (timeout 0) alike;
        // never suggests `maskrun put` — the secrets are still there.
        fn locked_error(e: dbus_secret_service::Error) -> Error {
            match e {
                dbus_secret_service::Error::Prompt => Error::msg(
                    "the keyring is locked and maskrun did not unlock it (no unlock \
                     prompt was completed). Your secrets are still there — unlock the \
                     keyring yourself and run this again.",
                ),
                other => Error::msg(format!("secret-service: {other}")),
            }
        }

        // A no-op when nothing's locked (ensure_unlocked() checks first),
        // so calling this up front is cheap on the common path.
        fn unlock_all_collections(&self) -> Result<()> {
            let collections = self
                .ss
                .get_all_collections()
                .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            for collection in &collections {
                collection.ensure_unlocked().map_err(Self::locked_error)?;
            }
            Ok(())
        }

        fn find(&self, secret: &str) -> Result<Vec<Item<'_>>> {
            self.unlock_all_collections()?;
            let found = self
                .ss
                .search_items(Self::search_attrs(secret))
                .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            Ok(found.unlocked.into_iter().chain(found.locked).collect())
        }
    }

    impl Backend for SecretServiceBackend {
        fn name(&self) -> &'static str {
            "secret-service"
        }

        fn put(&self, secret: &str, value: &str, update: &MetaUpdate) -> Result<()> {
            let collection = self
                .ss
                .get_any_collection()
                .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            // `put` is a direct write request, so unlocking to satisfy it
            // (unlike the incidental get()/list() calls from status/run) is
            // expected, not a surprise.
            collection.ensure_unlocked().map_err(Self::locked_error)?;

            let existing = self.get_meta(secret)?;
            let merged = existing.merge(update);

            // `create_item(replace: true, ...)` only replaces an item whose
            // FULL attribute set matches the one given here (verified: see
            // Sessions/2026-09-21-platform-etiketi.md) — changing platform/
            // note would otherwise leave a stale duplicate under the same
            // service+name instead of updating it. Deleting first sidesteps
            // that regardless of the exact matching rule.
            for item in self.find(secret)? {
                item.delete()
                    .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            }

            let mut attrs = Self::search_attrs(secret);
            if let Some(p) = merged.platform.as_deref() {
                attrs.insert("platform", p);
            }
            if let Some(n) = merged.note.as_deref() {
                attrs.insert("note", n);
            }
            collection
                .create_item(
                    &format!("maskrun: {secret}"),
                    attrs,
                    value.as_bytes(),
                    true,
                    "text/plain",
                )
                .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            Ok(())
        }

        fn get(&self, secret: &str) -> Result<Option<String>> {
            let items = self.find(secret)?;
            let Some(item) = items.first() else {
                return Ok(None);
            };
            // Belt and suspenders in case a provider still hands back a
            // locked item despite the collection-wide unlock in find().
            if item.is_locked().unwrap_or(false) {
                item.unlock().map_err(Self::locked_error)?;
            }
            let bytes = item
                .get_secret()
                .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
        }

        fn get_meta(&self, secret: &str) -> Result<SecretMeta> {
            let items = self.find(secret)?;
            let Some(item) = items.first() else {
                return Ok(SecretMeta::default());
            };
            let attrs = item
                .get_attributes()
                .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            Ok(Self::meta_from_attrs(&attrs))
        }

        fn set_meta(&self, secret: &str, update: &MetaUpdate) -> Result<()> {
            let items = self.find(secret)?;
            if items.is_empty() {
                return Err(Error::msg(format!("no such secret: {secret}")));
            }
            let existing = Self::meta_from_attrs(
                &items[0]
                    .get_attributes()
                    .map_err(|e| Error::msg(format!("secret-service: {e}")))?,
            );
            let merged = existing.merge(update);
            let mut attrs = Self::search_attrs(secret);
            if let Some(p) = merged.platform.as_deref() {
                attrs.insert("platform", p);
            }
            if let Some(n) = merged.note.as_deref() {
                attrs.insert("note", n);
            }
            // `Item.Attributes` is a D-Bus property: writing it replaces the
            // whole map, so `service`/`name` are re-sent every time, not
            // just the two fields that changed.
            for item in &items {
                item.set_attributes(attrs.clone())
                    .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            }
            Ok(())
        }

        fn delete(&self, secret: &str) -> Result<()> {
            for item in self.find(secret)? {
                item.delete()
                    .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            }
            Ok(())
        }

        fn list(&self) -> Result<Vec<String>> {
            Ok(self
                .list_meta()?
                .into_iter()
                .map(|(name, _)| name)
                .collect())
        }

        fn list_meta(&self) -> Result<Vec<(String, SecretMeta)>> {
            self.unlock_all_collections()?;
            let found = self
                .ss
                .search_items(HashMap::from([("service", SERVICE)]))
                .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            let mut out: Vec<(String, SecretMeta)> = Vec::new();
            for item in found.unlocked.iter().chain(found.locked.iter()) {
                if let Ok(attrs) = item.get_attributes() {
                    if let Some(name) = attrs.get("name") {
                        out.push((name.clone(), Self::meta_from_attrs(&attrs)));
                    }
                }
            }
            out.sort_by(|a, b| a.0.cmp(&b.0));
            out.dedup_by(|a, b| a.0 == b.0);
            Ok(out)
        }

        // put()/get()/list() don't pin to one collection, so there's no
        // single one to ask; treating any locked collection as a hit errs
        // toward warning over silently agreeing with "empty".
        fn is_locked(&self) -> Result<bool> {
            let collections = self
                .ss
                .get_all_collections()
                .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            Ok(collections.iter().any(|c| c.is_locked().unwrap_or(false)))
        }
    }
}

// UNVERIFIED (no macOS machine — see Project.md). put/get/delete use
// security-framework's `passwords`/`item` APIs, so the value never touches
// argv (unlike the Python backend's `security -w VALUE`, briefly visible in
// `ps` to this user). `list` still shells out to `security dump-keychain`
// (read-only, prints no secret values): the crate has no service-scoped
// enumeration call at this level.
#[cfg(target_os = "macos")]
pub mod macos {
    use super::*;
    use core_foundation::data::CFData;
    use security_framework::item::{
        update_item, ItemAddOptions, ItemAddValue, ItemClass, ItemSearchOptions, ItemUpdateOptions,
        ItemUpdateValue,
    };
    use security_framework::os::macos::keychain::SecKeychain;
    use security_framework::passwords::{
        delete_generic_password, generic_password, PasswordOptions,
    };
    use std::process::Command;

    // Apple's errSecItemNotFound (<Security/SecBase.h>); security-framework
    // doesn't re-export it, and it's not worth a direct dep on
    // security-framework-sys for one constant.
    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

    pub struct KeychainBackend;

    impl Default for KeychainBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    impl KeychainBackend {
        pub fn new() -> Self {
            KeychainBackend
        }

        fn search(secret: &str) -> ItemSearchOptions {
            let mut search = ItemSearchOptions::new();
            search
                .class(ItemClass::generic_password())
                .service(SERVICE)
                .account(secret);
            search
        }
    }

    // No is_locked() override: security-framework wraps no lock probe, only
    // unlock() — but SecKeychainUnlock is documented to no-op when already
    // unlocked, so calling it unconditionally below is as cheap as a probe
    // and a canceled prompt still surfaces as its own error instead of
    // looking like an empty keychain. Its one gap vs. the Linux path: no
    // timeout knob at all (a blocking FFI call), so MASKRUN_NO_UNLOCK is the
    // only way to guarantee headless macOS CI doesn't hang on it.
    fn unlock_default_keychain() -> Result<()> {
        if std::env::var("MASKRUN_NO_UNLOCK").as_deref() == Ok("1") {
            return Ok(());
        }
        let mut keychain =
            SecKeychain::default().map_err(|e| Error::msg(format!("keychain: {e}")))?;
        keychain.unlock(None).map_err(|e| {
            Error::msg(format!(
                "the keychain is locked and the unlock prompt was not completed ({e}). \
                 Your secrets are still there — unlock it and try again."
            ))
        })
    }

    impl Backend for KeychainBackend {
        fn name(&self) -> &'static str {
            "keychain"
        }

        fn put(&self, secret: &str, value: &str, update: &MetaUpdate) -> Result<()> {
            unlock_default_keychain()?;
            let existing = self.get_meta(secret)?;
            let merged = existing.merge(update);
            let comment = merged.note.as_deref().unwrap_or("");
            let description = merged.platform.as_deref().unwrap_or("");

            // Search params here carry only service+account, so this update
            // matches the existing item regardless of what its comment/
            // description used to be — unlike routing a changed attribute
            // through `set_generic_password`, whose duplicate-item fallback
            // re-searches using those same (new) attributes and would find
            // nothing to update.
            let mut upd = ItemUpdateOptions::new();
            upd.set_value(ItemUpdateValue::Data(CFData::from_buffer(value.as_bytes())))
                .set_comment(comment)
                .set_description(description);
            match update_item(&Self::search(secret), &upd) {
                Ok(()) => Ok(()),
                Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => {
                    let mut add = ItemAddOptions::new(ItemAddValue::Data {
                        class: ItemClass::generic_password(),
                        data: CFData::from_buffer(value.as_bytes()),
                    });
                    add.set_service(SERVICE)
                        .set_account_name(secret)
                        .set_comment(comment)
                        .set_description(description);
                    add.add().map_err(|e| Error::msg(format!("keychain: {e}")))
                }
                Err(e) => Err(Error::msg(format!("keychain: {e}"))),
            }
        }

        fn get(&self, secret: &str) -> Result<Option<String>> {
            unlock_default_keychain()?;
            match generic_password(PasswordOptions::new_generic_password(SERVICE, secret)) {
                Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
                Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
                Err(e) => Err(Error::msg(format!("keychain: {e}"))),
            }
        }

        fn get_meta(&self, secret: &str) -> Result<SecretMeta> {
            let mut search = Self::search(secret);
            search.load_attributes(true).limit(1);
            let results = match search.search() {
                Ok(r) => r,
                Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => return Ok(SecretMeta::default()),
                Err(e) => return Err(Error::msg(format!("keychain: {e}"))),
            };
            let Some(dict) = results.iter().find_map(|r| r.simplify_dict()) else {
                return Ok(SecretMeta::default());
            };
            let get = |k: &str| dict.get(k).filter(|s| !s.is_empty()).cloned();
            Ok(SecretMeta {
                platform: get("desc"),
                note: get("icmt"),
            })
        }

        fn set_meta(&self, secret: &str, update: &MetaUpdate) -> Result<()> {
            if self.get(secret)?.is_none() {
                return Err(Error::msg(format!("no such secret: {secret}")));
            }
            let existing = self.get_meta(secret)?;
            let merged = existing.merge(update);
            let mut upd = ItemUpdateOptions::new();
            upd.set_comment(merged.note.as_deref().unwrap_or(""))
                .set_description(merged.platform.as_deref().unwrap_or(""));
            update_item(&Self::search(secret), &upd)
                .map_err(|e| Error::msg(format!("keychain: {e}")))
        }

        fn delete(&self, secret: &str) -> Result<()> {
            match delete_generic_password(SERVICE, secret) {
                Ok(()) => Ok(()),
                Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
                Err(e) => Err(Error::msg(format!("keychain: {e}"))),
            }
        }

        fn list(&self) -> Result<Vec<String>> {
            unlock_default_keychain()?;
            let output = Command::new("security")
                .arg("dump-keychain")
                .output()
                .map_err(|e| Error::msg(format!("security not found: {e}")))?;
            let blob = String::from_utf8_lossy(&output.stdout);
            let mut names = Vec::new();
            let mut current: Option<String> = None;
            for line in blob.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("\"acct\"<blob>=\"") {
                    current = rest.strip_suffix('"').map(str::to_string);
                    continue;
                }
                if let Some(rest) = line.strip_prefix("\"svce\"<blob>=\"") {
                    if rest.strip_suffix('"') == Some(SERVICE) {
                        if let Some(name) = current.take() {
                            names.push(name);
                        }
                    }
                    current = None;
                }
            }
            names.sort();
            names.dedup();
            Ok(names)
        }
    }
}

// UNVERIFIED (no Windows machine — see Project.md). Storage-format change
// from the Python backend's DPAPI files under %LOCALAPPDATA%\maskrun: the
// repo is private and never published, so there is nothing to migrate.
// `MASKRUN_BACKEND=dpapi` still selects this backend, for override
// compatibility with the Python CLI's three accepted values.
#[cfg(target_os = "windows")]
pub mod windows {
    use super::*;
    use ::windows::core::{HRESULT, PWSTR};
    use ::windows::Win32::Foundation::{ERROR_NOT_FOUND, FILETIME};
    use ::windows::Win32::Security::Credentials::{
        CredDeleteW, CredEnumerateW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS,
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    fn target_name(secret: &str) -> Vec<u16> {
        wide(&format!("{SERVICE}/{secret}"))
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    // Credential Manager has one freeform field (`Comment`) and no separate
    // attribute store, so platform+note share it as `platform=P<US>note=N`.
    // U+001F (Unit Separator) can't appear in either half: `check_name`
    // forbids it in a platform label, and `check_note` rejects any control
    // character in a note — so splitting on it back apart is unambiguous.
    const META_SEP: char = '\u{1f}';

    fn encode_meta(meta: &SecretMeta) -> String {
        format!(
            "platform={}{META_SEP}note={}",
            meta.platform.as_deref().unwrap_or(""),
            meta.note.as_deref().unwrap_or("")
        )
    }

    fn decode_meta(comment: &str) -> SecretMeta {
        let mut platform = None;
        let mut note = None;
        if let Some((p, n)) = comment.split_once(META_SEP) {
            if let Some(p) = p.strip_prefix("platform=").filter(|s| !s.is_empty()) {
                platform = Some(p.to_string());
            }
            if let Some(n) = n.strip_prefix("note=").filter(|s| !s.is_empty()) {
                note = Some(n.to_string());
            }
        }
        SecretMeta { platform, note }
    }

    struct RawCredential {
        value: String,
        meta: SecretMeta,
    }

    fn read_credential(secret: &str) -> Result<Option<RawCredential>> {
        let target = target_name(secret);
        unsafe {
            let mut ptr: *mut CREDENTIALW = std::ptr::null_mut();
            match CredReadW(
                PWSTR(target.as_ptr() as *mut _),
                CRED_TYPE_GENERIC,
                None,
                &mut ptr,
            ) {
                Ok(()) => {
                    let cred = &*ptr;
                    let bytes = std::slice::from_raw_parts(
                        cred.CredentialBlob,
                        cred.CredentialBlobSize as usize,
                    );
                    let value = String::from_utf8_lossy(bytes).into_owned();
                    let meta = if cred.Comment.0.is_null() {
                        SecretMeta::default()
                    } else {
                        decode_meta(&cred.Comment.to_string().unwrap_or_default())
                    };
                    CredFree(ptr as *mut _);
                    Ok(Some(RawCredential { value, meta }))
                }
                Err(e) if e.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => Ok(None),
                Err(e) => Err(Error::msg(format!("credential manager: {e}"))),
            }
        }
    }

    pub struct CredentialManagerBackend;

    impl Default for CredentialManagerBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    impl CredentialManagerBackend {
        pub fn new() -> Self {
            CredentialManagerBackend
        }
    }

    // Default is_locked() stands: Credential Manager has no lock concept,
    // only the logged-in user session. Default set_meta() stands too:
    // CredWriteW always rewrites the whole entry, so relabelling has to read
    // the value back regardless — there is no cheaper attribute-only call.
    impl Backend for CredentialManagerBackend {
        fn name(&self) -> &'static str {
            "dpapi"
        }

        fn put(&self, secret: &str, value: &str, update: &MetaUpdate) -> Result<()> {
            let existing = self.get_meta(secret)?;
            let merged = existing.merge(update);
            let mut target = target_name(secret);
            let mut username = wide(SERVICE);
            let mut blob = value.as_bytes().to_vec();
            let mut comment = wide(&encode_meta(&merged));
            let credential = CREDENTIALW {
                Flags: CRED_FLAGS(0),
                Type: CRED_TYPE_GENERIC,
                TargetName: PWSTR(target.as_mut_ptr()),
                Comment: PWSTR(comment.as_mut_ptr()),
                LastWritten: FILETIME::default(),
                CredentialBlobSize: blob.len() as u32,
                CredentialBlob: blob.as_mut_ptr(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                AttributeCount: 0,
                Attributes: std::ptr::null_mut(),
                TargetAlias: PWSTR::null(),
                UserName: PWSTR(username.as_mut_ptr()),
            };
            unsafe { CredWriteW(&credential, 0) }
                .map_err(|e| Error::msg(format!("credential manager: {e}")))
        }

        fn get(&self, secret: &str) -> Result<Option<String>> {
            Ok(read_credential(secret)?.map(|c| c.value))
        }

        fn get_meta(&self, secret: &str) -> Result<SecretMeta> {
            Ok(read_credential(secret)?.map(|c| c.meta).unwrap_or_default())
        }

        fn delete(&self, secret: &str) -> Result<()> {
            let target = target_name(secret);
            unsafe { CredDeleteW(PWSTR(target.as_ptr() as *mut _), CRED_TYPE_GENERIC, None) }
                .or_else(|e| {
                    if e.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) {
                        Ok(())
                    } else {
                        Err(Error::msg(format!("credential manager: {e}")))
                    }
                })
        }

        fn list(&self) -> Result<Vec<String>> {
            Ok(self
                .list_meta()?
                .into_iter()
                .map(|(name, _)| name)
                .collect())
        }

        fn list_meta(&self) -> Result<Vec<(String, SecretMeta)>> {
            let filter = wide(&format!("{SERVICE}/*"));
            unsafe {
                let mut count: u32 = 0;
                let mut ptr: *mut *mut CREDENTIALW = std::ptr::null_mut();
                // No flags: CRED_ENUMERATE_ALL_CREDENTIALS requires a null
                // filter, and we want the filter.
                match CredEnumerateW(PWSTR(filter.as_ptr() as *mut _), None, &mut count, &mut ptr) {
                    Ok(()) => {}
                    // An empty store, or nothing matching the filter, comes
                    // back as ERROR_NOT_FOUND rather than a zero count.
                    Err(e) if e.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => {
                        return Ok(Vec::new())
                    }
                    Err(e) => return Err(Error::msg(format!("credential manager: {e}"))),
                }
                let mut out = Vec::new();
                let entries = std::slice::from_raw_parts(ptr, count as usize);
                for entry in entries {
                    let cred = &**entry;
                    let target = cred.TargetName.to_string().unwrap_or_default();
                    let Some(name) = target.strip_prefix(&format!("{SERVICE}/")) else {
                        continue;
                    };
                    let meta = if cred.Comment.0.is_null() {
                        SecretMeta::default()
                    } else {
                        decode_meta(&cred.Comment.to_string().unwrap_or_default())
                    };
                    out.push((name.to_string(), meta));
                }
                CredFree(ptr as *mut _);
                out.sort_by(|a, b| a.0.cmp(&b.0));
                out.dedup_by(|a, b| a.0 == b.0);
                Ok(out)
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    // Read-only, so safe against a real in-use keyring; skips (not fails)
    // when none is reachable, same as the rest of the suite.
    #[test]
    fn is_locked_completes_against_a_real_backend_if_one_is_reachable() {
        let Ok(backend) = linux::SecretServiceBackend::connect() else {
            eprintln!("skipping: no reachable secret-service backend");
            return;
        };
        let locked = backend.is_locked();
        assert!(
            locked.is_ok(),
            "is_locked() should report a state, not error: {locked:?}"
        );
    }

    #[test]
    fn field_update_keep_resolves_to_existing() {
        assert_eq!(
            FieldUpdate::Keep.resolve(Some("x".into())),
            Some("x".into())
        );
        assert_eq!(FieldUpdate::Keep.resolve(None), None);
    }

    #[test]
    fn field_update_clear_resolves_to_none() {
        assert_eq!(FieldUpdate::Clear.resolve(Some("x".into())), None);
    }

    #[test]
    fn field_update_set_resolves_to_new_value() {
        assert_eq!(
            FieldUpdate::Set("y".into()).resolve(Some("x".into())),
            Some("y".into())
        );
    }

    #[test]
    fn secret_meta_merge_keeps_untouched_fields() {
        let existing = SecretMeta {
            platform: Some("github".into()),
            note: Some("old".into()),
        };
        let update = MetaUpdate {
            platform: FieldUpdate::Keep,
            note: FieldUpdate::Set("new".into()),
        };
        let merged = existing.merge(&update);
        assert_eq!(merged.platform.as_deref(), Some("github"));
        assert_eq!(merged.note.as_deref(), Some("new"));
    }

    #[test]
    fn check_note_rejects_newline() {
        assert!(check_note("line one\nline two").is_err());
    }

    #[test]
    fn check_note_rejects_over_200_chars() {
        let long = "a".repeat(201);
        assert!(check_note(&long).is_err());
    }

    #[test]
    fn check_note_accepts_200_chars() {
        let ok = "a".repeat(200);
        assert!(check_note(&ok).is_ok());
    }
}
