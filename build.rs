use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=.git/HEAD");
    println!("cargo::rerun-if-changed=.git/index");

    let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let version = match git_identity() {
        Some(id) => format!("{pkg_version} ({id})"),
        None => pkg_version,
    };
    println!("cargo::rustc-env=MASKRUN_VERSION={version}");
}

// None when git isn't on PATH or this checkout has no .git (crates.io/tarball
// builds) — `--version` then falls back to the plain package version instead
// of failing the build.
fn git_identity() -> Option<String> {
    let hash = git(&["rev-parse", "--short=7", "HEAD"])?;
    let dirty = !git(&["status", "--porcelain"])?.is_empty();
    Some(if dirty { format!("{hash}-dirty") } else { hash })
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout)
        .ok()
        .map(|s| s.trim().to_string())
}
