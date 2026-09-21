mod agent;
mod error;
mod guard;
mod install_guard;
mod keyring;
mod manifest;
mod mask;
mod run;
mod tui;

use std::io::{IsTerminal, Read, Write};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use zeroize::Zeroizing;

use error::{Error, Result};
use keyring::{Backend, FieldUpdate, MetaUpdate, SecretMeta};

#[derive(Parser)]
#[command(
    name = "maskrun",
    version = env!("MASKRUN_VERSION"),
    about = "Run commands with secrets from your OS keyring, without leaking them into an AI agent's context.",
    after_help = "examples:\n  \
        maskrun put myapp-database-url        store a secret (prompts, not echoed)\n  \
        maskrun put                           fully interactive: asks name, platform, note, value\n  \
        maskrun label gh-token --for github   tag an existing secret's platform\n  \
        maskrun import .env --dry-run         see what would move to the keyring\n  \
        maskrun status                        is every manifest entry present?\n  \
        maskrun run -- npm run dev            run with secrets injected, output masked\n  \
        maskrun exec API_KEY=my-key -- curl https://api.example.com\n  \
        maskrun list                          grouped by platform, never values\n  \
        maskrun list --plain                  flat names only, one per line, for scripts\n  \
        maskrun completions fish > ~/.config/fish/completions/maskrun.fish\n\n\
        Output masking is automatic in an AI agent session and whenever stdout is not a\n\
        terminal. --raw turns it off, --mask forces it on."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "store a secret in the keyring (no args: asks for everything)")]
    Put {
        name: Option<String>,
        #[arg(long, help = "read the value from stdin instead of prompting")]
        stdin: bool,
        #[arg(long = "for", help = "platform/service this secret belongs to")]
        for_platform: Option<String>,
        #[arg(long, help = "free-text note, one line, <=200 chars")]
        note: Option<String>,
    },
    #[command(about = "print a secret (refused in an agent session)")]
    Get { name: String },
    #[command(about = "list secrets, grouped by platform (never values)")]
    List {
        #[arg(
            long,
            help = "flat sorted names, one per line — the old, script-facing format"
        )]
        plain: bool,
    },
    #[command(about = "delete a secret (no name: pick from a list, with confirmation)")]
    Rm { name: Option<String> },
    #[command(about = "check the manifest against the keyring")]
    Status,
    #[command(about = "run a command with the manifest injected")]
    Run {
        #[arg(long, help = "do not mask output")]
        raw: bool,
        #[arg(long, help = "always mask output")]
        mask: bool,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    #[command(about = "run with explicit VAR=secret-name pairs")]
    Exec {
        #[arg(long, help = "do not mask output")]
        raw: bool,
        #[arg(long, help = "always mask output")]
        mask: bool,
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            help = "VAR=secret-name"
        )]
        assignment: Vec<String>,
    },
    #[command(about = "move a .env into the keyring")]
    Import {
        envfile: String,
        #[arg(long, help = "secret name prefix (default: directory name)")]
        prefix: Option<String>,
        #[arg(long = "dry-run", help = "write nothing")]
        dry_run: bool,
        #[arg(
            long,
            help = "also import client-bundled vars (VITE_, NEXT_PUBLIC_, ...)"
        )]
        all: bool,
    },
    #[command(about = "tag an existing secret's platform and/or note (no name: pick from a list)")]
    Label {
        name: Option<String>,
        #[arg(long = "for", help = "platform/service label (\"\" clears it)")]
        for_platform: Option<String>,
        #[arg(long, help = "free-text note (\"\" clears it)")]
        note: Option<String>,
    },
    #[command(about = "print a shell completion script")]
    Completions { shell: clap_complete::Shell },
    #[command(about = "PreToolUse guard for agent harnesses (reads JSON on stdin)")]
    Hook,
    #[command(about = "add the guard hook to an agent harness's config")]
    InstallGuard {
        #[arg(
            long,
            default_value = "claude-code",
            help = "which harness (default: claude-code)"
        )]
        harness: String,
        #[arg(long, help = "uninstall instead")]
        remove: bool,
        #[arg(long = "dry-run", help = "print, write nothing")]
        dry_run: bool,
        #[arg(
            long,
            help = "settings file to edit (default: ~/.claude/settings.json)"
        )]
        config: Option<String>,
        #[arg(
            long = "command-path",
            help = "how to invoke maskrun (default: 'maskrun')"
        )]
        command_path: Option<String>,
    },
}

