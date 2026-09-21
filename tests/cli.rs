// Port of test/test_maskrun.py's TestKeyring class, against the compiled
// binary. Secret values are randomly generated and never printed —
// assertions compare fingerprints or check absence/presence, not the value.

use std::hash::{Hash, Hasher};
use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    nanos.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    n.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn random_value() -> String {
    format!("v-{}-{}", unique_suffix(), unique_suffix())
}

// FNV-1a: not cryptographic, but this only needs to catch "changed vs.
// unchanged", so it's not worth a sha2 dev-dependency.
fn fingerprint(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn run(args: &[&str], env_extra: &[(&str, &str)], stdin_data: Option<&[u8]>) -> Output {
    run_in(None, args, env_extra, stdin_data)
}

fn run_in(
    cwd: Option<&std::path::Path>,
    args: &[&str],
    env_extra: &[(&str, &str)],
    stdin_data: Option<&[u8]>,
) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_maskrun"));
    cmd.args(args);
    // Unset: these leak in from a session like this one and change get/refuse_in_agent.
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
    ] {
        cmd.env_remove(var);
    }
    for (k, v) in env_extra {
        cmd.env(k, v);
    }
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
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

fn keyring_probe_available() -> bool {
    let probe = format!("maskrun-selftest-probe-{}", unique_suffix());
    let put = run(
        &["put", &probe, "--stdin"],
        &[],
        Some(b"probe-value-long-enough"),
    );
    if !put.status.success() {
        return false;
    }
    let got = run(&["get", &probe], &[("MASKRUN_ALLOW_READ", "1")], None);
    let _ = run(&["rm", &probe], &[], None);
    got.status.success() && String::from_utf8_lossy(&got.stdout).contains("probe-value-long-enough")
}

// Python's `--require-keyring`, as an env var: no reachable backend prints
// why and skips, unless MASKRUN_REQUIRE_KEYRING=1 makes that a hard failure.
fn require_keyring() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    let available = *AVAILABLE.get_or_init(keyring_probe_available);
    if !available {
        if std::env::var("MASKRUN_REQUIRE_KEYRING").as_deref() == Ok("1") {
            panic!("MASKRUN_REQUIRE_KEYRING=1 but no keyring backend is reachable");
        }
        eprintln!("skipping: no reachable keyring backend on this machine");
    }
    available
}

struct StoredSecret {
    name: String,
}

impl Drop for StoredSecret {
    fn drop(&mut self) {
        let _ = run(&["rm", &self.name], &[], None);
    }
}

fn store(value: &[u8]) -> StoredSecret {
    let name = format!("maskrun-test-{}", unique_suffix());
    let out = run(&["put", &name, "--stdin"], &[], Some(value));
    assert!(
        out.status.success(),
        "put failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    StoredSecret { name }
}

#[test]
fn keyring_roundtrip_preserves_awkward_values() {
    if !require_keyring() {
        return;
    }
    let cases: [(&str, String); 6] = [
        (
            "url with = in query",
            "postgres://u:p@h:5432/db?opt=a=b&x=1".to_string(),
        ),
        ("double quoted", "value \"with\" quotes".to_string()),
        ("single quoted", "value 'with' quotes".to_string()),
        ("spaces", "a value with spaces".to_string()),
        ("unicode", "parola-çöğüş-éè".to_string()),
        ("trailing equals", "abcdefgh==".to_string()),
    ];
    for (label, value) in cases {
        let secret = store(value.as_bytes());
        let got = run(&["get", &secret.name], &[("MASKRUN_ALLOW_READ", "1")], None);
        assert!(
            got.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&got.stderr)
        );
        let returned = String::from_utf8_lossy(&got.stdout);
        let returned = returned.strip_suffix('\n').unwrap_or(&returned);
        assert_eq!(
            fingerprint(returned.as_bytes()),
            fingerprint(value.as_bytes()),
            "{label} did not round-trip byte-for-byte"
        );
    }
}

