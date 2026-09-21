// maskrun install-rules. Writes a short, marked block into a project's
// agent-instruction file(s) (AGENTS.md / CLAUDE.md / .cursor/rules) so an
// agent reading them learns how maskrun works here.
//
// This is guidance, not enforcement: install-guard wires `maskrun hook` in as
// a PreToolUse hook, which the harness itself runs against every tool call.
// A harness with no hook mechanism has nothing enforcing anything, so this
// command fills that gap the only way left — by asking nicely, in the same
// file the agent already reads. Say so in the block itself; don't imply this
// blocks anything on its own.

use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::manifest;

const START: &str = "<!-- maskrun:start -->";
const END: &str = "<!-- maskrun:end -->";

pub struct InstallRulesArgs<'a> {
    pub remove: bool,
    pub dry_run: bool,
    pub file: Option<&'a str>,
}

pub fn cmd_install_rules(args: InstallRulesArgs<'_>) -> Result<i32> {
    let targets: Vec<PathBuf> = match args.file {
        Some(f) => vec![PathBuf::from(f)],
        None => choose_targets(&std::env::current_dir()?)?,
    };

    let block = render_block();
    for path in &targets {
        apply_to_file(path, &args, &block)?;
    }
    Ok(0)
}

fn candidate_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let p = root.join(name);
        if p.is_file() {
            out.push(p);
        }
    }
    // .cursor/rules holds one file per rule; maskrun owns a dedicated one
    // rather than guessing which existing file to edit.
    if root.join(".cursor").join("rules").is_dir() {
        out.push(root.join(".cursor").join("rules").join("maskrun.mdc"));
    }
    out
}

fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

fn choose_targets(root: &Path) -> Result<Vec<PathBuf>> {
    let candidates = candidate_files(root);
    if candidates.is_empty() {
        return Ok(vec![root.join("AGENTS.md")]);
    }
    if candidates.len() == 1 || !interactive() {
        return Ok(candidates);
    }

    println!("more than one agent instruction file found:");
    for (i, p) in candidates.iter().enumerate() {
        println!("  {}) {}", i + 1, p.display());
    }
    let choice = crate::prompt_line("update which # (comma-separated, Enter = all): ")?;
    let Some(choice) = choice else {
        return Ok(candidates);
    };
    let mut picked = Vec::new();
    for part in choice.split(',') {
        let idx: usize = part
            .trim()
            .parse()
            .map_err(|_| Error::msg(format!("not a number: {:?}", part.trim())))?;
        let p = candidates
            .get(idx.wrapping_sub(1))
            .cloned()
            .ok_or_else(|| Error::msg(format!("out of range: {idx}")))?;
        picked.push(p);
    }
    Ok(picked)
}

fn manifest_vars() -> Vec<String> {
    let Ok(Some(path)) = manifest::find_manifest(None) else {
        return Vec::new();
    };
    let Ok(pairs) = manifest::read_manifest(&path) else {
        return Vec::new();
    };
    let mut vars: Vec<String> = pairs.into_iter().map(|(var, _)| var).collect();
    vars.sort();
    vars.dedup();
    vars
}

fn render_block() -> String {
    let vars = manifest_vars();
    let mut lines = vec![
        START.to_string(),
        "## Secrets (maskrun)".to_string(),
        String::new(),
        "Secrets live in the OS keyring, managed by maskrun — not in `.env`. \
         Don't read, write, or ask for `.env` contents or secret values."
            .to_string(),
        String::new(),
        "- Run with secrets injected: `maskrun run -- <command>`".to_string(),
        "- Check what's present: `maskrun status`".to_string(),
        "- `maskrun get <name>` is refused in an agent session; don't try to bypass it".to_string(),
        "- Never print, log, or copy a secret value anywhere".to_string(),
    ];
    if !vars.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "Variables this project expects: {}",
            vars.join(", ")
        ));
    }
    lines.push(String::new());
    lines.push(
        "This is guidance for you, not enforcement — the actual block on reading values \
         is `maskrun hook` (a PreToolUse hook), not this file."
            .to_string(),
    );
    lines.push(END.to_string());
    lines.join("\n")
}

fn find_block(content: &str) -> Option<(usize, usize)> {
    let start = content.find(START)?;
    let after_start = start + START.len();
    let end = after_start + content[after_start..].find(END)? + END.len();
    Some((start, end))
}

