//! Session-owned render caches. Runtime preparation performs their IO and drawing
//! reads their values immutably. Store-backed view reads still migrate separately.
//!
//! These are private render inputs, not serializable presentation state. Cache identities
//! include the vault: keyed note ids and relative attachment paths can occur in two vaults.

use std::{collections::{HashMap, HashSet}, path::PathBuf};
use crate::app::{App, Row, View};

pub(crate) struct PagePreview {
    pub vault: PathBuf,
    pub page: String,
    revision: String,
    collapsed: HashSet<String>,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug)]
pub(crate) struct Attachment {
    pub dimensions: Option<(u32, u32)>,
    pub label: String,
    pub missing: bool,
}

#[derive(PartialEq, Eq)]
struct AttachmentSource {
    vault: PathBuf,
    store_revision: String,
    target: crate::doc::Target,
    root: Option<String>,
    revision: u64,
}

const MISSING_RETRY: std::time::Duration = std::time::Duration::from_secs(3);

pub(crate) struct Derived {
    pub doc: Option<crate::doc_ui::PreparedDoc>,
    pub list: Option<crate::ui::PreparedList>,
    pub preview: Option<PagePreview>,
    pub attachments: HashMap<(PathBuf, String), Attachment>,
    attachment_source: Option<AttachmentSource>,
    attachment_paths: Vec<String>,
    attachment_probes: HashMap<(PathBuf, String), (std::time::Instant, String)>,
    pub data: crate::data_snapshot::DataSnapshot,
    pub inline_images: bool,
    pub caret_chip: Option<String>,
    chip_source: Option<(chrono::NaiveDate, String)>,
    pub now: std::time::Instant,
    pub clock: String,
    pub version: String,
    pub trace: bool,
    pub render_mode: bool,
    pub pinned_warning: Option<String>,
    pub mac: bool,
    pub command_keys: bool,
}

impl Derived {
    pub fn new() -> Self {
        Self {
            doc: None, list: None, preview: None, attachments: HashMap::new(),
            attachment_source: None,
            attachment_paths: vec![],
            attachment_probes: HashMap::new(),
            data: crate::data_snapshot::DataSnapshot::default(),
            inline_images: crate::images::proto().is_some(), caret_chip: None,
            chip_source: None, now: std::time::Instant::now(),
            clock: thc_core::dates::now_local().format("%H:%M").to_string(),
            version: std::env::var("THC_TUI_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into()),
            trace: std::env::var("THC_TUI_TRACE").is_ok_and(|v| v == "1" || v == "2"),
            render_mode: std::env::var("THC_TUI_RENDER").is_ok_and(|v| v == "1"),
            pinned_warning: thc_core::dates::pinned_warning(),
            mac: crate::keymap::detect_mac(), command_keys: false,
        }
    }

    pub fn attachment(&self, vault: &std::path::Path, path: &str) -> Option<&Attachment> {
        self.attachments.get(&(vault.to_owned(), path.to_owned()))
    }

    pub fn age(&self, since: std::time::Instant) -> std::time::Duration {
        self.now.saturating_duration_since(since)
    }
}

/// The runtime calls this before layout, so reading these caches during drawing does
/// not perform file or store IO.
pub(crate) fn prepare(app: &mut App) {
    app.derived.now = std::time::Instant::now();
    app.derived.command_keys = crate::keymap::detect_cmd_known(app);
    app.derived.data.overlay = crate::overlay_snapshot::capture(app);
    app.derived.data.presentation = crate::presentation_snapshot::capture(app);
    app.derived.data.bindings = crate::binding_snapshot::capture(app);
    app.derived.clock = thc_core::dates::now_local().format("%H:%M").to_string();
    let source = app.doc.as_ref().and_then(|d| d.lines().get(d.view.caret.line))
        .map(|l| (app.today, l.text.clone()));
    if source != app.derived.chip_source {
        app.derived.caret_chip = source.as_ref().and_then(|(today, text)| crate::doc_ui::chip(text, *today));
        app.derived.chip_source = source;
    }
    let store_revision = app.vault.store.max_okey().ok().flatten().unwrap_or_default();
    crate::data_snapshot::capture(app, &store_revision);
    prepare_attachments(app, &store_revision);
    prepare_preview(app);
    crate::row_snapshot::capture(app, &store_revision);
    crate::detail_snapshot::capture(app, &store_revision);
    crate::clock_snapshot::capture(app);
    crate::input_snapshot::capture(app);
}

fn prepare_preview(app: &mut App) {
    if app.view != View::Pages || app.page_open.is_some() || app.doc.is_some() { return }
    let Some(n) = app.selected_node().filter(|n| n.parent.is_none() && n.title.is_some()) else { return };
    let page = n.id.clone();
    let revision = app.vault.store.max_okey().ok().flatten().unwrap_or_default();
    if app.derived.preview.as_ref().is_some_and(|p| p.vault == app.vault.paths.vault && p.page == page && p.revision == revision && p.collapsed == app.collapsed) { return }
    let mut rows = app.outline_rows(&page).unwrap_or_default();
    rows.truncate(200);
    let label = n.label();
    let tagged = app.vault.store.nodes_where(
        "n.deleted=0 AND n.id IN (SELECT e.src FROM edges e JOIN nodes t ON t.id=e.dst WHERE e.rel='tag' AND t.title=?1 COLLATE NOCASE)", &[&label],
    ).unwrap_or_default();
    if !tagged.is_empty() {
        rows.push(Row::Blank);
        rows.push(Row::Section { title: format!("tagged #{} elsewhere", label.to_lowercase()), count: Some(tagged.len()), token: crate::theme::Token::Muted, note: None });
        rows.extend(tagged.into_iter().map(|node| Row::Node { node, depth: 0, outline: false, has_children: false, collapsed: false, child_count: 0, under_day: None }));
    }
    app.derived.preview = Some(PagePreview { vault: app.vault.paths.vault.clone(), page, revision, collapsed: app.collapsed.clone(), rows });
}

/// Scan paths only when content changes. Missing files retry on vault news or a timer,
/// including sync arrivals which do not carry a new note transaction.
fn prepare_attachments(app: &mut App, store_revision: &str) {
    let Some(doc) = &app.doc else {
        app.derived.attachment_source = None;
        app.derived.attachment_paths.clear();
        return;
    };
    let vault = &app.vault.paths.vault;
    let source = AttachmentSource {
        vault: vault.clone(),
        store_revision: store_revision.to_owned(),
        target: doc.target.clone(),
        root: doc.root.clone(),
        revision: doc.revision(),
    };
    if app.derived.attachment_source.as_ref() != Some(&source) {
        let paths: HashSet<String> = doc.lines().iter().filter_map(|line| crate::doc_ui::image_line(&line.text).map(|(_, path)| path)).collect();
        app.derived.attachment_paths = paths.into_iter().collect();
        app.derived.attachment_source = Some(source);
    }
    for path in &app.derived.attachment_paths {
        let key = (vault.clone(), path.clone());
        if app.derived.attachments.get(&key).is_some_and(|a| !a.missing) {
            continue;
        }
        if app.derived.attachment_probes.get(&key).is_some_and(|(at, rev)| !retry_due(app.derived.now, *at, store_revision, rev)) {
            continue;
        }
        let st = thc_core::attach::describe(vault, path, None);
        let missing = !st.abs.exists();
        let dimensions = st.w.zip(st.h);
        let mut label = dimensions.map(|(w, h)| format!(" · {w}×{h}")).unwrap_or_default();
        if !missing {
            label.push_str(&format!(" · {}", crate::doc_ui::human_bytes(st.bytes)));
        }
        app.derived.attachments.insert(key.clone(), Attachment { dimensions, label, missing });
        app.derived.attachment_probes.insert(key, (app.derived.now, store_revision.to_owned()));
    }
}

fn retry_due(now: std::time::Instant, at: std::time::Instant, revision: &str, probed_revision: &str) -> bool {
    revision != probed_revision || now.saturating_duration_since(at) >= MISSING_RETRY
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Line, Pos};
    use thc_core::outline::Kind;

