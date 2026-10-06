//! Crash recovery (hardening for milestone 1): what's typed and not saved yet survives a panic.
//!
//! The loop keeps a snapshot of the open document's unsaved lines here (cheap: one pass over the
//! lines, cloning only the unsaved ones). A panic can't unwind (`panic = "abort"`), so the panic
//! hook writes the snapshot to `recovery.json` in the vault's cache. The next time that document
//! opens, its lines go back in, as one undo step, and the bar says so.

use crate::app::App;
use crate::doc::{Doc, Line, Target};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thc_core::outline::Kind;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RecTarget {
    Journal { date: String },
    Page { id: String },
}

/// One unsaved line: a saved note's new text (by id), or a new line placed after `prev`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecLine {
    pub id: String,
    pub is_new: bool,
    /// The line before it in the document (None: the first line).
    pub prev: Option<String>,
    pub depth: usize,
    pub kind: Kind,
    pub status: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recovery {
    pub target: RecTarget,
    pub lines: Vec<RecLine>,
}

/// The snapshot and where to write it, for the panic hook.
static SNAPSHOT: Mutex<Option<(PathBuf, Recovery)>> = Mutex::new(None);

pub fn file(cache: &Path) -> PathBuf {
    cache.join("recovery.json")
}

fn rec_target(t: &Target) -> RecTarget {
    match t {
        Target::Journal { date } => RecTarget::Journal { date: date.format("%Y-%m-%d").to_string() },
        Target::Page { id, .. } => RecTarget::Page { id: id.clone() },
    }
}

/// A document's unsaved lines, as a recovery (None: nothing unsaved).
pub fn unsaved(d: &Doc) -> Option<Recovery> {
    let mut lines = Vec::new();
    let mut prev: Option<&str> = None;
    for l in d.lines() {
        if !l.text.trim().is_empty() && l.edited() {
            lines.push(RecLine { id: l.id.clone(), is_new: l.is_new, prev: prev.map(str::to_string), depth: l.depth, kind: l.kind, status: l.status.clone(), text: l.text.clone() });
        }
        prev = Some(&l.id);
    }
    (!lines.is_empty()).then(|| Recovery { target: rec_target(&d.target), lines })
}

/// After each turn of the loop: what a crash now would lose. Nothing unsaved: no snapshot.
pub fn note(app: &App) {
    let rec = app.doc.as_ref().and_then(unsaved);
    if let Ok(mut s) = SNAPSHOT.lock() {
        *s = rec.map(|r| (file(&app.vault.paths.cache), r));
    }
}

/// In the panic hook: write the snapshot, if there's anything to keep. Never panics itself.
pub fn write_on_panic() {
    let Ok(s) = SNAPSHOT.try_lock() else { return };
    if let Some((path, rec)) = s.as_ref() {
        if let Ok(json) = serde_json::to_vec(rec) {
            if let Ok(mut f) = std::fs::File::create(path) {
                use std::io::Write;
                let _ = f.write_all(&json);
                let _ = f.sync_all();
            }
        }
    }
}

/// When `target` opens: the recovery for it, if one was left (and the file is gone after).
pub fn take(cache: &Path, target: &Target) -> Option<Recovery> {
    let path = file(cache);
    let rec: Recovery = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    if rec.target != rec_target(target) {
        return None;
    }
    let _ = std::fs::remove_file(&path);
    Some(rec)
}

/// Put recovered lines back: a note still here takes its recovered text; a new line goes after
/// the line it followed (or at the end). One undo step. Returns how many lines came back.
pub fn apply(d: &mut Doc, rec: &Recovery) -> usize {
    d.begin_recovery();
    let mut n = 0;
    for r in &rec.lines {
        if let Some(l) = d.lines_mut().iter_mut().find(|l| l.id == r.id) {
            if l.text != r.text || l.kind != r.kind || l.depth != r.depth {
                l.text = r.text.clone();
                l.kind = r.kind;
                l.depth = r.depth;
                l.status = r.status.clone();
                n += 1;
            }
            continue;
        }
        let at = r.prev.as_ref().and_then(|p| d.lines().iter().position(|l| &l.id == p)).map_or(d.lines().len(), |i| i + 1);
        let mut l = Line::new(r.depth, r.kind, &r.text);
        l.status = r.status.clone();
        // A line that was saved once and isn't here now: a new note (its id may be gone).
        if r.is_new {
            l.id = r.id.clone();
        }
        d.lines_mut().insert(at, l);
        n += 1;
    }
    n
}

/// At start: a recovery waiting for a document that isn't the one opening, in words for the bar
/// (`§ mon 05 oct`, `¶ Lisbon`), so it isn't forgotten.
pub fn waiting(app: &App) -> Option<String> {
    let rec: Recovery = serde_json::from_slice(&std::fs::read(file(&app.vault.paths.cache)).ok()?).ok()?;
    if app.doc.as_ref().is_some_and(|d| rec_target(&d.target) == rec.target) {
        return None;
    }
    Some(match &rec.target {
        RecTarget::Journal { date } => match chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") {
            Ok(d) => format!("§ {}", d.format("%a %d %b").to_string().to_lowercase()),
            Err(_) => format!("§ {date}"),
        },
        RecTarget::Page { id } => format!("¶ {}", app.node_label(id)),
    })
}
