// Integration tests for `maskrun hook` and `maskrun install-guard`, run
// against the compiled binary. The BLOCK/ALLOW tables are a straight port of
// test/test_maskrun.py's tables of the same name; REGRESSION_BLOCK and
// REGRESSION_ALLOW pin the fix this session made: GUARD_RULES now matches
// against pipeline segments instead of the whole command string, so a
// command that only *mentions* "maskrun get" or "MASKRUN_MASK=0" in an
// argument is no longer confused with actually calling/setting it.

use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{n}", std::process::id())
}

fn run(args: &[&str], env_extra: &[(&str, &str)], stdin_data: Option<&[u8]>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_maskrun"));
    cmd.args(args);
    for var in [
        "MASKRUN_AGENT",
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "AI_AGENT",
        "AIDER_CHAT",
        "CURSOR_AGENT",
        "OPENAI_CODEX",
        "GEMINI_CLI",
        "REPLIT_AGENT",
        "MASKRUN_MASK",
        "MASKRUN_ALLOW_READ",
        "MASKRUN_BACKEND",
    ] {
        cmd.env_remove(var);
    }
    for (k, v) in env_extra {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("failed to spawn maskrun binary");
    match stdin_data {
        Some(data) => child.stdin.take().unwrap().write_all(data).unwrap(),
        None => drop(child.stdin.take()),
    }
    child.wait_with_output().expect("failed to wait on maskrun")
}

fn hook_output(payload: &Value) -> Output {
    run(&["hook"], &[], Some(payload.to_string().as_bytes()))
}

fn is_denied(tool: &str, field: &str, value: &str) -> bool {
    let payload = json!({"tool_name": tool, "tool_input": {field: value}});
    let out = hook_output(&payload);
    assert!(
        out.status.success(),
        "hook must always exit 0, got {:?}",
        out.status
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return false;
    }
    let decision: Value = serde_json::from_str(trimmed)
        .unwrap_or_else(|e| panic!("hook stdout must be JSON when non-empty ({e}): {trimmed}"));
    decision["hookSpecificOutput"]["permissionDecision"] == "deny"
}

use serde_json::{json, Value};

type Case = (&'static str, &'static str, &'static str, &'static str);

// Port of test/test_maskrun.py's BLOCK table (28 entries).
const BLOCK: &[Case] = &[
    ("maskrun get", "Bash", "command", "maskrun get my-api-key"),
    (
        "secret-tool lookup",
        "Bash",
        "command",
        "secret-tool lookup service maskrun name x",
    ),
    (
        "secret-tool search",
        "Bash",
        "command",
        "secret-tool search --all service maskrun",
    ),
    (
        "security find-generic-password",
        "Bash",
        "command",
        "security find-generic-password -s maskrun -a x -w",
    ),
    ("cat .env", "Bash", "command", "cat .env"),
    (
        "head .env.production",
        "Bash",
        "command",
        "head -5 apps/api/.env.production",
    ),
    (
        "cat .env.local piped",
        "Bash",
        "command",
        "cat .env.local | grep DB",
    ),
    (
        "source .env",
        "Bash",
        "command",
        "source .env && npm run dev",
    ),
    ("cat .envrc", "Bash", "command", "cat .envrc"),
    ("grep on .env", "Bash", "command", "grep DATABASE .env"),
    (
        "grep -E with a pattern then .env",
        "Bash",
        "command",
        "grep -E 'DB|KEY' .env",
    ),
    ("awk over .env", "Bash", "command", "awk '{print $1}' .env"),
    ("diff .env", "Bash", "command", "diff .env .env.example"),
    ("cat redirected from .env", "Bash", "command", "cat < .env"),
    ("sed on .env", "Bash", "command", "sed -n 1,5p .env"),
    (
        "/proc/pid/environ",
        "Bash",
        "command",
        "cat /proc/1234/environ",
    ),
    (
        "/proc/self/environ",
        "Bash",
        "command",
        "tr '\\0' '\\n' < /proc/self/environ",
    ),
    ("bare env", "Bash", "command", "env"),
    ("env piped", "Bash", "command", "env | grep -i key"),
    ("bare printenv", "Bash", "command", "printenv"),
    (
        "powershell env dump",
        "Bash",
        "command",
        "Get-ChildItem Env:",
    ),
    (
        "run --raw",
        "Bash",
        "command",
        "maskrun run --raw -- npm run dev",
    ),
    (
        "MASKRUN_MASK=0",
        "Bash",
        "command",
        "MASKRUN_MASK=0 maskrun run -- npm run dev",
    ),
    (
        "MASKRUN_ALLOW_READ=1",
        "Bash",
        "command",
        "MASKRUN_ALLOW_READ=1 maskrun get x",
    ),
    (
        "maskrun import",
        "Bash",
        "command",
        "maskrun import .env --prefix app",
    ),
    ("Read .env", "Read", "file_path", "/home/x/proj/.env"),
    (
        "Read .env.production",
        "Read",
        "file_path",
        "/home/x/proj/.env.production",
    ),
    ("Edit .env", "Edit", "file_path", "/home/x/proj/.env"),
];

// Port of test/test_maskrun.py's ALLOW table (30 entries).
const ALLOW: &[Case] = &[
    (
        "maskrun run",
        "Bash",
        "command",
        "maskrun run -- npm run dev",
    ),
    (
        "maskrun exec",
        "Bash",
        "command",
        "maskrun exec API=my-key -- curl https://api.example.com",
    ),
    ("maskrun list", "Bash", "command", "maskrun list"),
    ("maskrun status", "Bash", "command", "maskrun status"),
    ("maskrun put", "Bash", "command", "maskrun put new-key"),
    (
        "env VAR=1 cmd",
        "Bash",
        "command",
        "env NODE_ENV=test npm test",
    ),
    ("cat .env.example", "Bash", "command", "cat .env.example"),
    ("cat .env.sample", "Bash", "command", "cat .env.sample"),
    ("cat .maskrun", "Bash", "command", "cat .maskrun"),
    ("printenv PATH", "Bash", "command", "printenv PATH"),
    ("single powershell var", "Bash", "command", "echo $env:PATH"),
    ("ls -la .env", "Bash", "command", "ls -la .env"),
    ("rm .env", "Bash", "command", "rm .env"),
    (
        "write to .env",
        "Bash",
        "command",
        "echo \"DATABASE_URL=x\" > .env",
    ),
    ("grep in src", "Bash", "command", "grep -rn foo src/"),
    ("git status", "Bash", "command", "git status"),
    (
        "node --env-file",
        "Bash",
        "command",
        "node --env-file=.env server.js",
    ),
    (
        ".env in prose piped to head",
        "Bash",
        "command",
        "echo \"secrets live in the keyring, not .env\" | head -3",
    ),
    (
        ".env in prose piped to grep",
        "Bash",
        "command",
        "echo \"look at .env someday\" | grep -o env",
    ),
    (
        "ls piped to grep -E for .env",
        "Bash",
        "command",
        "ls -la | grep -E \"\\.env\"",
    ),
    (
        "ls piped to plain grep .env",
        "Bash",
        "command",
        "ls -la | grep '\\.env'",
    ),
    (
        "rg searching for the text .env",
        "Bash",
        "command",
        "rg .env",
    ),
    (
        "grep -r for .env across the tree",
        "Bash",
        "command",
        "grep -rn .env src/",
    ),
    (
        "wc counts lines without printing them",
        "Bash",
        "command",
        "wc -l .env",
    ),
    (
        "Read .env.example",
        "Read",
        "file_path",
        "/home/x/proj/.env.example",
    ),
    (
        "Read .maskrun",
        "Read",
        "file_path",
        "/home/x/proj/.maskrun",
    ),
    (
        "Read source file",
        "Read",
        "file_path",
        "/home/x/proj/src/index.ts",
    ),
    (
        "heredoc body then head",
        "Bash",
        "command",
        "cat <<'EOF' > note.txt\nDATABASE_URL is not in .env any more\nEOF\nls | head -3",
    ),
    (
        "heredoc documenting the guard",
        "Bash",
        "command",
        "cat <<'EOF' > doc.md\nWe used to run `maskrun get` and `cat .env`.\nEOF\nwc -l doc.md",
    ),
    (
        "python heredoc mentioning .env",
        "Bash",
        "command",
        "python3 - <<'PY'\ntext = \"\"\"\ncat .env is a leak\n\"\"\"\nPY\nbash x.sh | tail -5",
    ),
];

// The bug this session fixed: GUARD_RULES was a whole-string regex, so a
// `sed` command that merely mentions "maskrun get" inside its own argument
// got denied even though it never calls it. Must still deny the real thing.
const REGRESSION_BLOCK: &[Case] = &[
    ("bare maskrun get", "Bash", "command", "maskrun get x"),
    (
        "secret-tool lookup, generic",
        "Bash",
        "command",
        "secret-tool lookup service maskrun name x",
    ),
    (
        "maskrun run --raw, generic command",
        "Bash",
        "command",
        "maskrun run --raw -- cmd",
    ),
    (
        "MASKRUN_MASK=0, generic command",
        "Bash",
        "command",
        "MASKRUN_MASK=0 maskrun run -- cmd",
    ),
    (
        "cat /proc/self/environ directly",
        "Bash",
        "command",
        "cat /proc/self/environ",
    ),
    ("bare env, no pipe", "Bash", "command", "env"),
];

const REGRESSION_ALLOW: &[Case] = &[
    (
        "sed replacing the literal text maskrun get",
        "Bash",
        "command",
        "sed 's/maskrun get/X/' notes.md",
    ),
    (
        "grep searching docs for the text maskrun get",
        "Bash",
        "command",
        "grep -r \"maskrun get\" docs/",
    ),
    (
        "echo mentioning maskrun get in prose",
        "Bash",
        "command",
        "echo \"run maskrun get in your own terminal\"",
    ),
    (
        "commit message mentioning maskrun get",
        "Bash",
        "command",
        "git commit -m \"document maskrun get\"",
    ),
    (
        "printf argument that looks like the MASKRUN_MASK=0 assignment",
        "Bash",
        "command",
        "printf '%s\\n' \"MASKRUN_MASK=0\"",
    ),
];

#[test]
fn guard_blocks_known_cases() {
    let mut ran = 0;
    for (label, tool, field, value) in BLOCK {
        assert!(
            is_denied(tool, field, value),
            "should have been blocked: {label} -> {value}"
        );
        ran += 1;
    }
    assert_eq!(ran, 28, "BLOCK table should have exactly 28 entries");
}

#[test]
fn guard_allows_known_cases() {
    let mut ran = 0;
    for (label, tool, field, value) in ALLOW {
        assert!(
            !is_denied(tool, field, value),
            "should have been allowed: {label} -> {value}"
        );
        ran += 1;
    }
    assert_eq!(ran, 30, "ALLOW table should have exactly 30 entries");
}

#[test]
fn guard_regression_still_blocks_real_calls() {
    for (label, tool, field, value) in REGRESSION_BLOCK {
        assert!(
            is_denied(tool, field, value),
            "should have been blocked: {label} -> {value}"
        );
    }
}

#[test]
fn guard_regression_allows_text_that_merely_mentions_a_blocked_command() {
    for (label, tool, field, value) in REGRESSION_ALLOW {
        assert!(
            !is_denied(tool, field, value),
            "should have been allowed: {label} -> {value}"
        );
    }
}

#[test]
fn hook_emits_deny_json() {
    let payload = json!({"tool_name": "Bash", "tool_input": {"command": "maskrun get x"}});
    let out = hook_output(&payload);
    assert!(out.status.success());
    let decision: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(decision["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(decision["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap()
        .starts_with("maskrun guard: "));
}

#[test]
fn hook_stays_silent_when_allowed() {
    let payload = json!({"tool_name": "Bash", "tool_input": {"command": "maskrun status"}});
    let out = hook_output(&payload);
    assert!(out.status.success());
    assert_eq!(out.stdout.len(), 0, "allowed calls must produce no output");
}

#[test]
fn hook_never_blocks_on_garbage_input() {
    let out = run(&["hook"], &[], Some(b"not json at all"));
    assert!(out.status.success());
    assert_eq!(out.stdout.len(), 0);
}

#[test]
fn hook_never_blocks_on_empty_input() {
    let out = run(&["hook"], &[], Some(b""));
    assert!(out.status.success());
    assert_eq!(out.stdout.len(), 0);
}

#[test]
fn hook_works_with_no_reachable_keyring() {
    // hook (and install-guard) never call pick_backend — see main.rs's
    // dispatch — so an unreachable backend and an empty PATH must not
    // matter. This is what README promises: the guard answers even when the
    // keyring itself is gone.
    let payload = json!({"tool_name": "Bash", "tool_input": {"command": "maskrun get x"}});
    let out = run(
        &["hook"],
        &[
            ("MASKRUN_BACKEND", "secret-service"),
            ("PATH", "/nonexistent"),
        ],
        Some(payload.to_string().as_bytes()),
    );
    assert!(out.status.success());
    let decision: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(decision["hookSpecificOutput"]["permissionDecision"], "deny");

    let allowed_payload = json!({"tool_name": "Bash", "tool_input": {"command": "ls"}});
    let out = run(
        &["hook"],
        &[
            ("MASKRUN_BACKEND", "secret-service"),
            ("PATH", "/nonexistent"),
        ],
        Some(allowed_payload.to_string().as_bytes()),
    );
    assert!(out.status.success());
    assert_eq!(out.stdout.len(), 0);
}

// --------------------------------------------------------------------------
// install-guard, always against a temp file — never the user's real config.
// --------------------------------------------------------------------------

fn temp_settings_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "maskrun-install-guard-test-{}-{name}.json",
        unique_suffix()
    ))
}

