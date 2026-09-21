mod agent;
mod error;
mod guard;
mod install_guard;
mod keyring;
mod manifest;
mod mask;
mod run;

use std::io::{IsTerminal, Read, Write};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use zeroize::Zeroizing;

use error::{Error, Result};
use keyring::Backend;

#[derive(Parser)]
#[command(
    name = "maskrun",
    version,
    about = "Run commands with secrets from your OS keyring, without leaking them into an AI agent's context.",
    after_help = "examples:\n  \
        maskrun put myapp-database-url        store a secret (prompts, not echoed)\n  \
        maskrun import .env --dry-run         see what would move to the keyring\n  \
        maskrun status                        is every manifest entry present?\n  \
        maskrun run -- npm run dev            run with secrets injected, output masked\n  \
        maskrun exec API_KEY=my-key -- curl https://api.example.com\n  \
        maskrun list                          names only, never values\n\n\
        Output masking is automatic in an AI agent session and whenever stdout is not a\n\
        terminal. --raw turns it off, --mask forces it on."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "store a secret in the keyring")]
    Put {
        name: String,
        #[arg(long, help = "read the value from stdin instead of prompting")]
        stdin: bool,
    },
    #[command(about = "print a secret (refused in an agent session)")]
    Get { name: String },
    #[command(about = "list secret names (never values)")]
    List,
    #[command(about = "delete a secret")]
    Rm { name: String },
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
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, help = "VAR=secret-name")]
        assignment: Vec<String>,
    },
    #[command(about = "move a .env into the keyring")]
    Import {
        envfile: String,
        #[arg(long, help = "secret name prefix (default: directory name)")]
        prefix: Option<String>,
        #[arg(long = "dry-run", help = "write nothing")]
        dry_run: bool,
        #[arg(long, help = "also import client-bundled vars (VITE_, NEXT_PUBLIC_, ...)")]
        all: bool,
    },
    #[command(about = "PreToolUse guard for agent harnesses (reads JSON on stdin)")]
    Hook,
    #[command(about = "add the guard hook to an agent harness's config")]
    InstallGuard {
        #[arg(long, default_value = "claude-code", help = "which harness (default: claude-code)")]
        harness: String,
        #[arg(long, help = "uninstall instead")]
        remove: bool,
        #[arg(long = "dry-run", help = "print, write nothing")]
        dry_run: bool,
        #[arg(long, help = "settings file to edit (default: ~/.claude/settings.json)")]
        config: Option<String>,
        #[arg(long = "command-path", help = "how to invoke maskrun (default: 'maskrun')")]
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
        None => {
            use clap::CommandFactory;
            Cli::command().print_help().ok();
            println!();
            1
        }
        Some(cmd) => match dispatch(cmd, tail) {
            Ok(code) => code,
            Err(Error::BrokenPipe) => 0,
            Err(err) => {
                eprintln!("maskrun: {err}");
                1
            }
        },
    };

    std::io::stdout().flush().ok();
    std::io::stderr().flush().ok();
    ExitCode::from((code & 0xff) as u8)
}

fn dispatch(cmd: Command, tail: Vec<String>) -> Result<i32> {
    let backend: Option<Box<dyn Backend>> = match &cmd {
        Command::Hook | Command::InstallGuard { .. } => None,
        _ => Some(keyring::pick_backend()?),
    };

    match cmd {
        Command::Put { name, stdin } => cmd_put(&name, stdin, backend.as_deref().unwrap()),
        Command::Get { name } => cmd_get(&name, backend.as_deref().unwrap()),
        Command::List => cmd_list(backend.as_deref().unwrap()),
        Command::Rm { name } => cmd_rm(&name, backend.as_deref().unwrap()),
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

fn cmd_put(name: &str, from_stdin: bool, backend: &dyn Backend) -> Result<i32> {
    let secret = keyring::check_name(name)?;
    let value: Zeroizing<String> = if from_stdin {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        if buf.ends_with('\n') {
            buf.pop();
        }
        Zeroizing::new(buf)
    } else {
        let prompt = format!("Value for {secret} (not echoed): ");
        let value = rpassword::prompt_password(prompt)
            .map_err(|e| Error::msg(format!("could not read a password: {e}")))?;
        if value.is_empty() {
            return Err(Error::msg("empty value, nothing stored"));
        }
        Zeroizing::new(value)
    };
    backend.put(secret, &value)?;
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

fn cmd_list(backend: &dyn Backend) -> Result<i32> {
    let names = backend.list()?;
    if names.is_empty() {
        println!("(no secrets stored)");
        return Ok(0);
    }
    for name in names {
        println!("{name}");
    }
    Ok(0)
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

fn cmd_status(backend: &dyn Backend) -> Result<i32> {
    let path = manifest::require_manifest()?;
    let pairs = manifest::read_manifest(&path)?;
    println!("manifest: {}", path.display());
    println!("backend:  {}", backend.name());
    let mut missing = 0usize;
    for (var, secret) in &pairs {
        let present = matches!(backend.get(secret)?, Some(v) if !v.is_empty());
        let mark = if present { "ok     " } else { "MISSING" };
        println!("  {mark} {var:<28} -> {secret}");
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
}
