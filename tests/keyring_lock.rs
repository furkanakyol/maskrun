// Opt-in only: `gnome-keyring-daemon --start` re-execs itself and the child
// discovers the ambient session's control directory regardless of the
// --control-directory and env we hand the parent. On a real desktop that
// child prompts the user to create a new default keyring, repeatedly, and
// survives the test that spawned it. Stripping DISPLAY does not stop it.
// So these run only where someone asks for them (CI), never from a bare
// `cargo test`.
// Proves the locked-vault behaviour in keyring.rs/run.rs against a real
// Secret Service — but never the developer's own login collection. Each
// test spins up its own D-Bus daemon and gnome-keyring-daemon under a
// throwaway HOME/XDG_RUNTIME_DIR/control-directory, so "lock the collection"
// here can never be "lock the collection this account actually uses" (the
// exact incident this file exists to prevent a repeat of). Skips instead of
// failing when dbus-daemon/gnome-keyring-daemon/secret-tool aren't on PATH.
#![cfg(target_os = "linux")]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn have(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:x}-{:x}-{n:x}", std::process::id(), nanos)
}

struct IsolatedSession {
    _work: tempfile::TempDir,
    runtime_dir: PathBuf,
    home_dir: PathBuf,
    control_dir: PathBuf,
    bus_address: String,
    dbus_pid: String,
}

