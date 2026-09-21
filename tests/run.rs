// Integration tests for `maskrun run` / `exec` / `import`, run against the
// compiled binary. Mirrors test/test_maskrun.py's TestMasking, TestManifest
// and TestImport classes. Child processes are python3 (already required by
// test/test_maskrun.py itself; not a maskrun runtime dependency) so masking
// behaviour can be pinned down exactly, including a real split-write case.
//
// As in tests/cli.rs, secret values are randomly generated and compared by
// fingerprint or absence/presence, never printed or asserted by equality
// against a logged value.

use std::hash::{Hash, Hasher};
use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;

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
    // These tests assert agent-session behaviour (masking on by default, get
    // and import refused); individual tests override this via env_extra.
    cmd.env("MASKRUN_AGENT", "1");
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
    let put = run(&["put", &probe, "--stdin"], &[], Some(b"probe-value-long-enough"));
    if !put.status.success() {
        return false;
    }
    let got = run(&["get", &probe], &[("MASKRUN_ALLOW_READ", "1")], None);
    let _ = run(&["rm", &probe], &[], None);
    got.status.success() && String::from_utf8_lossy(&got.stdout).contains("probe-value-long-enough")
}

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

fn store(value: &str) -> StoredSecret {
    let name = format!("maskrun-test-{}", unique_suffix());
    let out = run(&["put", &name, "--stdin"], &[], Some(value.as_bytes()));
    assert!(out.status.success(), "put failed: {}", String::from_utf8_lossy(&out.stderr));
    StoredSecret { name }
}

fn exec_output(
    secret: &str,
    code: &str,
    extra_args: &[&str],
    env_extra: &[(&str, &str)],
    stdin_data: Option<&[u8]>,
) -> Output {
    let assignment = format!("TEST={secret}");
    let mut args: Vec<&str> = vec!["exec"];
    args.extend_from_slice(extra_args);
    args.push(&assignment);
    args.extend_from_slice(&["--", "python3", "-c", code]);
    run(&args, env_extra, stdin_data)
}

// --------------------------------------------------------------------------
// Output masking (TestMasking)
// --------------------------------------------------------------------------

#[test]
fn stdout_masked() {
    if !require_keyring() {
        return;
    }
    let value = random_value();
    let secret = store(&value);
    let out = exec_output(
        &secret.name,
        "import os;print('leak: ' + os.environ['TEST'])",
        &[],
        &[],
        None,
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains(&value));
    assert!(text.contains("<masked:TEST>"));
}

#[test]
fn stderr_masked() {
    if !require_keyring() {
        return;
    }
    let value = random_value();
    let secret = store(&value);
    let out = exec_output(
        &secret.name,
        "import os,sys;sys.stderr.write('err: ' + os.environ['TEST'] + '\\n')",
        &[],
        &[],
        None,
    );
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(!text.contains(&value));
    assert!(text.contains("<masked:TEST>"));
}

#[test]
fn raw_flag_disables_masking() {
    if !require_keyring() {
        return;
    }
    let value = random_value();
    let secret = store(&value);
    let out = exec_output(
        &secret.name,
        "import os;print(os.environ['TEST'])",
        &["--raw"],
        &[],
        None,
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains(&value));
}

#[test]
fn mask_env_var_disables_masking() {
    if !require_keyring() {
        return;
    }
    let value = random_value();
    let secret = store(&value);
    let out = exec_output(
        &secret.name,
        "import os;print(os.environ['TEST'])",
        &[],
        &[("MASKRUN_MASK", "0")],
        None,
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains(&value));
}

#[test]
fn base64_variant_masked() {
    if !require_keyring() {
        return;
    }
    let value = random_value();
    let secret = store(&value);
    let out = exec_output(
        &secret.name,
        "import os,base64;print(base64.b64encode(os.environ['TEST'].encode()).decode())",
        &[],
        &[],
        None,
    );
    let encoded = BASE64.encode(value.as_bytes());
    assert!(!String::from_utf8_lossy(&out.stdout).contains(&encoded));
}

#[test]
fn urlencoded_variant_masked() {
    if !require_keyring() {
        return;
    }
    let value = random_value();
    let secret = store(&value);
    let out = exec_output(
        &secret.name,
        "import os,urllib.parse;print(urllib.parse.quote(os.environ['TEST'], safe=''))",
        &[],
        &[],
        None,
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains(&value));
}

