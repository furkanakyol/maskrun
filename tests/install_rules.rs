// Integration tests for `maskrun install-rules`, run against the compiled
// binary in scratch temp directories — never the real project tree.

use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{n}", std::process::id())
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "maskrun-install-rules-test-{name}-{}",
        unique_suffix()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_in(cwd: &std::path::Path, args: &[&str], stdin_data: Option<&[u8]>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_maskrun"));
    cmd.args(args);
    cmd.current_dir(cwd);
    // These would make prompting or agent-detection behave unexpectedly if
    // they leaked in from the environment this test suite runs under.
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
    ] {
        cmd.env_remove(var);
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

fn install_rules(cwd: &std::path::Path, extra: &[&str]) -> Output {
    let mut args = vec!["install-rules"];
    args.extend_from_slice(extra);
    run_in(cwd, &args, None)
}

#[test]
fn empty_dir_creates_agents_md() {
    let dir = temp_dir("empty-dir");
    let out = install_rules(&dir, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("created"), "stdout was: {stdout}");

    let content = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    assert!(content.contains("<!-- maskrun:start -->"));
    assert!(content.contains("<!-- maskrun:end -->"));
    assert!(content.contains("maskrun run -- <command>"));
    assert!(content.contains("maskrun status"));
    assert!(content.contains("maskrun get"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn existing_file_content_survives_and_block_is_appended() {
    let dir = temp_dir("existing-content");
    let original = "# My project\n\nSome existing instructions here.\n";
    std::fs::write(dir.join("AGENTS.md"), original).unwrap();

    let out = install_rules(&dir, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let backup = dir.join("AGENTS.md.maskrun-backup");
    assert!(backup.exists(), "backup file was not created");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);

    let content = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    assert!(
        content.starts_with(original),
        "existing content was altered:\n{content}"
    );
    assert!(content.contains("<!-- maskrun:start -->"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn running_twice_does_not_duplicate_the_block() {
    let dir = temp_dir("no-duplicate");
    std::fs::write(dir.join("CLAUDE.md"), "notes\n").unwrap();

    let out1 = install_rules(&dir, &[]);
    assert!(out1.status.success());
    let after_first = std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
    assert_eq!(after_first.matches("<!-- maskrun:start -->").count(), 1);

    let out2 = install_rules(&dir, &[]);
    assert!(out2.status.success());
    let after_second = std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
    assert_eq!(after_second.matches("<!-- maskrun:start -->").count(), 1);
    assert_eq!(after_first, after_second, "a second run must be a no-op");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn manifest_variables_are_listed_in_the_block() {
    let dir = temp_dir("manifest");
    std::fs::write(
        dir.join(".maskrun"),
        "DATABASE_URL=myapp-database-url\nJWT_SECRET=myapp-jwt-secret\n",
    )
    .unwrap();

    let out = install_rules(&dir, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let content = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    assert!(content.contains("DATABASE_URL"), "block was:\n{content}");
    assert!(content.contains("JWT_SECRET"), "block was:\n{content}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_manifest_omits_the_variables_line() {
    let dir = temp_dir("no-manifest");
    let out = install_rules(&dir, &[]);
    assert!(out.status.success());
    let content = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    assert!(!content.contains("Variables this project expects"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remove_takes_out_the_block_and_keeps_the_rest() {
    let dir = temp_dir("remove");
    let original = "# My project\n\nSome existing instructions here.\n";
    std::fs::write(dir.join("AGENTS.md"), original).unwrap();

    let out = install_rules(&dir, &[]);
    assert!(out.status.success());

    let out2 = install_rules(&dir, &["--remove"]);
    assert!(
        out2.status.success(),
        "{}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let stdout = String::from_utf8_lossy(&out2.stdout);
    assert!(stdout.contains("removed"), "stdout was: {stdout}");

    let content = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    assert_eq!(content, original);
    assert!(!content.contains("maskrun:start"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remove_when_absent_is_a_no_op() {
    let dir = temp_dir("remove-absent");
    std::fs::write(dir.join("AGENTS.md"), "just some notes\n").unwrap();

    let out = install_rules(&dir, &["--remove"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("no maskrun block"));

    let content = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    assert_eq!(content, "just some notes\n");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dry_run_writes_nothing() {
    let dir = temp_dir("dry-run");
    let out = install_rules(&dir, &["--dry-run"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("dry run: nothing was written"));
    assert!(stdout.contains("maskrun:start"));
    assert!(
        !dir.join("AGENTS.md").exists(),
        "dry-run must not create a file"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dry_run_on_existing_file_leaves_it_untouched() {
    let dir = temp_dir("dry-run-existing");
    let original = "notes\n";
    std::fs::write(dir.join("AGENTS.md"), original).unwrap();

    let out = install_rules(&dir, &["--dry-run"]);
    assert!(out.status.success());
    assert_eq!(
        std::fs::read_to_string(dir.join("AGENTS.md")).unwrap(),
        original
    );
    assert!(!dir.join("AGENTS.md.maskrun-backup").exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn file_flag_targets_only_that_file_without_prompting() {
    let dir = temp_dir("file-flag");
    std::fs::write(dir.join("AGENTS.md"), "agents notes\n").unwrap();
    std::fs::write(dir.join("CLAUDE.md"), "claude notes\n").unwrap();

    let out = run_in(&dir, &["install-rules", "--file", "CLAUDE.md"], None);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let claude = std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
    assert!(claude.contains("maskrun:start"));
    let agents = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    assert_eq!(
        agents, "agents notes\n",
        "--file must not touch other candidates"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn multiple_candidates_non_interactive_updates_all_of_them() {
    let dir = temp_dir("multi-noninteractive");
    std::fs::write(dir.join("AGENTS.md"), "agents notes\n").unwrap();
    std::fs::write(dir.join("CLAUDE.md"), "claude notes\n").unwrap();

    // Test harness stdio is piped, not a tty, so this must not block on a
    // prompt — it should just update every candidate it found.
    let out = install_rules(&dir, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let agents = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    let claude = std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
    assert!(agents.contains("maskrun:start"));
    assert!(claude.contains("maskrun:start"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cursor_rules_directory_gets_its_own_dedicated_file() {
    let dir = temp_dir("cursor-rules");
    std::fs::create_dir_all(dir.join(".cursor").join("rules")).unwrap();

    let out = install_rules(&dir, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let path = dir.join(".cursor").join("rules").join("maskrun.mdc");
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("maskrun:start"));

    let _ = std::fs::remove_dir_all(&dir);
}
