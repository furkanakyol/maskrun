#[cfg(target_os = "linux")]
use std::collections::HashMap;

use crate::error::{Error, Result};

pub const SERVICE: &str = "maskrun";

pub trait Backend {
    fn name(&self) -> &'static str;
    fn put(&self, secret: &str, value: &str) -> Result<()>;
    fn get(&self, secret: &str) -> Result<Option<String>>;
    fn delete(&self, secret: &str) -> Result<()>;
    fn list(&self) -> Result<Vec<String>>;

    // Locked looks identical to empty to get()/list() on Secret Service;
    // default false since Keychain/Credential Manager expose no comparable
    // queryable lock.
    fn is_locked(&self) -> Result<bool> {
        Ok(false)
    }
}

// `^[A-Za-z0-9][A-Za-z0-9._-]*$`, written by hand instead of with `regex` —
// `regex` is kept for mask.rs/guard.rs, not needed for one fixed pattern.
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

pub fn pick_backend() -> Result<Box<dyn Backend>> {
    let override_raw = std::env::var("MASKRUN_BACKEND").unwrap_or_default();
    let override_val = override_raw.trim().to_lowercase();
    if !override_val.is_empty() {
        return match override_val.as_str() {
            "secret-service" => secret_service_backend(),
            "keychain" => keychain_backend(),
            "dpapi" => credential_manager_backend(),
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
    use dbus_secret_service::{EncryptionType, SecretService};

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
    }

    impl Backend for SecretServiceBackend {
        fn name(&self) -> &'static str {
            "secret-service"
        }

        fn put(&self, secret: &str, value: &str) -> Result<()> {
            let collection = self
                .ss
                .get_any_collection()
                .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            // `put` is a direct write request, so unlocking to satisfy it
            // (unlike the incidental get()/list() calls from status/run) is
            // expected, not a surprise.
            collection.ensure_unlocked().map_err(Self::locked_error)?;
            collection
                .create_item(
                    &format!("maskrun: {secret}"),
                    Self::search_attrs(secret),
                    value.as_bytes(),
                    true,
                    "text/plain",
                )
                .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            Ok(())
        }

        fn get(&self, secret: &str) -> Result<Option<String>> {
            self.unlock_all_collections()?;
            let found = self
                .ss
                .search_items(Self::search_attrs(secret))
                .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            let item = match found.unlocked.first().or_else(|| found.locked.first()) {
                Some(item) => item,
                None => return Ok(None),
            };
            // Belt and suspenders in case a provider still hands back a
            // locked item despite the collection-wide unlock above.
            if item.is_locked().unwrap_or(false) {
                item.unlock().map_err(Self::locked_error)?;
            }
            let bytes = item
                .get_secret()
                .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
        }

        fn delete(&self, secret: &str) -> Result<()> {
            let found = self
                .ss
                .search_items(Self::search_attrs(secret))
                .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            for item in found.unlocked.iter().chain(found.locked.iter()) {
                item.delete()
                    .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            }
            Ok(())
        }

        fn list(&self) -> Result<Vec<String>> {
            self.unlock_all_collections()?;
            let found = self
                .ss
                .search_items(HashMap::from([("service", SERVICE)]))
                .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            let mut names: Vec<String> = Vec::new();
            for item in found.unlocked.iter().chain(found.locked.iter()) {
                if let Ok(attrs) = item.get_attributes() {
                    if let Some(name) = attrs.get("name") {
                        names.push(name.clone());
                    }
                }
            }
            names.sort();
            names.dedup();
            Ok(names)
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
// security-framework's `passwords` API, so the value never touches argv
// (unlike the Python backend's `security -w VALUE`, briefly visible in `ps`
// to this user). `list` still shells out to `security dump-keychain` (read-
// only, prints no secret values): the crate has no service-scoped
// enumeration call at this level.
#[cfg(target_os = "macos")]
pub mod macos {
    use super::*;
    use security_framework::os::macos::keychain::SecKeychain;
    use security_framework::passwords::{
        delete_generic_password, generic_password, set_generic_password, PasswordOptions,
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

        fn put(&self, secret: &str, value: &str) -> Result<()> {
            unlock_default_keychain()?;
            set_generic_password(SERVICE, secret, value.as_bytes())
                .map_err(|e| Error::msg(format!("keychain: {e}")))
        }

        fn get(&self, secret: &str) -> Result<Option<String>> {
            unlock_default_keychain()?;
            match generic_password(PasswordOptions::new_generic_password(SERVICE, secret)) {
                Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
                Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
                Err(e) => Err(Error::msg(format!("keychain: {e}"))),
            }
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
        CredDeleteW, CredEnumerateW, CredFree, CredReadW, CredWriteW, CREDENTIALW,
        CRED_ENUMERATE_ALL_CREDENTIALS, CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    fn target_name(secret: &str) -> Vec<u16> {
        wide(&format!("{SERVICE}/{secret}"))
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
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
    // only the logged-in user session.
    impl Backend for CredentialManagerBackend {
        fn name(&self) -> &'static str {
            "dpapi"
        }

        fn put(&self, secret: &str, value: &str) -> Result<()> {
            let mut target = target_name(secret);
            let mut username = wide(SERVICE);
            let mut blob = value.as_bytes().to_vec();
            let credential = CREDENTIALW {
                Flags: CRED_FLAGS(0),
                Type: CRED_TYPE_GENERIC,
                TargetName: PWSTR(target.as_mut_ptr()),
                Comment: PWSTR::null(),
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
                        CredFree(ptr as *mut _);
                        Ok(Some(value))
                    }
                    Err(e) if e.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => Ok(None),
                    Err(e) => Err(Error::msg(format!("credential manager: {e}"))),
                }
            }
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
            let filter = wide(&format!("{SERVICE}/*"));
            unsafe {
                let mut count: u32 = 0;
                let mut ptr: *mut *mut CREDENTIALW = std::ptr::null_mut();
                match CredEnumerateW(
                    PWSTR(filter.as_ptr() as *mut _),
                    Some(CRED_ENUMERATE_ALL_CREDENTIALS),
                    &mut count,
                    &mut ptr,
                ) {
                    Ok(()) => {}
                    // An empty store, or nothing matching the filter, comes
                    // back as ERROR_NOT_FOUND rather than a zero count.
                    Err(e) if e.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => {
                        return Ok(Vec::new())
                    }
                    Err(e) => return Err(Error::msg(format!("credential manager: {e}"))),
                }
                let mut names = Vec::new();
                let entries = std::slice::from_raw_parts(ptr, count as usize);
                for entry in entries {
                    let cred = &**entry;
                    let target = cred.TargetName.to_string().unwrap_or_default();
                    if let Some(name) = target.strip_prefix(&format!("{SERVICE}/")) {
                        names.push(name.to_string());
                    }
                }
                CredFree(ptr as *mut _);
                names.sort();
                names.dedup();
                Ok(names)
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
}