#[test]
fn output_without_newline_masked() {
    if !require_keyring() {
        return;
    }
    let value = random_value();
    let secret = store(&value);
    let out = exec_output(
        &secret.name,
        "import os,sys;sys.stdout.write('prompt ' + os.environ['TEST'])",
        &[],
        &[],
        None,
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains(&value));
}

#[test]
fn value_split_across_writes_masked() {
    if !require_keyring() {
        return;
    }
    // The regression that matters: a secret straddling two flushes, with a
    // real delay between them so the two writes reach the pipe separately.
    let value = random_value();
    let secret = store(&value);
    let code = "import os,sys,time;\
                v=os.environ['TEST'];\
                sys.stdout.write('head ' + v[:20]);sys.stdout.flush();\
                time.sleep(0.3);\
                sys.stdout.write(v[20:] + ' tail\\n')";
    let out = exec_output(&secret.name, code, &[], &[], None);
    assert!(!String::from_utf8_lossy(&out.stdout).contains(&value));
}

#[test]
fn exit_code_preserved() {
    if !require_keyring() {
        return;
    }
    let secret = store(&random_value());
    let out = exec_output(&secret.name, "raise SystemExit(42)", &[], &[], None);
    assert_eq!(out.status.code(), Some(42));
}

#[test]
fn stdin_reaches_child() {
    if !require_keyring() {
        return;
    }
    let secret = store(&random_value());
    let out = exec_output(
        &secret.name,
        "import sys;sys.stdout.write(sys.stdin.read())",
        &[],
        &[],
        Some(b"hello-from-stdin"),
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("hello-from-stdin"));
}

#[test]
fn short_value_warns_and_is_not_masked() {
    if !require_keyring() {
        return;
    }
    let short = store("abc");
    let out = exec_output(
        &short.name,
        "import os;print(os.environ['TEST'])",
        &[],
        &[],
        None,
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("left unmasked"));
}

#[test]
fn missing_secret_refuses_to_run() {
    if !require_keyring() {
        return;
    }
    let out = run(
        &["exec", &format!("GONE=maskrun-absent-{}", unique_suffix()), "--", "python3", "-c", "print(1)"],
        &[],
        None,
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("half-filled"));
}

#[test]
fn get_refused_in_agent_session() {
    if !require_keyring() {
        return;
    }
    let value = random_value();
    let secret = store(&value);
    let out = run(&["get", &secret.name], &[], None);
    assert!(!out.status.success());
    assert!(!String::from_utf8_lossy(&out.stdout).contains(&value));
    assert!(String::from_utf8_lossy(&out.stderr).contains("disabled inside an AI agent session"));
}

// --------------------------------------------------------------------------
// Manifest-driven `run` (TestManifest)
// --------------------------------------------------------------------------

fn manifest_dir(secret_name: &str) -> tempfile::TempDir {
    let dir = tempfile::Builder::new().prefix("maskrun-run-test-").tempdir().unwrap();
    std::fs::write(dir.path().join(".maskrun"), format!("TEST={secret_name}\n")).unwrap();
    dir
}

#[test]
fn run_injects_and_masks() {
    if !require_keyring() {
        return;
    }
    let value = random_value();
    let secret = store(&value);
    let dir = manifest_dir(&secret.name);
    let out = run_in(
        Some(dir.path()),
        &["run", "--", "python3", "-c", "import os;print('run: ' + os.environ['TEST'])"],
        &[],
        None,
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains(&value));
    assert!(text.contains("<masked:TEST>"));
}

#[test]
fn bare_double_dash_is_run() {
    if !require_keyring() {
        return;
    }
    let value = random_value();
    let secret = store(&value);
    let dir = manifest_dir(&secret.name);
    let out = run_in(
        Some(dir.path()),
        &["--", "python3", "-c", "import os;print('bare: ' + os.environ['TEST'])"],
        &[],
        None,
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("<masked:TEST>"));
}

