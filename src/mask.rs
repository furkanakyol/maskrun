// Output redaction for run/exec. Ports bin/maskrun's value_variants,
// build_filter, StreamPump and run_masked.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use regex::bytes::{Captures, Regex};
use zeroize::Zeroize;

use crate::error::{Error, Result};

pub const MIN_MASK_LEN: usize = 6;
// A slow producer can flush a secret across two writes. Holding back a tail
// means the filter always sees the whole value before it decides what to
// pass through — see StreamPump::pump.
const TAIL_HOLD: usize = 512;

// urllib.parse.quote(value, safe="") keeps letters, digits and "_.-~"
// unescaped (its default safe set minus '/'); NON_ALPHANUMERIC escapes those
// too; removing them restores the same safe set.
const QUOTE_SAFE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'_')
    .remove(b'.')
    .remove(b'-')
    .remove(b'~');

pub fn value_variants(value: &str) -> Vec<String> {
    let mut candidates = vec![
        value.to_string(),
        BASE64.encode(value.as_bytes()),
        utf8_percent_encode(value, QUOTE_SAFE).to_string(),
    ];
    if value.contains('\\') || value.contains('"') || value.contains('\n') {
        candidates.push(
            value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n"),
        );
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for candidate in candidates {
        if candidate.chars().count() >= MIN_MASK_LEN && seen.insert(candidate.clone()) {
            out.push(candidate);
        }
    }
    out
}

pub struct FilterSpec {
    pattern: Regex,
    table: HashMap<Vec<u8>, Vec<u8>>,
    line_safe: bool,
}

impl Drop for FilterSpec {
    fn drop(&mut self) {
        for (mut variant, _) in self.table.drain() {
            variant.zeroize();
        }
    }
}

pub fn build_filter(pairs: &[(String, String)]) -> Option<FilterSpec> {
    let mut entries: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut short: Vec<&str> = Vec::new();
    for (var, value) in pairs {
        if value.is_empty() {
            continue;
        }
        if value.chars().count() < MIN_MASK_LEN {
            short.push(var);
            continue;
        }
        let label = format!("<masked:{var}>").into_bytes();
        for variant in value_variants(value) {
            entries.push((variant.into_bytes(), label.clone()));
        }
    }

    if !short.is_empty() {
        eprintln!(
            "maskrun: {} shorter than {} characters, left unmasked (masking a short value \
             would corrupt unrelated output).",
            short.join(", "),
            MIN_MASK_LEN
        );
    }

    if entries.is_empty() {
        return None;
    }

    // Longest variant first: regex alternation matches the first alternative
    // that fits at a position, not the longest overall, so a short variant
    // that happens to be a substring of a longer one must not shadow it.
    entries.sort_by_key(|(variant, _)| std::cmp::Reverse(variant.len()));

    let mut table = HashMap::with_capacity(entries.len());
    for (variant, label) in &entries {
        table.insert(variant.clone(), label.clone());
    }
    let line_safe = !entries.iter().any(|(v, _)| v.contains(&b'\n'));

    let pattern = entries
        .iter()
        .map(|(v, _)| regex::escape(std::str::from_utf8(v).expect("variant is valid utf-8")))
        .collect::<Vec<_>>()
        .join("|");
    let pattern = Regex::new(&pattern).expect("escaped literal alternation must compile");

    Some(FilterSpec { pattern, table, line_safe })
}

struct StreamPump<R: Read, W: Write> {
    source: R,
    sink: W,
    spec: Arc<FilterSpec>,
    buffer: Vec<u8>,
}

impl<R: Read, W: Write> StreamPump<R, W> {
    fn mask(&self, data: &[u8]) -> Vec<u8> {
        let table = &self.spec.table;
        self.spec
            .pattern
            .replace_all(data, |caps: &Captures| {
                table.get(&caps[0]).cloned().unwrap_or_default()
            })
            .into_owned()
    }

    fn write(&mut self, data: &mut [u8]) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        let masked = self.mask(data);
        self.sink.write_all(&masked)?;
        self.sink.flush()?;
        data.zeroize();
        Ok(())
    }

    // Threads rather than a readiness poll: select()/poll() do not work on
    // anonymous pipes on Windows, so both platforms need one thread per
    // stream regardless.
    fn pump(&mut self) -> Result<()> {
        let mut chunk = [0u8; 65536];
        loop {
            let n = self.source.read(&mut chunk)?;
            if n == 0 {
                let mut tail = std::mem::take(&mut self.buffer);
                self.write(&mut tail)?;
                return Ok(());
            }
            self.buffer.extend_from_slice(&chunk[..n]);
            chunk.zeroize();

            let mut ready = if self.spec.line_safe && self.buffer.contains(&b'\n') {
                let cut = self.buffer.iter().rposition(|&b| b == b'\n').unwrap();
                self.buffer.drain(..=cut).collect::<Vec<u8>>()
            } else if self.buffer.len() > TAIL_HOLD {
                let cut = self.buffer.len() - TAIL_HOLD;
                self.buffer.drain(..cut).collect::<Vec<u8>>()
            } else {
                Vec::new()
            };
            self.write(&mut ready)?;
        }
    }
}