#[test]
fn keyring_list_shows_name_not_value() {
    if !require_keyring() {
        return;
    }
    let value = random_value();
    let secret = store(value.as_bytes());
    let listed = run(&["list"], &[], None);
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let out = String::from_utf8_lossy(&listed.stdout);
    assert!(out.contains(&secret.name));
    assert!(!out.contains(&value));
}

#[test]
fn keyring_rm_removes() {
    if !require_keyring() {
        return;
    }
    let name = format!("maskrun-test-{}", unique_suffix());
    let put = run(
        &["put", &name, "--stdin"],
        &[],
        Some(random_value().as_bytes()),
    );
    assert!(put.status.success());
    let rm = run(&["rm", &name], &[], None);
    assert!(rm.status.success());
    let gone = run(&["get", &name], &[("MASKRUN_ALLOW_READ", "1")], None);
    assert!(!gone.status.success());
}

#[test]
fn keyring_get_missing_fails() {
    if !require_keyring() {
        return;
    }
    let name = format!("maskrun-definitely-absent-{}", unique_suffix());
    let result = run(&["get", &name], &[("MASKRUN_ALLOW_READ", "1")], None);
    assert!(!result.status.success());
}

#[test]
fn invalid_name_rejected() {
    for bad in ["has space", "has/slash", "-leading-dash", ""] {
        let result = run(&["get", bad], &[("MASKRUN_ALLOW_READ", "1")], None);
        assert!(
            !result.status.success(),
            "should have been rejected: {bad:?}"
        );
    }
}

#[test]
fn status_reports_ok_and_missing() {
    if !require_keyring() {
        return;
    }
    let value = random_value();
    let secret = store(value.as_bytes());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".maskrun"),
        format!("TEST={}\n", secret.name),
    )
    .unwrap();

    let ok = run_in(Some(dir.path()), &["status"], &[], None);
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert!(String::from_utf8_lossy(&ok.stdout).contains("ok"));

    std::fs::write(
        dir.path().join(".maskrun"),
        format!(
            "TEST={}\nOTHER=maskrun-absent-{}\n",
            secret.name,
            unique_suffix()
        ),
    )
    .unwrap();
    let missing = run_in(Some(dir.path()), &["status"], &[], None);
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stdout).contains("MISSING"));
}

#[test]
fn version_and_help_exit_cleanly() {
    let version = run(&["--version"], &[], None);
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).contains("maskrun"));

    let help = run(&["--help"], &[], None);
    assert!(help.status.success());
}

#[test]
fn double_dash_argv_split_reaches_real_command() {
    // If the split before `--` were wrong, "hi" would end up in the
    // assignment list and fail with "expected VAR=secret-name" before
    // collect() ever runs. Failing on "half-filled" instead proves the
    // assignment was exactly `FOO=<absent>` and the command was `echo hi`.
    let absent = format!("FOO=maskrun-absent-{}", unique_suffix());
    let result = run(&["exec", &absent, "--", "echo", "hi"], &[], None);
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    // These two hold whether or not a keyring is reachable: both are parse
    // errors that would fire before any backend is touched.
    assert!(!stderr.contains("expected VAR=secret-name"), "{stderr}");
    assert!(!stderr.contains("nothing to run"), "{stderr}");
    if !require_keyring() {
        return;
    }
    assert!(stderr.contains("half-filled"), "{stderr}");

    // Only the first -- splits; a second -- stays part of the command, so
    // `echo` sees it as a literal argument and prints it back.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".maskrun"), "").unwrap();
    let result = run_in(
        Some(dir.path()),
        &["run", "--", "echo", "hi", "--", "--flag"],
        &[],
        None,
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout).trim_end(),
        "hi -- --flag"
    );
}

#[test]
fn put_with_platform_and_note_groups_under_that_platform_in_list() {
    if !require_keyring() {
        return;
    }
    let name = format!("maskrun-test-{}", unique_suffix());
    let platform = format!("plat-{}", unique_suffix());
    let put = run(
        &[
            "put",
            &name,
            "--stdin",
            "--for",
            &platform,
            "--note",
            "fine-grained, repo+plan",
        ],
        &[],
        Some(random_value().as_bytes()),
    );
    assert!(
        put.status.success(),
        "{}",
        String::from_utf8_lossy(&put.stderr)
    );
    let _cleanup = StoredSecret { name: name.clone() };

    let listed = run(&["list"], &[], None);
    let out = String::from_utf8_lossy(&listed.stdout);
    let plat_line = out.find(&platform).expect("platform header missing");
    let name_line = out.find(&name).expect("secret name missing");
    assert!(
        plat_line < name_line,
        "platform header should precede the secret: {out}"
    );
    assert!(out.contains("fine-grained, repo+plan"), "{out}");
}