fn install_guard(config: &std::path::Path, extra: &[&str]) -> Output {
    let config_str = config.to_str().unwrap();
    let mut args = vec!["install-guard", "--config", config_str];
    args.extend_from_slice(extra);
    run(&args, &[], None)
}

#[test]
fn install_guard_preserves_other_hooks_and_key_order() {
    let path = temp_settings_path("preserve");
    let original = serde_json::json!({
        "zzz_first_key": "stays first",
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "Bash",
                    "hooks": [{"type": "command", "command": "some-other-tool check"}]
                }
            ]
        },
        "aaa_last_key": "stays last"
    });
    std::fs::write(&path, serde_json::to_string_pretty(&original).unwrap()).unwrap();

    let out = install_guard(&path, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let backup = format!("{}.maskrun-backup", path.display());
    assert!(
        std::path::Path::new(&backup).exists(),
        "backup file was not created"
    );

    let updated: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let keys: Vec<&str> = updated
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec!["zzz_first_key", "hooks", "aaa_last_key"],
        "key order must be preserved"
    );

    let pre = updated["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(
        pre.len(),
        2,
        "the other hook must survive alongside the new one"
    );
    assert_eq!(pre[0]["hooks"][0]["command"], "some-other-tool check");
    assert_eq!(pre[1]["hooks"][0]["command"], "maskrun hook");

    // Idempotent: installing again must not duplicate the entry.
    let out2 = install_guard(&path, &[]);
    assert!(out2.status.success());
    let updated2: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(updated2, updated, "a second install must be a no-op");

    // --remove takes out only the maskrun entry.
    let out3 = install_guard(&path, &["--remove"]);
    assert!(
        out3.status.success(),
        "{}",
        String::from_utf8_lossy(&out3.stderr)
    );
    let removed: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let pre_after_remove = removed["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre_after_remove.len(), 1);
    assert_eq!(
        pre_after_remove[0]["hooks"][0]["command"],
        "some-other-tool check"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&backup);
}

