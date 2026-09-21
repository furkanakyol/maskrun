use std::collections::HashMap;

use crate::error::{Error, Result};

pub const SERVICE: &str = "maskrun";

pub trait Backend {
    fn name(&self) -> &'static str;
    fn put(&self, secret: &str, value: &str) -> Result<()>;
    fn get(&self, secret: &str) -> Result<Option<String>>;
    fn delete(&self, secret: &str) -> Result<()>;
    fn list(&self) -> Result<Vec<String>>;
}

// `^[A-Za-z0-9][A-Za-z0-9._-]*$`, written by hand instead of with `regex` —
// `regex` is kept for mask.rs/guard.rs, not needed for one fixed pattern.
pub fn check_name(secret: &str) -> Result<&str> {
    let mut chars = secret.chars();
    let ok = match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => chars
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'),
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
    Err(Error::msg("the secret-service backend is only available on Linux"))
}

#[cfg(target_os = "macos")]
fn keychain_backend() -> Result<Box<dyn Backend>> {
    Ok(Box::new(macos::KeychainBackend::new()))
}
#[cfg(not(target_os = "macos"))]
fn keychain_backend() -> Result<Box<dyn Backend>> {
    Err(Error::msg("the keychain backend is only available on macOS"))
}

#[cfg(target_os = "windows")]
fn credential_manager_backend() -> Result<Box<dyn Backend>> {
    Ok(Box::new(windows::CredentialManagerBackend::new()))
}
#[cfg(not(target_os = "windows"))]
fn credential_manager_backend() -> Result<Box<dyn Backend>> {
    Err(Error::msg("the dpapi/credential-manager backend is only available on Windows"))
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
            let ss = SecretService::connect(EncryptionType::Plain).map_err(|e| {
                Error::msg(format!(
                    "could not reach the Secret Service ({e}). A running secret \
                     service (gnome-keyring or KeePassXC) is required."
                ))
            })?;
            Ok(Self { ss })
        }

        fn search_attrs(secret: &str) -> HashMap<&str, &str> {
            HashMap::from([("service", SERVICE), ("name", secret)])
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
            let found = self
                .ss
                .search_items(Self::search_attrs(secret))
                .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
            let item = match found.unlocked.first().or_else(|| found.locked.first()) {
                Some(item) => item,
                None => return Ok(None),
            };
            if item.is_locked().unwrap_or(false) {
                item.unlock()
                    .map_err(|e| Error::msg(format!("secret-service: {e}")))?;
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
    use security_framework::passwords::{
        delete_generic_password, generic_password, set_generic_password,
    };
    use std::process::Command;

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

    impl Backend for KeychainBackend {
        fn name(&self) -> &'static str {
            "keychain"
        }

        fn put(&self, secret: &str, value: &str) -> Result<()> {
            set_generic_password(SERVICE, secret, value.as_bytes())
                .map_err(|e| Error::msg(format!("keychain: {e}")))
        }

        fn get(&self, secret: &str) -> Result<Option<String>> {
            match generic_password(SERVICE, secret) {
                Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
                Err(e) if e.code() == security_framework::base::errSecItemNotFound => Ok(None),
                Err(e) => Err(Error::msg(format!("keychain: {e}"))),
            }
        }

        fn delete(&self, secret: &str) -> Result<()> {
            match delete_generic_password(SERVICE, secret) {
                Ok(()) => Ok(()),
                Err(e) if e.code() == security_framework::base::errSecItemNotFound => Ok(()),
                Err(e) => Err(Error::msg(format!("keychain: {e}"))),
            }
        }

        fn list(&self) -> Result<Vec<String>> {
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
    use ::windows::core::PWSTR;
    use ::windows::Win32::Foundation::{ERROR_NOT_FOUND, FILETIME};
    use ::windows::Win32::Security::Credentials::{
        CredDeleteW, CredEnumerateW, CredFree, CredReadW, CredWriteW, CREDENTIALW,
        CRED_ENUMERATE_ALL_CREDENTIALS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
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

    impl Backend for CredentialManagerBackend {
        fn name(&self) -> &'static str {
            "dpapi"
        }

        fn put(&self, secret: &str, value: &str) -> Result<()> {
            let mut target = target_name(secret);
            let mut username = wide(SERVICE);
            let mut blob = value.as_bytes().to_vec();
            let credential = CREDENTIALW {
                Flags: 0,
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
                match CredReadW(PWSTR(target.as_ptr() as *mut _), CRED_TYPE_GENERIC, None, &mut ptr)
                {
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
                    Err(e) if e.code().0 as u32 == ERROR_NOT_FOUND.0 => Ok(None),
                    Err(e) => Err(Error::msg(format!("credential manager: {e}"))),
                }
            }
        }

        fn delete(&self, secret: &str) -> Result<()> {
            let target = target_name(secret);
            unsafe { CredDeleteW(PWSTR(target.as_ptr() as *mut _), CRED_TYPE_GENERIC, None) }
                .or_else(|e| {
                    if e.code().0 as u32 == ERROR_NOT_FOUND.0 {
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
                CredEnumerateW(
                    PWSTR(filter.as_ptr() as *mut _),
                    CRED_ENUMERATE_ALL_CREDENTIALS.0,
                    &mut count,
                    &mut ptr,
                )
                .map_err(|e| Error::msg(format!("credential manager: {e}")))?;
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
