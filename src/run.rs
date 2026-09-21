// maskrun run / exec. Ports bin/maskrun's cmd_run, cmd_exec, collect and
// exec_with_env.

use std::collections::HashMap;

use crate::agent;
use crate::error::{Error, Result};
use crate::keyring::{check_name, Backend};
use crate::manifest;

pub fn cmd_run(command: &[String], raw: bool, mask: bool, backend: &dyn Backend) -> Result<i32> {
    if command.is_empty() {
        return Err(Error::msg("nothing to run. Usage: maskrun run -- <command>"));
    }
    let path = manifest::require_manifest()?;
    let pairs = manifest::read_manifest(&path)?;
    let injected = collect(&pairs, backend)?;
    dispatch(command, injected, raw, mask)
}

pub fn cmd_exec(
    assignments: &[String],
    command: &[String],
    raw: bool,
    mask: bool,
    backend: &dyn Backend,
) -> Result<i32> {
    if command.is_empty() {
        return Err(Error::msg(
            "nothing to run. Usage: maskrun exec VAR=secret-name -- <command>",
        ));
    }
    if assignments.is_empty() {
        return Err(Error::msg(
            "no VAR=secret-name given. Usage: maskrun exec VAR=name -- <command>",
        ));
    }
    let mut pairs = Vec::with_capacity(assignments.len());
    for item in assignments {
        let (var, secret) = item
            .split_once('=')
            .ok_or_else(|| Error::msg(format!("expected VAR=secret-name, got {item:?}")))?;
        pairs.push((var.trim().to_string(), check_name(secret.trim())?.to_string()));
    }
    let injected = collect(&pairs, backend)?;
    dispatch(command, injected, raw, mask)
}

fn dispatch(
    command: &[String],
    injected: HashMap<String, String>,
    raw: bool,
    force_mask: bool,
) -> Result<i32> {
    if agent::should_mask(raw, force_mask) {
        crate::mask::run_masked(command, injected)
    } else {
        exec_with_env(command, &injected)
    }
}

fn collect(pairs: &[(String, String)], backend: &dyn Backend) -> Result<HashMap<String, String>> {
    let mut injected = HashMap::with_capacity(pairs.len());
    let mut missing = Vec::new();
    for (var, secret) in pairs {
        match backend.get(secret)? {
            Some(value) if !value.is_empty() => {
                injected.insert(var.clone(), value);
            }
            _ => missing.push((var.clone(), secret.clone())),
        }
    }
    if missing.is_empty() {
        return Ok(injected);
    }

    let lines = missing
        .iter()
        .map(|(var, secret)| format!("    {var:<28} -> {secret}"))
        .collect::<Vec<_>>()
        .join("\n");

    // A locked keyring returns "not found" for everything in it, which looks
    // exactly like an empty one from here — check before telling the user to
    // `put` a secret that may already exist behind the lock, which would
    // have them overwrite it instead of just unlocking.
    if backend.is_locked().unwrap_or(false) {
        return Err(Error::msg(format!(
            "the keyring appears to be locked, so maskrun cannot tell whether these secrets \
             exist:\n{lines}\nUnlock it (log back in, or open your keyring/keychain manager) \
             and try again. Do not `maskrun put` them again — a locked keyring is not the \
             same as an empty one."
        )));
    }

    Err(Error::msg(format!(
        "refusing to run with a half-filled environment. Missing from the keyring:\n\
         {lines}\nAdd them with: maskrun put <name>"
    )))
}

#[cfg(unix)]
pub fn exec_with_env(command: &[String], injected: &HashMap<String, String>) -> Result<i32> {
    use std::os::unix::process::CommandExt;
    // Replaces this process outright (execvp under the hood) instead of
    // spawning and waiting, so the child's signals and exit status are the
    // real ones — nothing here survives to translate them.
    let err = std::process::Command::new(&command[0])
        .args(&command[1..])
        .envs(injected)
        .exec();
    if err.kind() == std::io::ErrorKind::NotFound {
        return Err(Error::msg(format!("command not found: {}", command[0])));
    }
    Err(Error::from(err))
}