#[test]
fn install_guard_refuses_broken_json() {
    let path = temp_settings_path("broken");
    std::fs::write(&path, "{ this is not json").unwrap();

    let out = install_guard(&path, &[]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("is not valid JSON"), "stderr was: {stderr}");
    assert!(
        stderr.contains("refusing to overwrite a broken config"),
        "stderr was: {stderr}"
    );

    // Must not have touched the file.
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "{ this is not json");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn install_guard_dry_run_writes_nothing() {
    let path = temp_settings_path("dry-run");
    // No file at all yet — dry-run must not create one.
    let out = install_guard(&path, &["--dry-run"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("dry run: nothing was written"));
    assert!(stdout.contains("maskrun hook"));
    assert!(!path.exists(), "dry-run must not create the config file");
}

#[test]
fn install_guard_remove_when_absent_is_a_no_op() {
    let path = temp_settings_path("remove-absent");
    std::fs::write(&path, "{}").unwrap();
    let out = install_guard(&path, &["--remove"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("is not installed"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn install_guard_unknown_harness_is_rejected() {
    let path = temp_settings_path("unknown-harness");
    let out = run(
        &[
            "install-guard",
            "--config",
            path.to_str().unwrap(),
            "--harness",
            "cursor",
        ],
        &[],
        None,
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown harness"), "stderr was: {stderr}");
    assert!(!path.exists());
}

#[test]
fn install_guard_command_path_override_is_used() {
    let path = temp_settings_path("command-path");
    let out = install_guard(&path, &["--command-path", "/opt/bin/maskrun"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        written["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "/opt/bin/maskrun hook"
    );
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}.maskrun-backup", path.display()));
}
