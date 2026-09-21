// Agent guard (`maskrun hook`). Not yet implemented — see bin/maskrun's
// guard_decision and test/test_maskrun.py's BLOCK/ALLOW tables.

use crate::error::{Error, Result};

pub fn cmd_hook() -> Result<i32> {
    Err(Error::msg("hook: not yet implemented"))
}
