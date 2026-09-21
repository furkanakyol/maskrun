use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

pub const MANIFEST: &str = ".maskrun";

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

// Not yet implemented — see bin/maskrun's cmd_import, parse_env_file and
// PUBLIC_PREFIXES for the reference behaviour.
pub fn cmd_import(
    _envfile: &str,
    _prefix: Option<&str>,
    _dry_run: bool,
    _all: bool,
    _backend: &dyn crate::keyring::Backend,
) -> Result<i32> {
    Err(Error::msg("import: not yet implemented"))
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
        assert_eq!(found.unwrap(), dir.path().canonicalize().unwrap().join(MANIFEST));
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
