// maskrun install-guard. Port of bin/maskrun's cmd_install_guard.
//
// serde_json's preserve_order feature (Cargo.toml) backs Value::Object with
// an order-preserving map, so rewriting someone's settings.json here does
// not reorder keys they already had — the Python version relies on the same
// property from a plain dict.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::{Error, Result};

const HOOK_MATCHER: &str = "Bash|Read|Edit|Write|NotebookEdit";

pub struct InstallGuardArgs<'a> {
    pub harness: &'a str,
    pub remove: bool,
    pub dry_run: bool,
    pub config: Option<&'a str>,
    pub command_path: Option<&'a str>,
}

fn default_settings_path() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(dir).join("settings.json");
    }
    // No `dirs`-style crate: HOME (and USERPROFILE on Windows) is enough for
    // the one path this needs, and matches Python's os.path.expanduser("~").
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    Path::new(&home).join(".claude").join("settings.json")
}

fn is_ours(block: &Value) -> bool {
    block
        .get("hooks")
        .and_then(Value::as_array)
        .map(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(Value::as_str)
                    .map(|c| c.contains("maskrun"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

pub fn cmd_install_guard(args: InstallGuardArgs<'_>) -> Result<i32> {
    if args.harness != "claude-code" {
        return Err(Error::msg(format!(
            "unknown harness '{}' (only claude-code so far)",
            args.harness
        )));
    }

    let path: PathBuf = match args.config {
        Some(c) => PathBuf::from(c),
        None => default_settings_path(),
    };

    let mut settings: Value = if path.exists() {
        let content = fs::read_to_string(&path)?;
        let trimmed = content.trim();
        if trimmed.is_empty() {
            Value::Object(Map::new())
        } else {
            match serde_json::from_str::<Value>(trimmed) {
                Ok(v) if v.is_object() => v,
                Ok(_) => {
                    return Err(Error::msg(format!("{} does not contain a JSON object", path.display())));
                }
                Err(e) => {
                    return Err(Error::msg(format!(
                        "{} is not valid JSON ({e}). Fix or move it first — refusing to \
                         overwrite a broken config, since that would silently disable every \
                         setting in it.",
                        path.display()
                    )));
                }
            }
        }
    } else {
        Value::Object(Map::new())
    };

    let settings_obj = settings.as_object_mut().expect("checked above");
    let hooks_val = settings_obj.entry("hooks").or_insert_with(|| Value::Object(Map::new()));
    let hooks_obj = hooks_val
        .as_object_mut()
        .ok_or_else(|| Error::msg(format!("{}: hooks is not an object", path.display())))?;
    let pre_val = hooks_obj.entry("PreToolUse").or_insert_with(|| Value::Array(Vec::new()));
    let pre = pre_val
        .as_array_mut()
        .ok_or_else(|| Error::msg(format!("{}: hooks.PreToolUse is not a list", path.display())))?;

    let existing: Vec<usize> = pre
        .iter()
        .enumerate()
        .filter(|(_, block)| block.is_object() && is_ours(block))
        .map(|(i, _)| i)
        .collect();

    if args.remove {
        if existing.is_empty() {
            println!("maskrun guard is not installed in {}", path.display());
            return Ok(0);
        }
        for &index in existing.iter().rev() {
            pre.remove(index);
        }
        if pre.is_empty() {
            hooks_obj.remove("PreToolUse");
        }
        if hooks_obj.is_empty() {
            settings_obj.remove("hooks");
        }
    } else {
        let command_path = args.command_path.unwrap_or("maskrun");
        let entry = serde_json::json!({
            "matcher": HOOK_MATCHER,
            "hooks": [{
                "type": "command",
                "command": format!("{command_path} hook"),
                "timeout": 10,
                "statusMessage": "maskrun guard",
            }],
        });
        if !existing.is_empty() {
            pre[existing[0]] = entry;
            for &index in existing[1..].iter().rev() {
                pre.remove(index);
            }
        } else {
            pre.push(entry);
        }
    }

    if args.dry_run {
        println!("--- {} would become ---", path.display());
        println!("{}", serde_json::to_string_pretty(&settings)?);
        println!("--- dry run: nothing was written ---");
        return Ok(0);
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    if path.exists() {
        let backup = PathBuf::from(format!("{}.maskrun-backup", path.display()));
        fs::copy(&path, &backup)?;
        println!("backed up: {}", backup.display());
    }

    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    let mut serialized = serde_json::to_string_pretty(&settings)?;
    serialized.push('\n');
    fs::write(&tmp, serialized)?;
    fs::rename(&tmp, &path)?;

    if args.remove {
        println!("removed the maskrun guard from {}", path.display());
    } else {
        println!("installed the maskrun guard in {}", path.display());
        println!("Other hooks in that file were left untouched.");
        println!("Restart Claude Code (or open /hooks once) to load it.");
    }
    Ok(0)
}