fn split_leading_command(argv: Vec<String>) -> Vec<String> {
    if argv.first().map(String::as_str) == Some("--") {
        let mut out = vec!["run".to_string()];
        out.extend(argv);
        out
    } else {
        argv
    }
}

// Split at the first `--`; everything after is the child command verbatim,
// including any later `--`. Done by hand rather than left to clap: `exec`
// needs the tokens before the first `--`, `run` needs the tokens after it,
// so the split has to happen before either subcommand's own parsing does.
fn extract_command(argv: Vec<String>) -> (Vec<String>, Vec<String>) {
    match argv.iter().position(|a| a == "--") {
        Some(pos) => (argv[..pos].to_vec(), argv[pos + 1..].to_vec()),
        None => (argv, Vec::new()),
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let argv = split_leading_command(argv);
    let (head, tail) = extract_command(argv);

    let mut full_head = vec!["maskrun".to_string()];
    full_head.extend(head);

    let cli = Cli::try_parse_from(&full_head).unwrap_or_else(|e| e.exit());

    let code = match cli.cmd {
        None => exit_code(cmd_overview()),
        Some(cmd) => exit_code(dispatch(cmd, tail)),
    };

    std::io::stdout().flush().ok();
    std::io::stderr().flush().ok();
    ExitCode::from((code & 0xff) as u8)
}

fn exit_code(result: Result<i32>) -> i32 {
    match result {
        Ok(code) => code,
        Err(Error::BrokenPipe) => 0,
        Err(err) => {
            eprintln!("maskrun: {err}");
            1
        }
    }
}

// Mirrors the exact "no name, can't prompt" condition each of
// cmd_put/cmd_label/cmd_rm_interactive (via choose_secret) already checks
// internally — duplicated here only so dispatch() can fail before handing
// control to a function that receives an already-constructed backend.
fn require_name_before_backend(cmd: &Command) -> Result<()> {
    match cmd {
        Command::Put {
            name: None, stdin, ..
        } if *stdin || !interactive_stdio() => Err(Error::msg(
            "secret name required. Usage: maskrun put <name>",
        )),
        Command::Label { name: None, .. } if !interactive_stdio() => Err(Error::msg(
            "secret name required. Usage: maskrun label <name> [--for X] [--note Y]",
        )),
        Command::Rm { name: None } if !interactive_stdio() => {
            Err(Error::msg("secret name required. Usage: maskrun rm <name>"))
        }
        _ => Ok(()),
    }
}

fn dispatch(cmd: Command, tail: Vec<String>) -> Result<i32> {
    // A missing required name with no tty to prompt on is a usage error,
    // not a keyring problem — it must be reported as one even when the
    // keyring is unreachable. Checked before `pick_backend()` below, not
    // inside cmd_put/cmd_label/cmd_rm_interactive, precisely so this
    // never gets as far as asking the keyring anything (see the "linux ·
    // no keyring" CI job, where the old order reported "could not reach
    // the Secret Service" for what was actually just `maskrun put` typed
    // with no name in a script).
    require_name_before_backend(&cmd)?;

    let backend: Option<Box<dyn Backend>> = match &cmd {
        Command::Hook | Command::InstallGuard { .. } | Command::Completions { .. } => None,
        _ => Some(keyring::pick_backend()?),
    };

    match cmd {
        Command::Put {
            name,
            stdin,
            for_platform,
            note,
        } => cmd_put(name, stdin, for_platform, note, backend.as_deref().unwrap()),
        Command::Get { name } => cmd_get(&name, backend.as_deref().unwrap()),
        Command::List { plain } => cmd_list(plain, backend.as_deref().unwrap()),
        Command::Rm { name } => cmd_rm_interactive(name, backend.as_deref().unwrap()),
        Command::Status => cmd_status(backend.as_deref().unwrap()),
        Command::Run { raw, mask, command } => {
            let mut full_command = command;
            full_command.extend(tail);
            run::cmd_run(&full_command, raw, mask, backend.as_deref().unwrap())
        }
        Command::Exec {
            raw,
            mask,
            assignment,
        } => {
            for item in &assignment {
                if !item.contains('=') {
                    eprintln!(
                        "maskrun: {item:?} is not a VAR=secret-name pair. Put '--' before \
                         the command: maskrun exec VAR=name -- <command>"
                    );
                    return Ok(1);
                }
            }
            run::cmd_exec(&assignment, &tail, raw, mask, backend.as_deref().unwrap())
        }
        Command::Import {
            envfile,
            prefix,
            dry_run,
            all,
        } => manifest::cmd_import(
            &envfile,
            prefix.as_deref(),
            dry_run,
            all,
            backend.as_deref().unwrap(),
        ),
        Command::Label {
            name,
            for_platform,
            note,
        } => cmd_label(name, for_platform, note, backend.as_deref().unwrap()),
        Command::Completions { shell } => cmd_completions(shell),
        Command::Hook => guard::cmd_hook(),
        Command::InstallGuard {
            harness,
            remove,
            dry_run,
            config,
            command_path,
        } => install_guard::cmd_install_guard(install_guard::InstallGuardArgs {
            harness: &harness,
            remove,
            dry_run,
            config: config.as_deref(),
            command_path: command_path.as_deref(),
        }),
    }
}

// Both TTYs, not an AI agent driving the shell: prompts are only useful to a
// human who can actually see and answer them. `--stdin`/non-interactive
// callers never reach the functions that check this.
fn interactive_stdio() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal() && !agent::in_agent()
}