#[test]
fn tail_hold_catches_split_with_no_newline() {
    if !require_keyring() {
        return;
    }
    // No trailing newline is ever printed, so masking relies on TAIL_HOLD
    // rather than the line-boundary cut.
    let value = random_value();
    let secret = store(&value);
    let code = "import os,sys,time;\
                v=os.environ['TEST'];\
                sys.stdout.write(v[:15]);sys.stdout.flush();\
                time.sleep(0.3);\
                sys.stdout.write(v[15:])";
    let out = exec_output(&secret.name, code, &[], &[], None);
    assert!(!String::from_utf8_lossy(&out.stdout).contains(&value));
    assert!(String::from_utf8_lossy(&out.stdout).contains("<masked:TEST>"));
}

// --------------------------------------------------------------------------
// Import (TestImport)
// --------------------------------------------------------------------------

struct ImportFixture {
    dir: tempfile::TempDir,
    values: Vec<(&'static str, String)>,
    _cleanup: Vec<StoredSecret>,
}

fn import_fixture() -> ImportFixture {
    let dir = tempfile::Builder::new().prefix("maskrun-import-test-").tempdir().unwrap();
    let values: Vec<(&'static str, String)> = vec![
        ("DATABASE_URL", "postgres://u:p@h:5432/db?opt=a=b".to_string()),
        ("JWT_SECRET", BASE64.encode(b"jwt-signing-material")),
        ("QUOTED", "value with spaces".to_string()),
        ("VITE_PUBLIC_URL", "https://example.com".to_string()),
    ];
    let content = format!(
        "DATABASE_URL={}\nJWT_SECRET=\"{}\"\nQUOTED='{}'\n# a comment\n\nVITE_PUBLIC_URL={}\n",
        values[0].1, values[1].1, values[2].1, values[3].1
    );
    std::fs::write(dir.path().join(".env"), content).unwrap();
    // Cleaned up regardless of whether the import in a given test actually
    // ran, same as test_maskrun.py's TestImport.tearDown.
    let cleanup = values
        .iter()
        .map(|(var, _)| StoredSecret { name: format!("imp-{}", var.to_lowercase().replace('_', "-")) })
        .collect();
    ImportFixture { dir, values, _cleanup: cleanup }
}

fn import_cmd(fixture: &ImportFixture, extra: &[&str]) -> Output {
    let mut args: Vec<&str> = vec!["import", ".env", "--prefix", "imp"];
    args.extend_from_slice(extra);
    run_in(Some(fixture.dir.path()), &args, &[("MASKRUN_AGENT", "0")], None)
}

#[test]
fn import_refused_in_agent_session() {
    if !require_keyring() {
        return;
    }
    let fixture = import_fixture();
    let out = run_in(Some(fixture.dir.path()), &["import", ".env"], &[], None);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("disabled inside an AI agent session"));
}

#[test]
fn import_dry_run_writes_nothing() {
    if !require_keyring() {
        return;
    }
    let fixture = import_fixture();
    let out = import_cmd(&fixture, &["--dry-run"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!fixture.dir.path().join(".maskrun").exists());
    assert!(String::from_utf8_lossy(&out.stdout).contains("nothing was written"));
}

#[test]
fn import_preserves_values_and_skips_public() {
    if !require_keyring() {
        return;
    }
    let fixture = import_fixture();
    let out = import_cmd(&fixture, &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let manifest = std::fs::read_to_string(fixture.dir.path().join(".maskrun")).unwrap();
    assert!(manifest.contains("DATABASE_URL=imp-database-url"));
    assert!(!manifest.contains("VITE_PUBLIC_URL")); // client-bundled, skipped
    for (_, value) in &fixture.values {
        assert!(!manifest.contains(value.as_str())); // manifest holds no values
    }

    for (var, value) in &fixture.values {
        if *var == "VITE_PUBLIC_URL" {
            continue;
        }
        let name = format!("imp-{}", var.to_lowercase().replace('_', "-"));
        let got = run(&["get", &name], &[("MASKRUN_ALLOW_READ", "1")], None);
        assert!(got.status.success(), "{}", String::from_utf8_lossy(&got.stderr));
        let returned = String::from_utf8_lossy(&got.stdout);
        let returned = returned.strip_suffix('\n').unwrap_or(&returned);
        assert_eq!(
            fingerprint(returned.as_bytes()),
            fingerprint(value.as_bytes()),
            "{var} did not survive import byte-for-byte"
        );
    }
}
