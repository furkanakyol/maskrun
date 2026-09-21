// PreToolUse guard for agent harnesses (`maskrun hook`). Port of bin/maskrun's
// guard_decision, with one behavioral fix: GUARD_RULES there runs a regex
// over the *whole* command string, so `sed 's/maskrun get/x/' notes.md` gets
// blocked even though it never calls `maskrun get` — the text just happens to
// sit inside another command's argument. Here every rule is checked against
// pipeline_segments() the same way the pre-existing .env check already was,
// so a rule only fires when the flagged command is the one actually invoked
// (or, for env-var rules, actually assigned) in that segment. This shrinks
// false positives but also shrinks what the guard catches — see README's
// "What this is not": it still won't stop `echo "..." | sh`.

use std::io::Read;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::error::Result;

const READERS: &[&str] = &[
    "cat", "bat", "tac", "nl", "less", "more", "head", "tail", "strings", "xxd", "od", "hexdump",
    "vi", "vim", "view", "nvim", "nano", "emacs", "jq", "yq", "dotenv", "source", "grep", "egrep",
    "fgrep", "rg", "ag", "sed", "awk", "cut", "diff", "type", "gc",
];
// Their first positional argument is a pattern/script, not a file — see
// drop_pattern_argument.
const PATTERN_READERS: &[&str] = &["grep", "egrep", "fgrep", "rg", "ag", "sed", "awk"];
const SEPARATORS: &[&str] = &["|", "||", "&&", ";", "&", "|&"];

const REASON_MASKRUN_GET: &str = "`maskrun get` prints the value to stdout, which lands directly in the transcript. To USE a secret: `maskrun run -- <command>` or `maskrun exec VAR=name -- <command>`. To see what exists: `maskrun list`.";
const REASON_MASKRUN_IMPORT: &str = "`maskrun import` reads the whole .env, so its contents enter the transcript. Let the human run this in their own terminal.";
const REASON_SECRET_TOOL: &str = "`secret-tool lookup/search` reads the keyring directly and prints the value. Use `maskrun list` for names.";
const REASON_SECURITY_FIND: &str = "`security find-generic-password` prints the keychain value. Use `maskrun list` for names.";
const REASON_RAW: &str = "`--raw` turns output masking off, so a secret can leak into the transcript. Drop the flag — masking is automatic in an agent session.";
const REASON_MASK_ZERO: &str = "MASKRUN_MASK=0 turns output masking off. Do not set it.";
const REASON_ALLOW_READ: &str = "MASKRUN_ALLOW_READ=1 re-enables `maskrun get` inside an agent session, which defeats the guard.";
const REASON_PROC_ENVIRON: &str = "/proc/<pid>/environ dumps a running process's environment, which is where injected secrets live.";
const REASON_BARE_ENV: &str = "A bare `env` / `printenv` dumps the whole environment, injected secrets included. Name the variable you want (`printenv PATH`).";
const REASON_POWERSHELL_ENV: &str = "`Get-ChildItem Env:` dumps the whole environment in PowerShell. Read a single variable instead ($env:PATH).";

fn harmless_env_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\.env\.(example|sample|template|dist|defaults?|tpl|ornek)$").unwrap()
    })
}

fn env_path_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[\w./~-]*\.env(?:\.[\w-]+)*\b|[\w./~-]*\.envrc\b").unwrap())
}

fn proc_environ_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"/proc/(?:\d+|self|\$\w+|\$\{\w+\})/environ").unwrap())
}

fn heredoc_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"<<-?\s*['"]?(\w+)"#).unwrap())
}

fn env_assign_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=").unwrap())
}

fn mask_zero_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^MASKRUN_MASK\s*=\s*0$").unwrap())
}

fn allow_read_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^MASKRUN_ALLOW_READ\s*=\s*1$").unwrap())
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

// Drops a leading run of `VAR=x` assignments and sudo/command/exec/time,
// same as bin/maskrun's segment_command, but also hands back that prefix
// (for the MASKRUN_MASK=0 / MASKRUN_ALLOW_READ=1 checks) and the words after
// the command (for subcommand checks) instead of just the command name.
fn split_segment(segment: &[String]) -> (&[String], String, &[String]) {
    let mut idx = 0;
    while idx < segment.len() {
        let word = &segment[idx];
        if env_assign_pattern().is_match(word) || matches!(word.as_str(), "sudo" | "command" | "exec" | "time") {
            idx += 1;
            continue;
        }
        break;
    }
    if idx >= segment.len() {
        return (&segment[..idx], String::new(), &segment[idx..]);
    }
    let name = basename(&segment[idx]).to_lowercase();
    (&segment[..idx], name, &segment[idx + 1..])
}