pub(crate) fn prompt_line(prompt: &str) -> Result<Option<String>> {
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let trimmed = line.trim();
    Ok(if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    })
}

// Shared by --for on `put`/`label` and by the interactive platform prompt:
// absent -> leave alone, "" -> clear, anything else -> validate and set.
pub(crate) fn platform_update(raw: Option<String>) -> Result<FieldUpdate> {
    match raw {
        None => Ok(FieldUpdate::Keep),
        Some(s) if s.is_empty() => Ok(FieldUpdate::Clear),
        Some(s) => Ok(FieldUpdate::Set(keyring::check_name(&s)?.to_string())),
    }
}

pub(crate) fn note_update(raw: Option<String>) -> Result<FieldUpdate> {
    match raw {
        None => Ok(FieldUpdate::Keep),
        Some(s) if s.is_empty() => Ok(FieldUpdate::Clear),
        Some(s) => Ok(FieldUpdate::Set(keyring::check_note(&s)?.to_string())),
    }
}

fn choose_secret(backend: &dyn Backend, verb: &str) -> Result<String> {
    if !interactive_stdio() {
        return Err(Error::msg(format!(
            "secret name required. Usage: maskrun {verb} <name>"
        )));
    }
    let names = backend.list()?;
    if names.is_empty() {
        return Err(Error::msg("no secrets stored"));
    }
    println!("secrets:");
    for (i, n) in names.iter().enumerate() {
        println!("  {}) {n}", i + 1);
    }
    let choice = prompt_line(&format!("{verb} which # : "))?
        .ok_or_else(|| Error::msg("no selection, nothing done"))?;
    let idx: usize = choice
        .parse()
        .map_err(|_| Error::msg(format!("not a number: {choice:?}")))?;
    names
        .into_iter()
        .nth(idx.wrapping_sub(1))
        .ok_or_else(|| Error::msg("out of range"))
}

fn prompt_platform_hint(backend: &dyn Backend) -> Result<Option<String>> {
    if let Ok(entries) = backend.list_meta() {
        let mut platforms: Vec<String> = entries
            .into_iter()
            .filter_map(|(_, meta)| meta.platform)
            .collect();
        platforms.sort();
        platforms.dedup();
        if !platforms.is_empty() {
            println!("existing platforms: {}", platforms.join(", "));
        }
    }
    prompt_line("Platform (Enter to skip): ")
}

fn prompt_value_confirmed(secret: &str) -> Result<Zeroizing<String>> {
    loop {
        let a = Zeroizing::new(
            rpassword::prompt_password(format!("Value for {secret} (not echoed): "))
                .map_err(|e| Error::msg(format!("could not read a password: {e}")))?,
        );
        if a.is_empty() {
            return Err(Error::msg("empty value, nothing stored"));
        }
        let b = Zeroizing::new(
            rpassword::prompt_password("Confirm (not echoed): ")
                .map_err(|e| Error::msg(format!("could not read a password: {e}")))?,
        );
        if *a == *b {
            return Ok(a);
        }
        eprintln!("maskrun: values did not match, try again.");
    }
}