fn upsert(content: &str, block: &str) -> String {
    match find_block(content) {
        Some((s, e)) => format!("{}{block}{}", &content[..s], &content[e..]),
        None => {
            let mut out = content.to_string();
            if !out.is_empty() {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
            out.push_str(block);
            out.push('\n');
            out
        }
    }
}

// Undoes exactly what `upsert` adds when it appends: the block, plus the one
// blank separator line before it and the one trailing newline after it — so
// installing and then removing gets back to the original content.
fn remove_block(content: &str) -> Option<String> {
    let (s, e) = find_block(content)?;
    let mut before = &content[..s];
    let mut after = &content[e..];
    if before.ends_with("\n\n") {
        before = &before[..before.len() - 1];
    }
    if let Some(stripped) = after.strip_prefix('\n') {
        after = stripped;
    }
    Some(format!("{before}{after}"))
}

fn backup_if_exists(path: &Path, existed: bool) -> Result<()> {
    if !existed {
        return Ok(());
    }
    let backup = PathBuf::from(format!("{}.maskrun-backup", path.display()));
    fs::copy(path, &backup)?;
    println!("backed up: {}", backup.display());
    Ok(())
}

fn write_atomic(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn print_dry_run(path: &Path, content: &str) {
    println!("--- {} would become ---", path.display());
    print!("{content}");
    if !content.ends_with('\n') {
        println!();
    }
    println!("--- dry run: nothing was written ---");
}

fn apply_to_file(path: &Path, args: &InstallRulesArgs<'_>, block: &str) -> Result<()> {
    let existed = path.is_file();
    let content = if existed {
        fs::read_to_string(path).map_err(|e| Error::msg(format!("{}: {e}", path.display())))?
    } else {
        String::new()
    };

    if args.remove {
        let Some(new_content) = remove_block(&content) else {
            println!("no maskrun block in {}", path.display());
            return Ok(());
        };
        if args.dry_run {
            print_dry_run(path, &new_content);
            return Ok(());
        }
        backup_if_exists(path, existed)?;
        write_atomic(path, &new_content)?;
        println!("removed the maskrun block from {}", path.display());
        return Ok(());
    }

    let new_content = upsert(&content, block);
    if args.dry_run {
        print_dry_run(path, &new_content);
        return Ok(());
    }
    backup_if_exists(path, existed)?;
    write_atomic(path, &new_content)?;
    if existed {
        println!("updated the maskrun block in {}", path.display());
    } else {
        println!("created {} with the maskrun block", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_appends_to_existing_content() {
        let out = upsert(
            "Existing text\n",
            "<!-- maskrun:start -->\nx\n<!-- maskrun:end -->",
        );
        assert_eq!(
            out,
            "Existing text\n\n<!-- maskrun:start -->\nx\n<!-- maskrun:end -->\n"
        );
    }

    #[test]
    fn upsert_creates_block_only_content_for_empty_file() {
        let out = upsert("", "<!-- maskrun:start -->\nx\n<!-- maskrun:end -->");
        assert_eq!(out, "<!-- maskrun:start -->\nx\n<!-- maskrun:end -->\n");
    }

    #[test]
    fn upsert_replaces_existing_block_in_place() {
        let content = "before\n\n<!-- maskrun:start -->\nold\n<!-- maskrun:end -->\nafter\n";
        let out = upsert(content, "<!-- maskrun:start -->\nnew\n<!-- maskrun:end -->");
        assert_eq!(
            out,
            "before\n\n<!-- maskrun:start -->\nnew\n<!-- maskrun:end -->\nafter\n"
        );
    }

    #[test]
    fn upsert_is_idempotent() {
        let block = "<!-- maskrun:start -->\nx\n<!-- maskrun:end -->";
        let once = upsert("Existing text\n", block);
        let twice = upsert(&once, block);
        assert_eq!(once, twice);
    }

    #[test]
    fn remove_block_reverses_a_plain_append() {
        let original = "Existing text\n";
        let block = "<!-- maskrun:start -->\nx\n<!-- maskrun:end -->";
        let appended = upsert(original, block);
        let removed = remove_block(&appended).unwrap();
        assert_eq!(removed, original);
    }

    #[test]
    fn remove_block_on_missing_block_returns_none() {
        assert!(remove_block("plain text, no block here\n").is_none());
    }

    #[test]
    fn render_block_stays_within_the_line_budget() {
        let block = render_block();
        assert!(
            block.lines().count() <= 15,
            "block was {} lines:\n{block}",
            block.lines().count()
        );
    }

    #[test]
    fn render_block_is_bracketed_by_markers() {
        let block = render_block();
        assert!(block.starts_with(START));
        assert!(block.ends_with(END));
    }
}