#[cfg(not(unix))]
pub fn exec_with_env(command: &[String], injected: &HashMap<String, String>) -> Result<i32> {
    let status = std::process::Command::new(&command[0])
        .args(&command[1..])
        .envs(injected)
        .status()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::msg(format!("command not found: {}", command[0]))
            } else {
                Error::from(e)
            }
        })?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error as MaskrunError;

    #[derive(Default)]
    struct FakeBackend {
        values: HashMap<&'static str, &'static str>,
        locked: bool,
    }

    impl Backend for FakeBackend {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn put(&self, _secret: &str, _value: &str) -> Result<()> {
            Ok(())
        }
        fn get(&self, secret: &str) -> Result<Option<String>> {
            Ok(self.values.get(secret).map(|v| v.to_string()))
        }
        fn delete(&self, _secret: &str) -> Result<()> {
            Ok(())
        }
        fn list(&self) -> Result<Vec<String>> {
            Ok(self.values.keys().map(|s| s.to_string()).collect())
        }
        fn is_locked(&self) -> Result<bool> {
            Ok(self.locked)
        }
    }

    #[test]
    fn collect_refuses_half_filled_environment() {
        let backend = FakeBackend { values: HashMap::from([("present", "value")]), ..Default::default() };
        let pairs = vec![
            ("A".to_string(), "present".to_string()),
            ("B".to_string(), "absent".to_string()),
        ];
        let err = collect(&pairs, &backend).unwrap_err();
        assert!(err.to_string().contains("half-filled"));
        assert!(err.to_string().contains("B"));
    }

    #[test]
    fn collect_reports_locked_keyring_instead_of_suggesting_put() {
        let backend = FakeBackend {
            values: HashMap::from([("present", "value")]),
            locked: true,
        };
        let pairs = vec![
            ("A".to_string(), "present".to_string()),
            ("B".to_string(), "absent".to_string()),
        ];
        let err = collect(&pairs, &backend).unwrap_err().to_string();
        assert!(err.contains("locked"), "{err}");
        assert!(!err.contains("half-filled"), "{err}");
        // The unlocked-and-missing message's actual suggestion; this one
        // must not make it, even though it mentions `maskrun put` itself to
        // tell the user not to.
        assert!(!err.contains("Add them with"), "{err}");
    }

    #[test]
    fn collect_ignores_lock_state_when_nothing_is_missing() {
        // A locked keyring must not turn a fully successful run into an
        // error just because is_locked() happens to report true (e.g. some
        // unrelated collection is locked) — the lock check only kicks in
        // once something has actually failed to resolve.
        let backend = FakeBackend {
            values: HashMap::from([("present", "value")]),
            locked: true,
        };
        let pairs = vec![("A".to_string(), "present".to_string())];
        let injected = collect(&pairs, &backend).unwrap();
        assert_eq!(injected.get("A"), Some(&"value".to_string()));
    }

    #[test]
    fn collect_treats_empty_value_as_missing() {
        let backend = FakeBackend { values: HashMap::from([("empty", "")]), ..Default::default() };
        let pairs = vec![("A".to_string(), "empty".to_string())];
        let err = collect(&pairs, &backend).unwrap_err();
        assert!(err.to_string().contains("half-filled"));
    }

    #[test]
    fn collect_succeeds_when_everything_present() {
        let backend = FakeBackend { values: HashMap::from([("present", "value")]), ..Default::default() };
        let pairs = vec![("A".to_string(), "present".to_string())];
        let injected = collect(&pairs, &backend).unwrap();
        assert_eq!(injected.get("A"), Some(&"value".to_string()));
    }

    #[test]
    fn cmd_run_rejects_empty_command() {
        let backend = FakeBackend { values: HashMap::new(), ..Default::default() };
        let err = cmd_run(&[], false, false, &backend).unwrap_err();
        assert!(matches!(err, MaskrunError::User(_)));
        assert!(err.to_string().contains("nothing to run"));
    }

    #[test]
    fn cmd_exec_rejects_missing_assignment() {
        let backend = FakeBackend { values: HashMap::new(), ..Default::default() };
        let err =
            cmd_exec(&[], &["echo".to_string()], false, false, &backend).unwrap_err();
        assert!(err.to_string().contains("no VAR=secret-name given"));
    }

    #[test]
    fn cmd_exec_rejects_malformed_assignment() {
        let backend = FakeBackend { values: HashMap::new(), ..Default::default() };
        let err = cmd_exec(
            &["not-an-assignment".to_string()],
            &["echo".to_string()],
            false,
            false,
            &backend,
        )
        .unwrap_err();
        assert!(err.to_string().contains("expected VAR=secret-name"));
    }
}
