// maskrun install-guard. Not yet implemented — see bin/maskrun's
// cmd_install_guard. serde_json's preserve_order feature (Cargo.toml) is
// required here: rewriting settings.json must not reorder existing keys.

use crate::error::{Error, Result};

#[allow(dead_code)]
pub struct InstallGuardArgs<'a> {
    pub harness: &'a str,
    pub remove: bool,
    pub dry_run: bool,
    pub config: Option<&'a str>,
    pub command_path: Option<&'a str>,
}

pub fn cmd_install_guard(args: InstallGuardArgs<'_>) -> Result<i32> {
    let _ = args;
    Err(Error::msg("install-guard: not yet implemented"))
}
