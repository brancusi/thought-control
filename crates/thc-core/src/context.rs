//! Contexts (docs/design/views.md §2): a view applied by default to a person's listings and
//! captures. Local state only: never in the log, never synced.
//!
//! Where it comes from, in order: `THC_CONTEXT`, a `.thc-context` file in the current directory
//! or a parent, then this device's setting (`<cache>/context`, written by `thc context <name>`).

use crate::error::not_found;
use crate::store::Store;
use crate::views::{self, View};
use anyhow::Result;
use chrono::NaiveDate;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub const FILE: &str = ".thc-context";

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// `--context <name>` on this command.
    Flag,
    Env,
    File(PathBuf),
    Device,
}

impl Source {
    /// Explicit sources apply to JSON output and agents too; the file and device ones don't.
    pub fn explicit(&self) -> bool {
        matches!(self, Source::Flag | Source::Env)
    }

    pub fn describe(&self) -> String {
        match self {
            Source::Flag => "from --context".into(),
            Source::Env => "from THC_CONTEXT".into(),
            Source::File(p) => format!("from {FILE} in {}", p.parent().map(|d| d.display().to_string()).unwrap_or_default()),
            Source::Device => "this device's setting".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Context {
    pub name: String,
    pub source: Source,
}

fn device_file(cache: &Path) -> PathBuf {
    cache.join("context")
}

/// This device's setting, if any.
pub fn device(cache: &Path) -> Option<String> {
    fs::read_to_string(device_file(cache)).ok().map(|s| s.trim().trim_start_matches('@').to_string()).filter(|s| !s.is_empty())
}

/// `thc context <name>` / `thc context none`.
pub fn set_device(cache: &Path, name: Option<&str>) -> Result<()> {
    match name {
        Some(n) => fs::write(device_file(cache), format!("{n}\n"))?,
        None => {
            let _ = fs::remove_file(device_file(cache));
        }
    }
    Ok(())
}

fn find_file(start: &Path) -> Option<(PathBuf, String)> {
    let mut dir = start.to_path_buf();
    loop {
        let f = dir.join(FILE);
        if let Ok(s) = fs::read_to_string(&f) {
            let n = s.trim().trim_start_matches('@').to_string();
            if !n.is_empty() {
                return Some((f, n));
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// The active context, if any. `flag` is `--context <name>` (`none` turns it off for this command).
pub fn resolve(flag: Option<&str>, cache: &Path, cwd: &Path) -> Option<Context> {
    if let Some(f) = flag {
        let n = f.trim_start_matches('@');
        return (n != "none" && !n.is_empty()).then(|| Context { name: n.to_string(), source: Source::Flag });
    }
    if let Ok(e) = std::env::var("THC_CONTEXT") {
        let n = e.trim().trim_start_matches('@').to_string();
        return (n != "none" && !n.is_empty()).then_some(Context { name: n, source: Source::Env });
    }
    if let Some((path, n)) = find_file(cwd) {
        return (n != "none").then_some(Context { name: n, source: Source::File(path) });
    }
    device(cache).map(|n| Context { name: n, source: Source::Device })
}

/// What a context does: its view, and the ids it lets through.
pub struct Active {
    pub ctx: Context,
    pub view: View,
    ids: HashSet<String>,
}

impl Active {
    pub fn load(store: &Store, ctx: Context, today: NaiveDate) -> Result<Active> {
        let view = views::find(store, &ctx.name)?.ok_or_else(|| {
            let hint = views::closest(store, &ctx.name).map(|c| format!(" · did you mean @{c}?")).unwrap_or_default();
            not_found(format!("context @{} names no view{hint} · thc context none", ctx.name))
        })?;
        let ids = store.query(&view.query, today, 1_000_000)?.into_iter().map(|n| n.id).collect();
        Ok(Active { ctx, view, ids })
    }

    pub fn allows(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// Keep what the context lets through; returns (kept, total before).
    pub fn filter<T>(&self, items: Vec<T>, id: impl Fn(&T) -> &str) -> (Vec<T>, usize) {
        let total = items.len();
        (items.into_iter().filter(|x| self.allows(id(x))).collect(), total)
    }

    /// The view's capture defaults: `#tags` and an optional `under:<page or id>`.
    pub fn capture_defaults(&self) -> CaptureDefaults {
        CaptureDefaults::parse(self.view.capture.as_deref().unwrap_or(""))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptureDefaults {
    pub tags: Vec<String>,
    pub under: Option<String>,
}

impl CaptureDefaults {
    /// `#work under:Acme` (also `--under Acme`).
    pub fn parse(s: &str) -> CaptureDefaults {
        let mut d = CaptureDefaults::default();
        let words: Vec<&str> = s.split_whitespace().collect();
        let mut i = 0;
        while i < words.len() {
            let w = words[i];
            if let Some(t) = w.strip_prefix('#').filter(|t| !t.is_empty()) {
                d.tags.push(t.to_lowercase());
            } else if let Some(u) = w.strip_prefix("under:") {
                d.under = Some(u.trim_matches('"').to_string());
            } else if w == "--under" && i + 1 < words.len() {
                d.under = Some(words[i + 1..].join(" ").trim_start_matches("¶ ").to_string());
                break;
            }
            i += 1;
        }
        d
    }

    pub fn is_empty(&self) -> bool {
        self.tags.is_empty() && self.under.is_none()
    }

    /// Apply to typed capture text: add the default tags unless the text names them (`#work`) or
    /// opts out (`-#work`, which is removed from the text). Returns (text, tags added).
    pub fn apply(&self, text: &str) -> (String, Vec<String>) {
        let words: Vec<&str> = text.split_whitespace().collect();
        let skip: HashSet<String> = words.iter().filter_map(|w| w.strip_prefix("-#")).map(|t| t.to_lowercase()).collect();
        let has: HashSet<String> = words.iter().filter_map(|w| w.strip_prefix('#')).map(|t| t.to_lowercase()).collect();
        let kept: Vec<&str> = words.into_iter().filter(|w| !w.starts_with("-#")).collect();
        let mut out = kept.join(" ");
        let mut added = Vec::new();
        for t in &self.tags {
            if !skip.contains(t) && !has.contains(t) {
                out.push_str(&format!(" #{t}"));
                added.push(t.clone());
            }
        }
        (out, added)
    }
}
