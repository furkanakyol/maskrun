// maskrun run / exec. Not yet implemented — see bin/maskrun's cmd_run,
// cmd_exec, collect, exec_with_env.

use crate::error::{Error, Result};
use crate::keyring::Backend;

pub fn cmd_run(command: &[String], raw: bool, mask: bool, backend: &dyn Backend) -> Result<i32> {
    let _ = (command, raw, mask, backend);
    Err(Error::msg("run: not yet implemented"))
}

pub fn cmd_exec(
    assignments: &[String],
    command: &[String],
    raw: bool,
    mask: bool,
    backend: &dyn Backend,
) -> Result<i32> {
    let _ = (assignments, command, raw, mask, backend);
    Err(Error::msg("exec: not yet implemented"))
}
