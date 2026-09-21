use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use zeroize::Zeroize;

use crate::error::{Error, Result};

pub const MANIFEST: &str = ".maskrun";

// Client-bundled prefixes: these ship to the browser anyway, so they are
// configuration, not secrets, and moving them buys nothing.
const PUBLIC_PREFIXES: &[&str] = &[
    "VITE_",
    "NEXT_PUBLIC_",
    "PUBLIC_",
    "REACT_APP_",
    "NUXT_PUBLIC_",
    "EXPO_PUBLIC_",
    "GATSBY_",
];

pub fn find_manifest(start: Option<&Path>) -> Result<Option<PathBuf>> {
    let mut dir: PathBuf = match start {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?,
    };
    dir = dir.canonicalize().unwrap_or(dir);
    loop {
        let candidate = dir.join(MANIFEST);
        if candidate.is_file() {
            return Ok(Some(candidate));
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => return Ok(None),
        }
    }
}

pub fn read_manifest(path: &Path) -> Result<Vec<(String, String)>> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| Error::msg(format!("{}: {e}", path.display())))?;
    let mut pairs = Vec::new();
    for (lineno, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = || {
            Error::msg(format!(
                "{}:{}: expected VAR=secret-name, got {:?}",
                path.display(),
                lineno + 1,
                line
            ))
        };
        let (var, secret) = line.split_once('=').ok_or_else(bad)?;
        let (var, secret) = (var.trim(), secret.trim());
        if var.is_empty() || secret.is_empty() {
            return Err(bad());
        }
        pairs.push((var.to_string(), secret.to_string()));
    }
    Ok(pairs)
}

pub fn require_manifest() -> Result<PathBuf> {
    find_manifest(None)?.ok_or_else(|| {
        Error::msg(format!(
            "no {MANIFEST} found in this directory or any parent.\n\
             Create one with lines like:\n\
             \x20   DATABASE_URL=myapp-database-url\n\
             or generate it from an existing .env with: maskrun import .env"
        ))
    })
}

fn env_line_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=(.*)$").unwrap())
}

fn parse_env_file(path: &Path) -> Result<Vec<(String, String)>> {
    let mut content = std::fs::read_to_string(path)
        .map_err(|e| Error::msg(format!("{}: {e}", path.display())))?;
    let mut out = Vec::new();
    for raw in content.lines() {
        let line = raw.trim_end_matches('\r');
        let stripped = line.trim_start();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        let Some(caps) = env_line_pattern().captures(line) else {
            continue;
        };
        let var = caps[1].to_string();
        let mut value = caps[2].trim().to_string();
        if value.len() >= 2 {
            let bytes = value.as_bytes();
            let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
            if first == last && (first == b'"' || first == b'\'') {
                value = value[1..value.len() - 1].to_string();
            }
        }
        out.push((var, value));
    }
    // The whole .env is sitting in this one String; zero it now rather than
    // leaving its raw bytes for whenever the allocator reuses that memory.
    content.zeroize();
    Ok(out)
}

pub fn cmd_import(
    envfile: &str,
    prefix: Option<&str>,
    dry_run: bool,
    all: bool,
    backend: &dyn crate::keyring::Backend,
) -> Result<i32> {
    crate::agent::refuse_in_agent(
        "maskrun import",
        "  Reading a .env pulls its entire contents into the transcript, which\n\
         \x20 is the exact thing maskrun exists to prevent.",
    )?;
    let path = Path::new(envfile);
    if !path.is_file() {
        return Err(Error::msg(format!("no such file: {envfile}")));
    }
    let prefix = match prefix {
        Some(p) => p.to_string(),
        None => std::env::current_dir()?
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };

    let mut rows: Vec<(String, String, String)> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for (var, value) in parse_env_file(path)? {
        if !all && PUBLIC_PREFIXES.iter().any(|p| var.starts_with(p)) {
            skipped.push(var);
            continue;
        }
        let secret = format!("{prefix}-{}", var.to_lowercase().replace('_', "-"));
        let secret = crate::keyring::check_name(&secret)?.to_string();
        rows.push((var, secret, value));
    }

    if rows.is_empty() {
        return Err(Error::msg(format!("nothing to import from {envfile}")));
    }

    for (var, secret, value) in &rows {
        if dry_run {
            println!(
                "  {var:<28} -> {secret:<40} ({} chars)",
                value.chars().count()
            );
        } else {
            backend.put(secret, value)?;
            println!("  stored {var:<28} -> {secret}");
        }
    }

    if !skipped.is_empty() {
        println!(
            "\nskipped {} client-bundled variable(s) (not secrets): {}",
            skipped.len(),
            skipped.join(", ")
        );
        println!("Use --all to import them anyway.");
    }

    let body: String = rows
        .iter()
        .map(|(var, secret, _)| format!("{var}={secret}\n"))
        .collect();
    let header = "# maskrun manifest — secret NAMES, never values. Safe to commit.\n\
                  # Run commands with: maskrun run -- <command>\n";

    let result = if dry_run {
        println!(
            "\n--- {MANIFEST} that would be written ({} lines) ---",
            rows.len()
        );
        print!("{body}");
        println!("--- dry run: nothing was written ---");
        Ok(0)
    } else {
        std::fs::write(MANIFEST, format!("{header}{body}"))
            .map_err(|e| Error::msg(format!("{MANIFEST}: {e}")))?;
        println!("\n{} secret(s) stored, {MANIFEST} written.", rows.len());
        println!("Verify with: maskrun status");
        println!("Then check it runs (maskrun run -- <command>) BEFORE deleting {envfile}.");
        Ok(0)
    };

    for (_, _, value) in rows.iter_mut() {
        value.zeroize();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_manifest(dir: &Path, content: &str) -> PathBuf {
        let path = dir.join(MANIFEST);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn reads_pairs_preserving_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_manifest(dir.path(), "B=b-name\nA=a-name\n# comment\n\nC=c-name\n");
        let pairs = read_manifest(&path).unwrap();
        assert_eq!(
            pairs,
            vec![
                ("B".into(), "b-name".into()),
                ("A".into(), "a-name".into()),
                ("C".into(), "c-name".into()),
            ]
        );
    }

    #[test]
    fn rejects_line_without_equals() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_manifest(dir.path(), "no equals here\n");
        let err = read_manifest(&path).unwrap_err();
        assert!(err.to_string().contains("expected VAR=secret-name"));
    }

    #[test]
    fn finds_manifest_from_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        write_manifest(dir.path(), "TEST=x\n");
        let deep = dir.path().join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        let found = find_manifest(Some(&deep)).unwrap();
        assert_eq!(
            found.unwrap(),
            dir.path().canonicalize().unwrap().join(MANIFEST)
        );
    }

    #[test]
    fn find_manifest_returns_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nowhere");
        std::fs::create_dir_all(&missing).unwrap();
        let found = find_manifest(Some(&missing)).unwrap();
        assert!(found.is_none());
    }
}
