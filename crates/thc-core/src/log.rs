//! Per-device append-only JSONL logs: `log/<device>/<YYYY-MM>.jsonl`.
//! A device only ever appends to its own directory, so sync tools never see two writers.

use crate::event::{Event, FORMAT_VERSION, KNOWN_OPS};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub struct Log {
    pub root: PathBuf,
}

pub struct FileChunk {
    pub rel: String,
    pub events: Vec<Event>,
    pub new_offset: u64,
    pub bad_lines: Vec<String>,
    /// Lines from a newer format (unknown op), skipped on purpose.
    pub skipped: usize,
}

impl Log {
    pub fn new(root: PathBuf) -> Log {
        Log { root }
    }

    pub fn rel_path(dev: &str, ms: u64) -> String {
        let dt: DateTime<Utc> = DateTime::from_timestamp_millis(ms as i64).unwrap_or_default();
        format!("{dev}/{}.jsonl", dt.format("%Y-%m"))
    }

    /// Append events (all from this device) and fsync. Returns (rel path, size before, size after).
    pub fn append(&self, dev: &str, events: &[Event]) -> Result<Vec<(String, u64, u64)>> {
        let mut grouped: BTreeMap<String, String> = BTreeMap::new();
        for e in events {
            let line = serde_json::to_string(e)?;
            let buf = grouped.entry(Log::rel_path(dev, e.hlc.ms())).or_default();
            buf.push_str(&line);
            buf.push('\n');
        }
        let mut out = Vec::new();
        for (rel, buf) in grouped {
            let path = self.root.join(&rel);
            fs::create_dir_all(path.parent().unwrap())?;
            let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
            let before = f.metadata()?.len();
            f.write_all(buf.as_bytes())?;
            f.sync_all()?;
            out.push((rel, before, before + buf.len() as u64));
        }
        Ok(out)
    }

    /// All log files with their sizes, as paths relative to the log root.
    pub fn files(&self) -> Result<Vec<(String, u64)>> {
        let mut out = Vec::new();
        if !self.root.exists() {
            return Ok(out);
        }
        for dev in fs::read_dir(&self.root)? {
            let dev = dev?;
            if !dev.file_type()?.is_dir() {
                continue;
            }
            let dev_name = dev.file_name().to_string_lossy().to_string();
            if dev_name.starts_with('.') {
                continue;
            }
            for f in fs::read_dir(dev.path())? {
                let f = f?;
                let name = f.file_name().to_string_lossy().to_string();
                // Skip sync-tool artefacts such as "2026-10 (conflicted copy).jsonl".
                if !name.ends_with(".jsonl") || name.contains(' ') || name.starts_with('.') {
                    continue;
                }
                out.push((format!("{dev_name}/{name}"), f.metadata()?.len()));
            }
        }
        out.sort();
        Ok(out)
    }

    /// Read complete lines starting at `offset`. A trailing partial line is left for later.
    pub fn read_from(&self, rel: &str, offset: u64) -> Result<FileChunk> {
        let path = self.root.join(rel);
        let mut f = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        let complete = match buf.iter().rposition(|&b| b == b'\n') {
            Some(i) => i + 1,
            None => 0,
        };
        let mut events = Vec::new();
        let mut bad_lines = Vec::new();
        let mut skipped = 0;
        for line in buf[..complete].split(|&b| b == b'\n') {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            match serde_json::from_slice::<Event>(line) {
                Ok(e) => events.push(e),
                Err(_) if is_newer(line) => skipped += 1,
                Err(err) => bad_lines.push(format!("{rel}: {err}")),
            }
        }
        Ok(FileChunk { rel: rel.to_string(), events, new_offset: offset + complete as u64, bad_lines, skipped })
    }

    pub fn exists(&self) -> bool {
        Path::new(&self.root).exists()
    }
}

/// A well-formed event from a newer writer: an op we don't know, or a version above ours.
fn is_newer(line: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else { return false };
    let op = v.get("op").and_then(|o| o.as_str());
    let ver = v.get("v").and_then(|x| x.as_u64()).unwrap_or(1);
    match op {
        Some(op) => !KNOWN_OPS.contains(&op) || ver > FORMAT_VERSION as u64,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_ops_are_skipped_not_reported() {
        let dir = std::env::temp_dir().join(format!("thc-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("dev")).unwrap();
        let known = r#"{"v":1,"eid":"E1","hlc":[1,0],"dev":"dev","actor":{"kind":"human"},"via":"cli","tx":"T","op":"node.delete","id":"aaaaaaaaaaaa"}"#;
        let future = r#"{"v":3,"eid":"E2","hlc":[2,0],"dev":"dev","actor":{"kind":"human"},"via":"cli","tx":"T","op":"node.fold","id":"aaaaaaaaaaaa"}"#;
        fs::write(dir.join("dev/2026-10.jsonl"), format!("{known}\n{future}\nnot json\n")).unwrap();
        let c = Log::new(dir.clone()).read_from("dev/2026-10.jsonl", 0).unwrap();
        assert_eq!((c.events.len(), c.skipped, c.bad_lines.len()), (1, 1, 1));
        let _ = fs::remove_dir_all(&dir);
    }
}