fn cmd_put(
    name: Option<String>,
    from_stdin: bool,
    for_platform: Option<String>,
    note: Option<String>,
    backend: &dyn Backend,
) -> Result<i32> {
    let interactive = !from_stdin && interactive_stdio();

    let name = match name {
        Some(n) => n,
        None if interactive => {
            prompt_line("Secret name: ")?.ok_or_else(|| Error::msg("empty name, nothing stored"))?
        }
        None => {
            return Err(Error::msg(
                "secret name required. Usage: maskrun put <name>",
            ))
        }
    };
    let secret = keyring::check_name(&name)?.to_string();

    if interactive && backend.get(&secret)?.is_some() {
        println!(
            "{secret} already exists — its value will be overwritten; its platform/note \
             stay unless you set new ones below."
        );
    }

    let for_platform = match for_platform {
        Some(v) => Some(v),
        None if interactive => prompt_platform_hint(backend)?,
        None => None,
    };
    let note = match note {
        Some(v) => Some(v),
        None if interactive => prompt_line("Note (Enter to skip): ")?,
        None => None,
    };
    let meta = MetaUpdate {
        platform: platform_update(for_platform)?,
        note: note_update(note)?,
    };

    let value: Zeroizing<String> = if from_stdin {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        if buf.ends_with('\n') {
            buf.pop();
        }
        Zeroizing::new(buf)
    } else if interactive {
        prompt_value_confirmed(&secret)?
    } else {
        let prompt = format!("Value for {secret} (not echoed): ");
        let value = rpassword::prompt_password(prompt)
            .map_err(|e| Error::msg(format!("could not read a password: {e}")))?;
        if value.is_empty() {
            return Err(Error::msg("empty value, nothing stored"));
        }
        Zeroizing::new(value)
    };

    backend.put(&secret, &value, &meta)?;
    println!("stored: {secret}");
    Ok(0)
}

fn cmd_get(name: &str, backend: &dyn Backend) -> Result<i32> {
    agent::refuse_in_agent(
        "maskrun get",
        "  To USE the secret:  maskrun run -- <command>\n\
         \x20                     maskrun exec VAR=name -- <command>\n\
         \x20 To see which secrets exist:  maskrun list",
    )?;
    let secret = keyring::check_name(name)?;
    let value = backend
        .get(secret)?
        .ok_or_else(|| Error::msg(format!("no such secret: {secret}")))?;
    let mut stdout = std::io::stdout();
    stdout.write_all(value.as_bytes())?;
    if !value.ends_with('\n') {
        stdout.write_all(b"\n")?;
    }
    Ok(0)
}