// Best-effort POSIX-ish word split with quote handling. Falls back to a
// plain whitespace split on unbalanced quotes, same as bin/maskrun falling
// back from a shlex.split ValueError — malformed input must never crash the
// guard into blocking (or failing to check) everything.
fn shlex_split(command: &str) -> Vec<String> {
    try_shlex_split(command).unwrap_or_else(|| command.split_whitespace().map(String::from).collect())
}

fn try_shlex_split(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => current.push(ch),
                        None => return None,
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.peek() {
                            Some('"') | Some('\\') => current.push(chars.next().unwrap()),
                            _ => current.push('\\'),
                        },
                        Some(ch) => current.push(ch),
                        None => return None,
                    }
                }
            }
            '\\' => {
                in_word = true;
                current.push(chars.next()?);
            }
            _ => {
                in_word = true;
                current.push(c);
            }
        }
    }
    if in_word {
        words.push(current);
    }
    Some(words)
}

fn pipeline_segments(command: &str) -> Vec<Vec<String>> {
    let words = shlex_split(command);
    let mut groups: Vec<Vec<String>> = vec![Vec::new()];
    for word in words {
        if SEPARATORS.contains(&word.as_str()) {
            groups.push(Vec::new());
        } else {
            groups.last_mut().unwrap().push(word);
        }
    }
    groups.into_iter().filter(|g| !g.is_empty()).collect()
}

// `grep -E "\.env"` searches FOR the text ".env"; it does not read a file
// called that. Options are skipped on the way to the pattern.
fn drop_pattern_argument(args: &[String]) -> Vec<String> {
    let mut remaining = Vec::new();
    let mut dropped = false;
    for word in args {
        if !dropped {
            if word.starts_with('-') {
                continue;
            }
            dropped = true;
            continue;
        }
        remaining.push(word.clone());
    }
    remaining
}

// A body is data, not a command: without this, a note that merely mentions
// `cat .env` or `maskrun get` tripped the guard.
fn strip_heredocs(command: &str) -> String {
    let lines: Vec<&str> = command.split('\n').collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        kept.push(line);
        let tags: Vec<String> = heredoc_pattern()
            .captures_iter(line)
            .map(|c| c[1].to_string())
            .collect();
        i += 1;
        for tag in tags {
            while i < lines.len() && lines[i].trim() != tag {
                i += 1;
            }
            i += 1;
        }
    }
    kept.join("\n")
}

fn segment_rule_reason(segment: &[String]) -> Option<&'static str> {
    let (prefix, name, args) = split_segment(segment);

    if name == "maskrun" {
        match args.first().map(String::as_str) {
            Some("get") => return Some(REASON_MASKRUN_GET),
            Some("import") => return Some(REASON_MASKRUN_IMPORT),
            Some("run") | Some("exec") if segment.iter().any(|w| w == "--raw") => {
                return Some(REASON_RAW);
            }
            _ => {}
        }
    }
    if name == "secret-tool" && matches!(args.first().map(String::as_str), Some("lookup") | Some("search")) {
        return Some(REASON_SECRET_TOOL);
    }
    if name == "security" && args.first().map(String::as_str) == Some("find-generic-password") {
        return Some(REASON_SECURITY_FIND);
    }
    if prefix.iter().any(|w| mask_zero_pattern().is_match(w)) {
        return Some(REASON_MASK_ZERO);
    }
    if prefix.iter().any(|w| allow_read_pattern().is_match(w)) {
        return Some(REASON_ALLOW_READ);
    }
    if segment.iter().any(|w| proc_environ_pattern().is_match(w)) {
        return Some(REASON_PROC_ENVIRON);
    }
    if matches!(name.as_str(), "env" | "printenv") && args.is_empty() {
        return Some(REASON_BARE_ENV);
    }
    if matches!(name.as_str(), "get-childitem" | "gci" | "ls" | "dir")
        && args.first().map(|w| w.to_lowercase().starts_with("env:")).unwrap_or(false)
    {
        return Some(REASON_POWERSHELL_ENV);
    }
    None
}