#[test]
fn put_without_for_leaves_secret_under_no_platform() {
    if !require_keyring() {
        return;
    }
    let secret = store(random_value().as_bytes());
    let listed = run(&["list"], &[], None);
    let out = String::from_utf8_lossy(&listed.stdout);
    let no_platform_line = out
        .find("(no platform)")
        .expect("(no platform) header missing");
    let name_line = out.find(&secret.name).expect("secret name missing");
    assert!(no_platform_line < name_line, "{out}");
}

#[test]
fn label_tags_an_existing_secret_after_the_fact() {
    if !require_keyring() {
        return;
    }
    let secret = store(random_value().as_bytes());
    let platform = format!("plat-{}", unique_suffix());
    let label = run(&["label", &secret.name, "--for", &platform], &[], None);
    assert!(
        label.status.success(),
        "{}",
        String::from_utf8_lossy(&label.stderr)
    );

    let listed = run(&["list"], &[], None);
    let out = String::from_utf8_lossy(&listed.stdout);
    let plat_line = out.find(&platform).expect("platform header missing");
    let name_line = out.find(&secret.name).expect("secret name missing");
    assert!(plat_line < name_line, "{out}");
}

#[test]
fn label_for_empty_string_clears_the_platform() {
    if !require_keyring() {
        return;
    }
    let secret = store(random_value().as_bytes());
    let platform = format!("plat-{}", unique_suffix());
    let label = run(&["label", &secret.name, "--for", &platform], &[], None);
    assert!(label.status.success());

    let clear = run(&["label", &secret.name, "--for", ""], &[], None);
    assert!(
        clear.status.success(),
        "{}",
        String::from_utf8_lossy(&clear.stderr)
    );

    let listed = run(&["list"], &[], None);
    let out = String::from_utf8_lossy(&listed.stdout);
    let no_platform_line = out
        .find("(no platform)")
        .expect("(no platform) header missing");
    let name_line = out.find(&secret.name).expect("secret name missing");
    assert!(no_platform_line < name_line, "{out}");
    assert!(
        !out.contains(&platform),
        "cleared platform still shown: {out}"
    );
}

#[test]
fn put_without_for_flag_preserves_the_existing_label() {
    if !require_keyring() {
        return;
    }
    let name = format!("maskrun-test-{}", unique_suffix());
    let platform = format!("plat-{}", unique_suffix());
    let first = run(
        &["put", &name, "--stdin", "--for", &platform],
        &[],
        Some(random_value().as_bytes()),
    );
    assert!(first.status.success());
    let _cleanup = StoredSecret { name: name.clone() };

    // Overwrite the value with no --for at all: the old label must survive.
    let second = run(
        &["put", &name, "--stdin"],
        &[],
        Some(random_value().as_bytes()),
    );
    assert!(second.status.success());

    let listed = run(&["list"], &[], None);
    let out = String::from_utf8_lossy(&listed.stdout);
    let plat_line = out.find(&platform).expect("label was lost on overwrite");
    let name_line = out.find(&name).expect("secret name missing");
    assert!(plat_line < name_line, "{out}");
}

#[test]
fn list_plain_stays_flat_sorted_and_unlabelled() {
    if !require_keyring() {
        return;
    }
    let name = format!("maskrun-test-{}", unique_suffix());
    let put = run(
        &[
            "put",
            &name,
            "--stdin",
            "--for",
            "some-platform",
            "--note",
            "some note",
        ],
        &[],
        Some(random_value().as_bytes()),
    );
    assert!(put.status.success());
    let _cleanup = StoredSecret { name: name.clone() };

    let listed = run(&["list", "--plain"], &[], None);
    assert!(listed.status.success());
    let out = String::from_utf8_lossy(&listed.stdout);
    assert!(out.lines().any(|l| l == name), "{out}");
    assert!(!out.contains("some-platform"), "{out}");
    assert!(!out.contains("some note"), "{out}");
    assert!(!out.contains("(no platform)"), "{out}");
}