pub fn run_masked(command: &[String], mut injected: HashMap<String, String>) -> Result<i32> {
    let pairs: Vec<(String, String)> =
        injected.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let Some(spec) = build_filter(&pairs) else {
        let code = crate::run::exec_with_env(command, &injected);
        for value in injected.values_mut() {
            value.zeroize();
        }
        return code;
    };
    let spec = Arc::new(spec);

    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .envs(&injected)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::msg(format!("command not found: {}", command[0]))
            } else {
                Error::from(e)
            }
        })?;
    for value in injected.values_mut() {
        value.zeroize();
    }

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    let out_spec = Arc::clone(&spec);
    let out_thread = std::thread::spawn(move || {
        StreamPump { source: stdout, sink: std::io::stdout(), spec: out_spec, buffer: Vec::new() }
            .pump()
    });
    let err_spec = Arc::clone(&spec);
    let err_thread = std::thread::spawn(move || {
        StreamPump { source: stderr, sink: std::io::stderr(), spec: err_spec, buffer: Vec::new() }
            .pump()
    });

    let status = child.wait()?;
    let _ = out_thread.join();
    let _ = err_thread.join();

    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_skip_short_values() {
        assert!(value_variants("abc").is_empty());
    }

    #[test]
    fn variants_include_encodings() {
        let variants = value_variants("secret-value-here");
        assert!(variants.contains(&"secret-value-here".to_string()));
        assert!(variants.contains(&BASE64.encode(b"secret-value-here")));
    }

    #[test]
    fn variants_include_urlencoding_and_backslash_escape() {
        let variants = value_variants("a value/with \"quotes\"");
        let want_url = utf8_percent_encode("a value/with \"quotes\"", QUOTE_SAFE).to_string();
        assert!(variants.contains(&want_url));
        assert!(variants.iter().any(|v| v.contains("\\\"")));
    }

    #[test]
    fn variants_dedupe() {
        // percent-encoding an alnum-only value is a no-op, so raw and
        // url-encoded collapse to one entry.
        let variants = value_variants("abcdefgh");
        let raw_count = variants.iter().filter(|v| v.as_str() == "abcdefgh").count();
        assert_eq!(raw_count, 1);
    }

    #[test]
    fn filter_is_none_when_nothing_maskable() {
        assert!(build_filter(&[("A".into(), "abc".into())]).is_none());
    }

    #[test]
    fn filter_masks_longest_variant_first() {
        let pairs = vec![("A".into(), "sixchr".to_string()), ("B".into(), "sixchrlonger".to_string())];
        let spec = build_filter(&pairs).unwrap();
        let masked = spec.pattern.replace_all(b"sixchrlonger", |c: &Captures| {
            spec.table.get(&c[0]).cloned().unwrap_or_default()
        });
        assert_eq!(masked, b"<masked:B>".to_vec());
    }

    // Delivers each chunk from a separate `read()` call, unlike a Cursor
    // (which can hand back everything at once) — this is what actually
    // exercises the tail-hold logic instead of merely asserting on it.
    struct ChunkedReader {
        chunks: std::collections::VecDeque<Vec<u8>>,
    }

    impl Read for ChunkedReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.chunks.pop_front() {
                Some(chunk) => {
                    let n = chunk.len();
                    buf[..n].copy_from_slice(&chunk);
                    Ok(n)
                }
                None => Ok(0),
            }
        }
    }

    #[test]
    fn stream_pump_catches_value_split_across_reads() {
        let value = "abcdefghijklmnopqrstuvwx"; // 24 chars, >= MIN_MASK_LEN
        let spec = Arc::new(build_filter(&[("TEST".into(), value.into())]).unwrap());
        let (first, second) = value.split_at(10);
        let reader = ChunkedReader {
            chunks: [format!("head {first}").into_bytes(), format!("{second} tail\n").into_bytes()]
                .into_iter()
                .collect(),
        };
        let mut sink = Vec::new();
        StreamPump { source: reader, sink: &mut sink, spec, buffer: Vec::new() }
            .pump()
            .unwrap();
        let out = String::from_utf8(sink).unwrap();
        assert!(!out.contains(value));
        assert!(out.contains("<masked:TEST>"));
    }

    #[test]
    fn stream_pump_tail_hold_catches_split_with_no_newline() {
        // A value with no trailing newline relies on TAIL_HOLD, not the
        // line-boundary cut, to keep the two halves together.
        let value = "abcdefghijklmnopqrstuvwx";
        let spec = Arc::new(build_filter(&[("TEST".into(), value.into())]).unwrap());
        let (first, second) = value.split_at(10);
        let reader = ChunkedReader {
            chunks: [first.as_bytes().to_vec(), second.as_bytes().to_vec()].into_iter().collect(),
        };
        let mut sink = Vec::new();
        StreamPump { source: reader, sink: &mut sink, spec, buffer: Vec::new() }
            .pump()
            .unwrap();
        let out = String::from_utf8(sink).unwrap();
        assert!(!out.contains(value));
        assert!(out.contains("<masked:TEST>"));
    }
}
