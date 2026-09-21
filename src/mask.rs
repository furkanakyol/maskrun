// Output redaction for run/exec. Not yet implemented — see bin/maskrun's
// value_variants, build_filter, StreamPump, run_masked.

#![allow(dead_code)]

use crate::error::{Error, Result};

pub fn value_variants(_value: &str) -> Vec<String> {
    Vec::new()
}

pub fn build_filter(_pairs: &[(String, String)]) -> Option<()> {
    None
}

pub fn run_masked(
    command: &[String],
    injected: &std::collections::HashMap<String, String>,
    masked: bool,
) -> Result<i32> {
    let _ = (command, injected, masked);
    Err(Error::msg("run: output masking not yet implemented"))
}