#[test]
fn list_groups_platforms_alphabetically_with_no_platform_last() {
    if !require_keyring() {
        return;
    }
    let a = format!("plat-a-{}", unique_suffix());
    let z = format!("plat-z-{}", unique_suffix());
    let s1 = store(random_value().as_bytes());
    let s2 = store(random_value().as_bytes());
    let _s3 = store(random_value().as_bytes()); // stays unlabelled
    assert!(run(&["label", &s1.name, "--for", &z], &[], None)
        .status
        .success());
    assert!(run(&["label", &s2.name, "--for", &a], &[], None)
        .status
        .success());

    let listed = run(&["list"], &[], None);
    let out = String::from_utf8_lossy(&listed.stdout);
    let pos_a = out.find(&a).unwrap();
    let pos_z = out.find(&z).unwrap();
    let pos_none = out.find("(no platform)").unwrap();
    assert!(pos_a < pos_z, "{out}");
    assert!(pos_z < pos_none, "{out}");
}

#[test]
fn label_with_no_flags_and_no_tty_is_a_clear_error_not_a_hang() {
    if !require_keyring() {
        return;
    }
    let secret = store(random_value().as_bytes());
    // The test harness's child stdin/stdout are pipes, never a tty, so this
    // must fail immediately instead of trying to prompt.
    let result = run(&["label", &secret.name], &[], None);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("nothing to do"));
}

#[test]
fn put_with_no_name_and_no_tty_fails_fast_instead_of_prompting() {
    // No keyring needed: this must fail before ever touching the backend.
    let result = run(&["put"], &[], None);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("secret name required"));
}

#[test]
fn label_with_no_name_and_no_tty_fails_fast_instead_of_prompting() {
    let result = run(&["label"], &[], None);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("secret name required"));
}

// The three tests below force `pick_backend()` itself to fail (`dpapi` is
// Windows-only) — reproducing the CI "no keyring" job's failure mode, which
// a locally-reachable keyring can't: the ordering bug they guard against
// only shows up when the backend construction that used to happen before
// this argument check is the thing that fails. Without the fix, all three
// report "could not reach"/"only available on Windows" instead of the
// actionable "secret name required".
#[test]
fn put_with_no_name_fails_on_usage_even_when_the_backend_is_unreachable() {
    let result = run(&["put"], &[("MASKRUN_BACKEND", "dpapi")], None);
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("secret name required"), "stderr: {stderr}");
}

#[test]
fn label_with_no_name_fails_on_usage_even_when_the_backend_is_unreachable() {
    let result = run(&["label"], &[("MASKRUN_BACKEND", "dpapi")], None);
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("secret name required"), "stderr: {stderr}");
}

#[test]
fn rm_with_no_name_fails_on_usage_even_when_the_backend_is_unreachable() {
    let result = run(&["rm"], &[("MASKRUN_BACKEND", "dpapi")], None);
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("secret name required"), "stderr: {stderr}");
}

#[test]
fn note_over_200_chars_is_rejected() {
    if !require_keyring() {
        return;
    }
    let name = format!("maskrun-test-{}", unique_suffix());
    let long_note = "a".repeat(201);
    let result = run(
        &["put", &name, "--stdin", "--note", &long_note],
        &[],
        Some(random_value().as_bytes()),
    );
    assert!(!result.status.success());
    let _ = run(&["rm", &name], &[], None);
}

#[test]
fn note_with_a_newline_is_rejected() {
    if !require_keyring() {
        return;
    }
    let name = format!("maskrun-test-{}", unique_suffix());
    let result = run(
        &["put", &name, "--stdin", "--note", "line one\nline two"],
        &[],
        Some(random_value().as_bytes()),
    );
    assert!(!result.status.success());
    let _ = run(&["rm", &name], &[], None);
}
