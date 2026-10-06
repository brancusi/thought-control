//! Per-device append-only JSONL logs: `log/<device>/<YYYY-MM>.jsonl`.
//! A device only ever appends to its own directory, so sync tools never see two writers.

use crate::event::{Event, FORMAT_VERSION, KNOWN_OPS};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
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
            // Vault's writer lock is cache-local. Separate caches can still use the same
            // device log (e.g. THC_DEVICE in fixtures), so serialize the size measurement
            // and the whole append here too. Closing this descriptor releases the lock.
            // SAFETY: f owns a valid descriptor for the duration of this append.
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(std::io::Error::last_os_error()).with_context(|| format!("locking {}", path.display()));
            }
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

    fn event(eid: &str, text: &str) -> Event {
        serde_json::from_value(serde_json::json!({
            "v": 1, "eid": eid, "hlc": [1, 0], "dev": "dev",
            "actor": {"kind": "human"}, "via": "cli", "tx": eid,
            "op": "node.text", "id": "aaaaaaaaaaaa", "text": text
        })).unwrap()
    }

    #[test]
    fn empty_and_incomplete_lines_wait_for_a_newline() {
        let dir = std::env::temp_dir().join(format!("thc-log-partial-{}", std::process::id()));
        fs::create_dir_all(dir.join("dev")).unwrap();
        let path = dir.join("dev/2026-10.jsonl");
        let log = Log::new(dir.clone());
        fs::write(&path, "").unwrap();
        let empty = log.read_from("dev/2026-10.jsonl", 0).unwrap();
        assert_eq!(empty.new_offset, 0);
        assert!(empty.bad_lines.is_empty());
        let line = serde_json::to_vec(&event("partial", "a multibyte note: café")).unwrap();
        let split = line.len() / 2;
        fs::write(&path, [&b"\n \t\n"[..], &line[..split]].concat()).unwrap();
        let partial = log.read_from("dev/2026-10.jsonl", 0).unwrap();
        assert_eq!(partial.new_offset, 4);
        assert!(partial.events.is_empty());
        assert!(partial.bad_lines.is_empty());
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(&line[split..]).unwrap();
        f.write_all(b"\n").unwrap();
        let complete = log.read_from("dev/2026-10.jsonl", partial.new_offset).unwrap();
        assert_eq!(complete.events, [event("partial", "a multibyte note: café")]);
        assert!(complete.bad_lines.is_empty());
        assert_eq!(complete.new_offset, fs::metadata(&path).unwrap().len());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_appends_return_their_actual_byte_ranges() {
        let dir = std::env::temp_dir().join(format!("thc-log-append-race-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let log = Log::new(dir.clone());
        let barrier = std::sync::Barrier::new(16);
        let mut ranges = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..16).map(|i| {
                let log = &log;
                let barrier = &barrier;
                scope.spawn(move || {
                    let e = event(&format!("writer-{i}"), &"x".repeat(128_000 + i));
                    barrier.wait();
                    let range = log.append("dev", std::slice::from_ref(&e)).unwrap().remove(0);
                    (range, e)
                })
            }).collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect::<Vec<_>>()
        });
        ranges.sort_by_key(|((_, before, _), _)| *before);
        let mut end = 0;
        for ((rel, before, after), e) in &ranges {
            assert_eq!(*before, end, "append ranges must neither overlap nor leave gaps");
            let chunk = log.read_from(rel, *before).unwrap();
            assert!(chunk.bad_lines.is_empty(), "{:?}", chunk.bad_lines);
            assert_eq!(chunk.events.first().map(|e| &e.eid), Some(&e.eid));
            end = *after;
        }
        let rel = &ranges[0].0.0;
        assert_eq!(end, fs::metadata(dir.join(rel)).unwrap().len());
        assert_eq!(log.read_from(rel, 0).unwrap().events.len(), 16);
        fs::remove_dir_all(dir).unwrap();
    }

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