fn cmd_list(plain: bool, backend: &dyn Backend) -> Result<i32> {
    if plain {
        let names = backend.list()?;
        if names.is_empty() {
            println!("(no secrets stored)");
            return Ok(0);
        }
        for name in names {
            println!("{name}");
        }
        return Ok(0);
    }

    let mut entries = backend.list_meta()?;
    if entries.is_empty() {
        println!("(no secrets stored)");
        return Ok(0);
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    print_grouped(&entries);
    Ok(0)
}

// Platforms alphabetical, "(no platform)" last, names alphabetical within
// each group (guaranteed by the caller pre-sorting `entries` by name).
fn print_grouped(entries: &[(String, SecretMeta)]) {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<&str, Vec<&(String, SecretMeta)>> = BTreeMap::new();
    let mut unlabelled: Vec<&(String, SecretMeta)> = Vec::new();
    for entry in entries {
        match entry.1.platform.as_deref() {
            Some(p) => groups.entry(p).or_default().push(entry),
            None => unlabelled.push(entry),
        }
    }
    for (platform, items) in &groups {
        println!("{platform}");
        print_group_items(items);
    }
    if !unlabelled.is_empty() {
        println!("(no platform)");
        print_group_items(&unlabelled);
    }
}

fn print_group_items(items: &[&(String, SecretMeta)]) {
    let width = items
        .iter()
        .map(|(n, _)| n.chars().count())
        .max()
        .unwrap_or(0);
    for (name, meta) in items {
        match meta.note.as_deref() {
            Some(note) => println!("  {name:<width$}   {note}"),
            None => println!("  {name}"),
        }
    }
}

fn cmd_rm_interactive(name: Option<String>, backend: &dyn Backend) -> Result<i32> {
    let (secret, needs_confirm) = match name {
        Some(n) => (n, false),
        None => (choose_secret(backend, "rm")?, true),
    };
    if needs_confirm {
        let answer = prompt_line(&format!("delete '{secret}'? [y/N] "))?;
        if !matches!(answer.as_deref(), Some("y" | "Y" | "yes" | "YES")) {
            println!("cancelled");
            return Ok(0);
        }
    }
    cmd_rm(&secret, backend)
}

fn cmd_rm(name: &str, backend: &dyn Backend) -> Result<i32> {
    let secret = keyring::check_name(name)?;
    if backend.get(secret)?.is_none() {
        return Err(Error::msg(format!("no such secret: {secret}")));
    }
    backend.delete(secret)?;
    println!("deleted: {secret}");
    Ok(0)
}

fn cmd_label(
    name: Option<String>,
    for_platform: Option<String>,
    note: Option<String>,
    backend: &dyn Backend,
) -> Result<i32> {
    let interactive = interactive_stdio();
    let name = match name {
        Some(n) => n,
        None if interactive => choose_secret(backend, "label")?,
        None => {
            return Err(Error::msg(
                "secret name required. Usage: maskrun label <name> [--for X] [--note Y]",
            ))
        }
    };
    let secret = keyring::check_name(&name)?.to_string();

    let (for_platform, note) = if for_platform.is_none() && note.is_none() && interactive {
        (
            prompt_platform_hint(backend)?,
            prompt_line("Note (Enter to skip): ")?,
        )
    } else {
        (for_platform, note)
    };

    let platform = platform_update(for_platform)?;
    let note = note_update(note)?;
    if platform == FieldUpdate::Keep && note == FieldUpdate::Keep {
        return Err(Error::msg(
            "nothing to do: give --for, --note, or both (or answer at least one prompt)",
        ));
    }
    backend.set_meta(&secret, &MetaUpdate { platform, note })?;
    println!("labelled: {secret}");
    Ok(0)
}

fn cmd_completions(shell: clap_complete::Shell) -> Result<i32> {
    use clap::CommandFactory;
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
    // clap_complete only knows the static flag/subcommand surface. Completing
    // an actual secret name means shelling back out to `maskrun list
    // --plain`, which is exactly why that flag's output is frozen as one
    // sorted name per line.
    if shell == clap_complete::Shell::Fish {
        print!("{FISH_DYNAMIC_COMPLETIONS}");
    }
    Ok(0)
}

const FISH_DYNAMIC_COMPLETIONS: &str = "
complete -c maskrun -n '__fish_seen_subcommand_from get rm label' -f -a '(maskrun list --plain 2>/dev/null)'
";

fn cmd_status(backend: &dyn Backend) -> Result<i32> {
    let path = manifest::require_manifest()?;
    let pairs = manifest::read_manifest(&path)?;
    println!("manifest: {}", path.display());
    println!("backend:  {}", backend.name());
    let mut missing = 0usize;
    for (var, secret) in &pairs {
        let present = matches!(backend.get(secret)?, Some(v) if !v.is_empty());
        let mark = if present { "ok     " } else { "MISSING" };
        let platform = backend.get_meta(secret).unwrap_or_default().platform;
        match platform {
            Some(p) => println!("  {mark} {var:<28} -> {secret}  [{p}]"),
            None => println!("  {mark} {var:<28} -> {secret}"),
        }
        if !present {
            missing += 1;
        }
    }
    if missing > 0 {
        println!("\n{missing} secret(s) missing. Add them with: maskrun put <name>");
        return Ok(1);
    }
    println!("\nall {} secret(s) present.", pairs.len());
    Ok(0)
}

// Bare `maskrun`. Piped/CI output (`maskrun | cat`, no tty) always gets the
// static summary below. A real interactive terminal gets the arrow-key TUI
// (platform -> secret -> view/edit/delete/copy) in tui.rs, which falls back
// to this same summary itself on a too-small terminal, an unreachable
// backend, or (redundantly, on purpose — see tui::run_overview) an agent
// session.
fn cmd_overview() -> Result<i32> {
    if interactive_stdio() {
        tui::run_overview()
    } else {
        cmd_overview_summary()
    }
}

// An at-a-glance view instead of a help dump, so the command surface
// doesn't have to live in the user's head.
pub(crate) fn cmd_overview_summary() -> Result<i32> {
    println!("maskrun — run commands with secrets from your OS keyring\n");

    match keyring::pick_backend() {
        Ok(backend) => {
            match manifest::find_manifest(None) {
                Ok(Some(path)) => match manifest::read_manifest(&path) {
                    Ok(pairs) => {
                        let missing = pairs
                            .iter()
                            .filter(
                                |(_, s)| !matches!(backend.get(s), Ok(Some(v)) if !v.is_empty()),
                            )
                            .count();
                        println!(
                            "manifest: {} ({} secret(s), {missing} missing)",
                            path.display(),
                            pairs.len()
                        );
                    }
                    Err(e) => println!("manifest: {} ({e})", path.display()),
                },
                _ => println!("no .maskrun manifest here (see: maskrun import <.env>)"),
            }
            println!();
            match backend.list_meta() {
                Ok(mut entries) if !entries.is_empty() => {
                    entries.sort_by(|a, b| a.0.cmp(&b.0));
                    println!("secrets ({}):", entries.len());
                    print_grouped(&entries);
                }
                Ok(_) => println!("(no secrets stored)"),
                Err(e) => println!("could not list secrets: {e}"),
            }
        }
        Err(e) => println!("keyring backend not reachable: {e}"),
    }

    println!();
    println!("next:");
    println!("  maskrun put <name>          store a secret");
    println!("  maskrun run -- <command>    run with the manifest injected");
    println!("  maskrun --help              full command reference");
    Ok(0)
}

#[allow(dead_code)]
fn _uses_is_terminal() -> bool {
    std::io::stdout().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_leading_command_bare_double_dash_becomes_run() {
        let argv = vec!["--".to_string(), "echo".to_string(), "hi".to_string()];
        let out = split_leading_command(argv);
        assert_eq!(out, vec!["run", "--", "echo", "hi"]);
    }

    #[test]
    fn split_leading_command_leaves_other_input_alone() {
        let argv = vec!["status".to_string()];
        assert_eq!(split_leading_command(argv.clone()), argv);
    }

    #[test]
    fn extract_command_splits_at_first_double_dash_only() {
        let argv: Vec<String> = ["run", "--", "npm", "run", "dev", "--", "--flag"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (head, tail) = extract_command(argv);
        assert_eq!(head, vec!["run".to_string()]);
        assert_eq!(
            tail,
            vec!["npm", "run", "dev", "--", "--flag"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn extract_command_exec_assignment_then_command() {
        let argv: Vec<String> = ["exec", "FOO=x", "--", "echo", "hi"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (head, tail) = extract_command(argv);
        assert_eq!(head, vec!["exec".to_string(), "FOO=x".to_string()]);
        assert_eq!(tail, vec!["echo".to_string(), "hi".to_string()]);
    }

    #[test]
    fn extract_command_no_double_dash_is_all_head() {
        let argv: Vec<String> = ["status"].iter().map(|s| s.to_string()).collect();
        let (head, tail) = extract_command(argv);
        assert_eq!(head, vec!["status".to_string()]);
        assert!(tail.is_empty());
    }

    #[test]
    fn full_pipeline_bare_shorthand_matches_run() {
        let argv: Vec<String> = ["--", "npm", "run", "dev", "--", "--flag"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let argv = split_leading_command(argv);
        let (head, tail) = extract_command(argv);
        assert_eq!(head, vec!["run".to_string()]);
        assert_eq!(
            tail,
            vec!["npm", "run", "dev", "--", "--flag"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn platform_update_absent_keeps() {
        assert_eq!(platform_update(None).unwrap(), FieldUpdate::Keep);
    }

    #[test]
    fn platform_update_empty_clears() {
        assert_eq!(
            platform_update(Some(String::new())).unwrap(),
            FieldUpdate::Clear
        );
    }

    #[test]
    fn platform_update_validates_charset() {
        assert!(platform_update(Some("has space".to_string())).is_err());
    }

    #[test]
    fn note_update_rejects_newline() {
        assert!(note_update(Some("a\nb".to_string())).is_err());
    }
}
