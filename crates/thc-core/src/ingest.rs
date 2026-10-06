//! `drop/` ingestion: any .md/.txt file becomes inbox nodes, one per list item or paragraph.

use crate::builder::TxBuilder;
use crate::capture;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

/// Claim files by renaming them with this device's id; returns claimed paths.
pub fn claim(drop_dir: &Path, device: &str) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !drop_dir.exists() {
        return Ok(out);
    }
    let claimed_dir = drop_dir.join(".claimed");
    fs::create_dir_all(&claimed_dir)?;
    for entry in fs::read_dir(drop_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || !entry.file_type()?.is_file() {
            continue;
        }
        if !(name.ends_with(".md") || name.ends_with(".txt")) {
            continue;
        }
        let target = claimed_dir.join(format!("{device}--{name}"));
        // Losing a rename race just means another device took it.
        if fs::rename(entry.path(), &target).is_ok() {
            out.push(target);
        }
    }
    out.sort();
    Ok(out)
}

/// Split a dropped file into capture strings: bullets and paragraphs.
pub fn items(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let flush = |para: &mut Vec<String>, out: &mut Vec<String>| {
        if !para.is_empty() {
            out.push(para.join(" "));
            para.clear();
        }
    };
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') && t.chars().nth(1) == Some(' ') {
            flush(&mut para, &mut out);
            continue;
        }
        if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("[ ]") {
            flush(&mut para, &mut out);
            out.push(t.to_string());
        } else {
            para.push(t.to_string());
        }
    }
    flush(&mut para, &mut out);
    out
}

/// What one dropped file became.
pub struct Ingested {
    pub ids: Vec<String>,
    /// Tokens kept as plain text because their value didn't parse, as notices for `thc doctor`.
    pub kept: Vec<String>,
}

/// Every item becomes an inbox node. Nobody reads an exit code here, so parsing is lenient: a
/// token whose value doesn't parse (`due:fryday`) stays in the text and is reported, never dropped.
pub fn ingest_file(b: &mut TxBuilder, path: &Path) -> Result<Ingested> {
    let text = fs::read_to_string(path)?;
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let name = name.split_once("--").map(|(_, n)| n.to_string()).unwrap_or(name);
    let mut out = Ingested { ids: Vec::new(), kept: Vec::new() };
    for item in items(&text) {
        let (cap, bad) = capture::parse_lenient(&item, b.today)?;
        let id = b.create_from_capture(None, &cap, None)?;
        for tok in bad {
            out.kept.push(format!("drop/{name}: kept {tok:?} as text in {}", &id[..5]));
        }
        out.ids.push(id);
    }
    Ok(out)
}

/// The notices file `thc doctor` reads (per device cache, newest last, the last 50 kept).
pub fn notices_path(cache: &Path) -> PathBuf {
    cache.join("notices.jsonl")
}

pub fn record_notices(cache: &Path, notes: &[String]) -> Result<()> {
    if notes.is_empty() {
        return Ok(());
    }
    let path = notices_path(cache);
    let mut lines: Vec<String> = fs::read_to_string(&path).unwrap_or_default().lines().map(str::to_string).collect();
    let at = chrono::Local::now().format("%Y-%m-%dT%H:%M").to_string();
    for n in notes {
        lines.push(serde_json::json!({ "at": at, "notice": n }).to_string());
    }
    let keep = lines.len().saturating_sub(50);
    fs::write(&path, lines[keep..].join("\n") + "\n")?;
    Ok(())
}

/// Recorded notices, newest first: (local time, text).
pub fn read_notices(cache: &Path) -> Vec<(String, String)> {
    let text = fs::read_to_string(notices_path(cache)).unwrap_or_default();
    let mut out: Vec<(String, String)> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .map(|v| (v["at"].as_str().unwrap_or("").to_string(), v["notice"].as_str().unwrap_or("").to_string()))
        .collect();
    out.reverse();
    out
}

pub fn archive(path: &Path, drop_dir: &Path) -> Result<()> {
    let done = drop_dir.join(".ingested");
    fs::create_dir_all(&done)?;
    let name = path.file_name().unwrap();
    fs::rename(path, done.join(name))?;
    Ok(())
}