impl IsolatedSession {
    fn start() -> Option<Self> {
        if !have("dbus-daemon") || !have("gnome-keyring-daemon") || !have("secret-tool") || !have("timeout") {
            eprintln!("skipping: dbus-daemon/gnome-keyring-daemon/secret-tool/timeout not on PATH");
            return None;
        }

        let work = tempfile::tempdir().ok()?;
        let runtime_dir = work.path().join("runtime");
        let home_dir = work.path().join("home");
        let control_dir = runtime_dir.join("keyring");
        std::fs::create_dir_all(&control_dir).ok()?;
        std::fs::create_dir_all(&home_dir).ok()?;
        #[cfg(unix)]
        for dir in [&runtime_dir, &control_dir] {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }

        // --fork so this call returns once the bus is up, handing back its
        // address and pid instead of a session we'd have to babysit.
        let out = Command::new("timeout")
            .args(["5", "dbus-daemon", "--session", "--fork", "--print-address", "--print-pid"])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines();
        let bus_address = lines.next()?.trim().to_string();
        let dbus_pid = lines.next()?.trim().to_string();

        let session = Self { _work: work, runtime_dir, home_dir, control_dir, bus_address, dbus_pid };

        // --control-directory is what keeps this off the real
        // $XDG_RUNTIME_DIR/keyring: gnome-keyring-daemon otherwise discovers
        // and reuses whatever control directory the ambient session already
        // has, regardless of DBUS_SESSION_BUS_ADDRESS.
        let mut daemon = Command::new("gnome-keyring-daemon")
            .args(["--start", "--components=secrets"])
            .arg(format!("--control-directory={}", session.control_dir.display()))
            .envs(session.env())
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        // Empty passphrase: creates a fresh, initially-unlocked "login"
        // collection under our control-directory, same as ci.yml's warmup.
        // Not waited on: --start double-forks and daemonizes, and in one
        // run waiting on the immediate child here hung indefinitely (it
        // ran fine standalone under a plain shell, so this is specific to
        // spawning it from a multi-threaded parent) — the warmup retry loop
        // below already accounts for the daemon needing a moment to be up.
        let _ = daemon.stdin.take().unwrap().write_all(b"\n");

        // The daemon takes a moment to register org.freedesktop.secrets on
        // the new bus; retry rather than guess a fixed sleep (ci.yml uses
        // the same loop against the real job for the same reason). Each
        // attempt is itself `timeout`-bounded: an unready D-Bus service can
        // make secret-tool block for its full method-call timeout (tens of
        // seconds) rather than fail fast, which turned 30 quick retries into
        // a multi-minute hang the first time this ran without it.
        for _ in 0..30 {
            let stored = secret_tool(&["store", "--label=warmup", "service", "maskrun-lock-test", "name", "warmup"], &session.env(), Some(b"warmup"))
                .map(|s| s.success())
                .unwrap_or(false);
            if stored {
                let _ = secret_tool(&["clear", "service", "maskrun-lock-test", "name", "warmup"], &session.env(), None);
                return Some(session);
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        None
    }

    // DISPLAY/WAYLAND_DISPLAY are stripped at every spawn site below, not
    // here: `envs()` adds to the inherited environment rather than replacing
    // it, so without that the throwaway daemon inherits the real session's
    // display and pops "create a new keyring" dialogs on the user's actual
    // screen. It did.
    fn env(&self) -> [(&'static str, String); 3] {
        [
            ("DBUS_SESSION_BUS_ADDRESS", self.bus_address.clone()),
            ("XDG_RUNTIME_DIR", self.runtime_dir.display().to_string()),
            ("HOME", self.home_dir.display().to_string()),
        ]
    }

    fn lock(&self) {
        let status = secret_tool(&["lock"], &self.env(), None);
        assert!(status.map(|s| s.success()).unwrap_or(false), "secret-tool lock failed");
    }
}

// Every secret-tool call goes through `timeout`: against an unready or
// wedged D-Bus service it can block for its own multi-second method-call
// timeout instead of failing fast, and the warmup loop above depends on
// failures being fast to stay bounded.
fn secret_tool(args: &[&str], env: &[(&'static str, String)], stdin_data: Option<&[u8]>) -> Option<std::process::ExitStatus> {
    let mut cmd = Command::new("timeout");
    cmd.arg("5").arg("secret-tool").args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.env_remove("DISPLAY");
    cmd.env_remove("WAYLAND_DISPLAY");
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    let mut child = cmd.spawn().ok()?;
    if let Some(data) = stdin_data {
        child.stdin.take().unwrap().write_all(data).ok()?;
    } else {
        drop(child.stdin.take());
    }
    child.wait().ok()
}

impl Drop for IsolatedSession {
    fn drop(&mut self) {
        // Targeted kill by pid, never pkill: the grep below matches this
        // session's own control-directory path, which is unique per test
        // run, so there is no way for this to reach the real
        // gnome-keyring-daemon (a different control-directory entirely).
        if let Ok(out) = Command::new("ps").args(["-eo", "pid,args"]).output() {
            let marker = self.control_dir.display().to_string();
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if line.contains("gnome-keyring-daemon") && line.contains(&marker) {
                    if let Some(pid) = line.split_whitespace().next() {
                        let _ = Command::new("kill").arg(pid).status();
                    }
                }
            }
        }
        let _ = Command::new("kill").arg(&self.dbus_pid).status();
    }
}

// Agent-detection variables tests/cli.rs already strips for the same reason:
// this test process itself runs inside an agent session, and maskrun's own
// guard refuses `get`/`put` there by design — unrelated to the lock behaviour
// under test.
const AGENT_ENV_VARS: &[&str] = &[
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
];

struct RunOpts<'a> {
    cwd: Option<&'a Path>,
    stdin: Option<&'a [u8]>,
    no_unlock: bool,
    strip_display: bool,
}

impl Default for RunOpts<'_> {
    fn default() -> Self {
        RunOpts { cwd: None, stdin: None, no_unlock: true, strip_display: false }
    }
}

// Polls try_wait() instead of a plain blocking wait(): a run that hangs
// waiting on an unlock prompt nobody will ever answer is exactly the bug
// these tests exist to catch, so the test itself must not hang alongside it.
fn run_maskrun(session: &IsolatedSession, args: &[&str], opts: RunOpts) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_maskrun"));
    cmd.args(args);
    cmd.env("MASKRUN_BACKEND", "secret-service");
    for (k, v) in session.env() {
        cmd.env(k, v);
    }
    for var in AGENT_ENV_VARS {
        cmd.env_remove(var);
    }
    if opts.no_unlock {
        cmd.env("MASKRUN_NO_UNLOCK", "1");
    } else {
        cmd.env_remove("MASKRUN_NO_UNLOCK");
    }
    if opts.strip_display {
        cmd.env_remove("DISPLAY");
        cmd.env_remove("WAYLAND_DISPLAY");
    }
    if let Some(dir) = opts.cwd {
        cmd.current_dir(dir);
    }
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("failed to spawn maskrun binary");
    if let Some(data) = opts.stdin {
        child.stdin.take().unwrap().write_all(data).unwrap();
    } else {
        drop(child.stdin.take());
    }

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().expect("try_wait failed") {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let _ = child.stdout.take().unwrap().read_to_end(&mut stdout);
            let _ = child.stderr.take().unwrap().read_to_end(&mut stderr);
            return Output { status, stdout, stderr };
        }
        if Instant::now() >= deadline {
            let _ = Command::new("kill").args(["-9", &child.id().to_string()]).status();
            panic!(
                "maskrun did not exit within 15s — an unlock attempt that never returns is \
                 exactly the regression this test guards against"
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

fn put_secret(session: &IsolatedSession, name: &str, value: &[u8]) {
    let out = run_maskrun(
        session,
        &["put", name, "--stdin"],
        RunOpts { stdin: Some(value), no_unlock: false, ..Default::default() },
    );
    assert!(out.status.success(), "setup: put failed: {}", text(&out));
}

// (a) + (b) from the incident: a locked collection must not look like an
// empty one, and the error must never point the user at `maskrun put` —
// doing so on secrets that are actually just behind the lock is how they'd
// get overwritten.
#[test]
#[ignore = "spawns a gnome-keyring-daemon; opt in with MASKRUN_LOCK_TESTS=1"]
fn locked_collection_is_reported_as_locked_not_empty() {
    if std::env::var_os("MASKRUN_LOCK_TESTS").is_none() {
        eprintln!("skipping: set MASKRUN_LOCK_TESTS=1 to run (spawns a keyring daemon)");
        return;
    }

    let Some(session) = IsolatedSession::start() else { return };
    let name = format!("maskrun-lock-test-{}", unique_suffix());
    put_secret(&session, &name, b"locked-vault-value");

    let listed = run_maskrun(&session, &["list"], RunOpts::default());
    assert!(text(&listed).contains(&name), "sanity: unlocked list should show the secret");

    session.lock();

    let manifest_dir = tempfile::tempdir().unwrap();
    std::fs::write(manifest_dir.path().join(".maskrun"), format!("TEST_VAR={name}\n")).unwrap();

    let list = run_maskrun(&session, &["list"], RunOpts::default());
    let list_text = text(&list);
    assert!(!list.status.success());
    assert!(list_text.contains("locked"), "{list_text}");
    assert!(!list_text.contains("no secrets stored"), "{list_text}");

    let status = run_maskrun(
        &session,
        &["status"],
        RunOpts { cwd: Some(manifest_dir.path()), ..Default::default() },
    );
    let status_text = text(&status);
    assert!(!status.status.success());
    assert!(status_text.contains("locked"), "{status_text}");
    assert!(!status_text.contains("MISSING"), "{status_text}");
    assert!(!status_text.contains("Add them with"), "{status_text}");

    let run = run_maskrun(
        &session,
        &["run", "--raw", "--", "true"],
        RunOpts { cwd: Some(manifest_dir.path()), ..Default::default() },
    );
    let run_text = text(&run);
    assert!(!run.status.success());
    assert!(run_text.contains("locked"), "{run_text}");
    assert!(!run_text.contains("half-filled"), "{run_text}");
    assert!(!run_text.contains("Add them with"), "{run_text}");

    let get = run_maskrun(&session, &["get", &name], RunOpts::default());
    let get_text = text(&get);
    assert!(!get.status.success());
    assert!(get_text.contains("locked"), "{get_text}");
    assert!(!get_text.contains("no such secret"), "{get_text}");
}

// (c): put() on a locked collection used to fail with dbus-secret-service's
// raw "Cannot create an item in a locked collection" DBus error instead of
// this crate's own message.
#[test]
#[ignore = "spawns a gnome-keyring-daemon; opt in with MASKRUN_LOCK_TESTS=1"]
fn put_on_a_locked_collection_gives_the_locked_message_not_a_raw_dbus_error() {
    if std::env::var_os("MASKRUN_LOCK_TESTS").is_none() {
        eprintln!("skipping: set MASKRUN_LOCK_TESTS=1 to run (spawns a keyring daemon)");
        return;
    }

    let Some(session) = IsolatedSession::start() else { return };
    let name = format!("maskrun-lock-test-{}", unique_suffix());
    put_secret(&session, &name, b"pre-existing");
    session.lock();

    let out = run_maskrun(&session, &["put", &name, "--stdin"], RunOpts { stdin: Some(b"new-value"), ..Default::default() });
    let msg = text(&out);
    assert!(!out.status.success());
    assert!(msg.contains("locked"), "{msg}");
    assert!(!msg.contains("Cannot create an item"), "{msg}");
}

// Requirement 2 from the coordinator review: without MASKRUN_NO_UNLOCK, a
// process with no DISPLAY/WAYLAND_DISPLAY must still fail fast instead of
// blocking on an unlock prompt nothing can render — this is the headless
// auto-detection path, not the explicit opt-out tested above (which every
// other test in this file uses to stay deterministic regardless of this
// machine's own display).
#[test]
#[ignore = "spawns a gnome-keyring-daemon; opt in with MASKRUN_LOCK_TESTS=1"]
fn locked_collection_without_a_display_fails_fast_without_the_opt_out() {
    if std::env::var_os("MASKRUN_LOCK_TESTS").is_none() {
        eprintln!("skipping: set MASKRUN_LOCK_TESTS=1 to run (spawns a keyring daemon)");
        return;
    }

    let Some(session) = IsolatedSession::start() else { return };
    let name = format!("maskrun-lock-test-{}", unique_suffix());
    put_secret(&session, &name, b"headless-value");
    session.lock();

    let start = Instant::now();
    let out = run_maskrun(
        &session,
        &["list"],
        RunOpts { no_unlock: false, strip_display: true, ..Default::default() },
    );
    let elapsed = start.elapsed();
    let msg = text(&out);
    assert!(!out.status.success());
    assert!(msg.contains("locked"), "{msg}");
    assert!(!msg.contains("no secrets stored"), "{msg}");
    // Generous margin over the immediate cancel this should actually take;
    // the real regression this guards against is the ~1 year default the
    // underlying crate documents for connect() without a bounded timeout.
    assert!(elapsed < Duration::from_secs(5), "took {elapsed:?} — should fail immediately");
}

// A locked keyring reported by is_locked() elsewhere must not make an
// already-unlocked, already-successful collection look broken.
#[test]
#[ignore = "spawns a gnome-keyring-daemon; opt in with MASKRUN_LOCK_TESTS=1"]
fn unlocked_collection_behaves_normally_after_the_lock_handling_changes() {
    if std::env::var_os("MASKRUN_LOCK_TESTS").is_none() {
        eprintln!("skipping: set MASKRUN_LOCK_TESTS=1 to run (spawns a keyring daemon)");
        return;
    }

    let Some(session) = IsolatedSession::start() else { return };
    let name = format!("maskrun-lock-test-{}", unique_suffix());
    put_secret(&session, &name, b"still-fine");

    let list = run_maskrun(&session, &["list"], RunOpts::default());
    assert!(list.status.success());
    assert!(text(&list).contains(&name));

    let manifest_dir = tempfile::tempdir().unwrap();
    std::fs::write(manifest_dir.path().join(".maskrun"), format!("TEST_VAR={name}\n")).unwrap();
    let status = run_maskrun(&session, &["status"], RunOpts { cwd: Some(manifest_dir.path()), ..Default::default() });
    assert!(status.status.success(), "{}", text(&status));
    assert!(text(&status).contains("all 1 secret(s) present"));
}