fn reads_env_file(command: &str) -> Option<String> {
    for segment in pipeline_segments(command) {
        let (_, name, _) = split_segment(&segment);
        if !READERS.contains(&name.as_str()) {
            continue;
        }
        let raw_args: &[String] = if segment.len() > 1 { &segment[1..] } else { &[] };
        let args = if PATTERN_READERS.contains(&name.as_str()) {
            drop_pattern_argument(raw_args)
        } else {
            raw_args.to_vec()
        };
        for word in &args {
            for m in env_path_pattern().find_iter(word) {
                let found = m.as_str();
                if !found.is_empty() && !harmless_env_pattern().is_match(found) {
                    return Some(format!(
                        "'{found}' is a .env file; reading it puts secrets in the transcript. \
                         Check what the keyring holds with `maskrun status` and run the command \
                         with `maskrun run -- <command>`. Templates such as .env.example are fine to read."
                    ));
                }
            }
        }
    }
    None
}

fn guard_decision(payload: &Value) -> Option<String> {
    let tool = payload.get("tool_name").and_then(Value::as_str).unwrap_or("");

    if tool == "Bash" || tool == "BashOutput" {
        let command = payload
            .get("tool_input")
            .and_then(|v| v.get("command"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let clean = strip_heredocs(command);
        for segment in pipeline_segments(&clean) {
            if let Some(reason) = segment_rule_reason(&segment) {
                return Some(reason.to_string());
            }
        }
        return reads_env_file(&clean);
    }

    if matches!(tool, "Read" | "Edit" | "Write" | "NotebookEdit") {
        let path = payload
            .get("tool_input")
            .and_then(|v| v.get("file_path").or_else(|| v.get("notebook_path")))
            .and_then(Value::as_str)
            .unwrap_or("");
        let base = basename(path);
        let looks_env = base == ".env" || base.starts_with(".env.") || base == ".envrc";
        if looks_env && !harmless_env_pattern().is_match(base) {
            return Some(format!(
                "'{path}' is a .env file; reading it puts secrets in the transcript. Use \
                 `maskrun status` to see what the keyring holds and `maskrun run -- <command>` \
                 to run with them."
            ));
        }
    }

    None
}

pub fn cmd_hook() -> Result<i32> {
    let mut input = String::new();
    // Unreadable or non-UTF8 stdin must never block work, same as unparseable JSON below.
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return Ok(0);
    }
    let payload: Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return Ok(0),
    };
    if let Some(reason) = guard_decision(&payload) {
        let output = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": format!("maskrun guard: {reason}"),
            }
        });
        println!("{output}");
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decide(tool: &str, field: &str, value: &str) -> Option<String> {
        guard_decision(&serde_json::json!({"tool_name": tool, "tool_input": {field: value}}))
    }

    #[test]
    fn blocks_maskrun_get_call() {
        assert!(decide("Bash", "command", "maskrun get my-api-key").is_some());
    }

    #[test]
    fn allows_maskrun_get_mentioned_in_another_commands_argument() {
        assert!(decide("Bash", "command", "sed 's/maskrun get/x/' notes.md").is_none());
        assert!(decide("Bash", "command", "grep -r \"maskrun get\" docs/").is_none());
        assert!(decide("Bash", "command", "echo \"run maskrun get in your own terminal\"").is_none());
    }

    #[test]
    fn allows_printf_literal_that_looks_like_the_mask_zero_assignment() {
        assert!(decide("Bash", "command", "printf '%s\\n' \"MASKRUN_MASK=0\"").is_none());
    }

    #[test]
    fn blocks_mask_zero_env_assignment_prefix() {
        assert!(decide("Bash", "command", "MASKRUN_MASK=0 maskrun run -- npm run dev").is_some());
    }

    #[test]
    fn strip_heredocs_removes_body_but_keeps_surrounding_command() {
        let command = "cat <<'EOF' > f\nsecret .env line\nEOF\nls";
        let cleaned = strip_heredocs(command);
        assert!(!cleaned.contains("secret .env line"));
        assert!(cleaned.contains("ls"));
    }

    #[test]
    fn drop_pattern_argument_drops_only_the_pattern() {
        assert!(drop_pattern_argument(&["-E".into(), "\\.env".into()]).is_empty());
        assert_eq!(
            drop_pattern_argument(&["DATABASE".into(), ".env".into()]),
            vec![".env".to_string()]
        );
    }

    #[test]
    fn pipeline_segments_splits_on_separators() {
        let segments = pipeline_segments("ls -la | grep -E \"\\.env\"");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0], vec!["ls".to_string(), "-la".to_string()]);
        assert_eq!(segments[1][0], "grep");
    }

    #[test]
    fn hook_never_blocks_on_garbage_input() {
        // Exercised end to end in tests/guard.rs; this pins guard_decision
        // never panicking on a shape it doesn't recognize.
        let weird = serde_json::json!(["not", "an", "object"]);
        assert!(guard_decision(&weird).is_none());
    }
}