    #[test]
    fn missing_attachments_wait_for_retry_or_vault_news_and_edits_rescan() {
        let (_scratch, vault) = crate::fuzz::scratch("attachment-retry");
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut app = App::new(vault).unwrap();
        app.set_view(View::Journal);
        let doc = app.doc.as_mut().unwrap();
        *doc.lines_mut() = vec![Line::new(0, Kind::Para, "![item](files/item.txt)")];
        doc.view.caret = Pos::default();
        let at = app.derived.now;
        prepare_attachments(&mut app, "one");
        let vault = app.vault.paths.vault.clone();
        assert!(app.derived.attachment(&vault, "files/item.txt").unwrap().missing);
        std::fs::create_dir_all(vault.join("files")).unwrap();
        std::fs::write(vault.join("files/item.txt"), b"abc").unwrap();
        // Hundreds of redraws/caret moves must neither rescan nor probe the missing file.
        let paths = app.derived.attachment_paths.as_ptr();
        for n in 0..200 {
            app.doc.as_mut().unwrap().view.caret.byte = n % 10;
            app.derived.now = at + std::time::Duration::from_millis(n as u64);
            prepare_attachments(&mut app, "one");
            assert_eq!(paths, app.derived.attachment_paths.as_ptr());
            assert!(app.derived.attachment(&vault, "files/item.txt").unwrap().missing);
        }
        app.derived.now = at + MISSING_RETRY;
        prepare_attachments(&mut app, "one");
        assert_eq!(app.derived.attachment(&vault, "files/item.txt").unwrap().label, " · 3 B");

        // An actual edit changes the path list, even inside one coalesced undo step.
        let doc = app.doc.as_mut().unwrap();
        doc.view.caret = Pos { line: 0, byte: doc.lines()[0].text.len() };
        doc.newline();
        doc.newline();
        doc.insert("![next](files/next.txt)");
        prepare_attachments(&mut app, "one");
        assert!(app.derived.attachment(&vault, "files/next.txt").unwrap().missing);
        std::fs::write(vault.join("files/next.txt"), b"new").unwrap();
        prepare_attachments(&mut app, "two");
        assert!(!app.derived.attachment(&vault, "files/next.txt").unwrap().missing);
    }
}
