//! Immutable, vault-qualified metadata for list rows. Runtime capture is cached by
//! store revision; rendering consumes values without opening a Store.
use crate::app::{App, Row, View};
use chrono::NaiveDate;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use thc_core::{model::Node, store::Store};

#[derive(Default)]
pub(crate) struct RowSnapshot {
    stores: HashMap<PathBuf, StoreRows>,
}

#[derive(Default)]
struct StoreRows {
    revision: String,
    today: Option<NaiveDate>,
    today_view: bool,
    page_counts: bool,
    shorts: HashMap<String, String>,
    nodes: HashMap<String, NodeData>,
}

pub(crate) struct NodeData {
    pub node: Node,
    pub label: String,
    pub short: String,
    pub text: String,
    pub parent: Option<Node>,
    pub parent_text: String,
    pub root: Option<Node>,
    pub has_alert: bool,
    pub alert_only: Option<String>,
    pub children: usize,
    pub open: usize,
    pub backlinks: usize,
}

impl RowSnapshot {
    pub fn timestamps(&self) -> impl Iterator<Item = i64> + '_ {
        self.stores.values().flat_map(|s| s.nodes.values().map(|n| n.node.created_ms))
    }

    pub fn revision(&self, vault: &Path) -> Option<&str> {
        self.stores.get(vault).map(|s| s.revision.as_str())
    }

    pub fn short(&self, vault: &Path, id: &str) -> String {
        self.stores
            .get(vault)
            .and_then(|store| store.nodes.get(id).map(|n| &n.short).or_else(|| store.shorts.get(id)))
            .cloned()
            .unwrap_or_else(|| id.chars().take(5).collect())
    }

    pub fn node(&self, vault: &Path, id: &str) -> Option<&NodeData> {
        self.stores.get(vault)?.nodes.get(id)
    }
}

fn capture_node(store: &Store, n: &Node, today: NaiveDate, today_view: bool, page_counts: bool) -> NodeData {
    let parent = n.parent.as_deref().and_then(|id| store.node(id).ok().flatten());
    let parent_text = parent.as_ref().map(|p| store.render_text(&p.label())).unwrap_or_default();
    let mut root = parent.clone();
    let mut guard = 0;
    while let Some(p) = root.as_ref().and_then(|p| p.parent.as_deref()).and_then(|id| store.node(id).ok().flatten()) {
        root = Some(p);
        guard += 1;
        if guard > 64 {
            break;
        }
    }
    let reasons = if today_view { thc_core::why::reasons(store, n, today, today) } else { vec![] };
    let other = reasons.iter().any(|r| matches!(r.code, "overdue" | "due-today" | "scheduled" | "alert-fired"));
    let alert_only = (!other).then(|| reasons.into_iter().find(|r| r.code == "alert").and_then(|r| r.detail)).flatten();
    let page = page_counts && n.parent.is_none() && n.title.is_some();
    NodeData {
        node: n.clone(),
        label: store.render_text(&n.label()),
        short: store.short(&n.id),
        text: store.render_text(&n.text),
        parent,
        parent_text,
        root,
        has_alert: store.alerts_of(&n.id).is_ok_and(|alerts| !alerts.is_empty()),
        alert_only,
        children: if page { store.children(&n.id).map(|v| v.len()).unwrap_or(0) } else { 0 },
        open: if page { store.query(&format!("status:open under:{}", n.id), today, 1000).map(|v| v.len()).unwrap_or(0) } else { 0 },
        backlinks: if page { store.backlinks(&n.id).map(|v| v.len()).unwrap_or(0) } else { 0 },
    }
}

pub(crate) fn capture(app: &mut App, revision: &str) {
    let page_counts = app.doc.is_none() && matches!(app.view, View::Pages | View::Search);
    let mut stores = std::mem::take(&mut app.derived.data.rows.stores);
    let mut load = |path: &Path, store: &Store, revision: &str, nodes: Vec<&Node>| {
        let data = stores.entry(path.to_owned()).or_default();
        if data.revision != revision
            || data.today != Some(app.today)
            || data.today_view != (app.view == View::Today)
            || data.page_counts != page_counts
        {
            data.revision = revision.to_owned();
            data.today = Some(app.today);
            data.nodes.clear();
            data.shorts.clear();
            data.today_view = app.view == View::Today;
            data.page_counts = page_counts;
        }
        for node in nodes {
            data.nodes.entry(node.id.clone()).or_insert_with(|| capture_node(store, node, app.today, app.view == View::Today, page_counts));
        }
    };
    let mut nodes: Vec<&Node> = app
        .rows
        .iter()
        .enumerate()
        .filter(|_| app.doc.is_none())
        .filter(|(i, _)| app.row_from(*i).is_none())
        .filter_map(|(_, r)| r.node())
        .collect();
    if app.view == View::Pages {
        if let Some(preview) = &app.derived.preview {
            if preview.vault == app.vault.paths.vault {
                nodes.extend(preview.rows.iter().filter_map(Row::node));
            }
        }
    }
    load(&app.vault.paths.vault, &app.vault.store, revision, nodes);
    for (i, other) in app.others.iter().enumerate() {
        let rev = other.vault.store.max_okey().ok().flatten().unwrap_or_default();
        let nodes = app
            .rows
            .iter()
            .enumerate()
            .filter(|_| app.doc.is_none())
            .filter(|(row, _)| app.row_from(*row) == Some(i))
            .filter_map(|(_, r)| r.node())
            .collect();
        load(&other.vault.paths.vault, &other.vault.store, &rev, nodes);
    }
    if let Some(main) = stores.get_mut(&app.vault.paths.vault) {
        let mut ids: Vec<&str> = app.page_open.iter().chain(app.log_node.iter()).map(String::as_str).collect();
        if let Some(doc) = &app.doc {
            ids.extend(doc.root.as_deref());
            if let crate::doc::Target::Page { id, .. } = &doc.target {
                ids.push(id);
            }
        }
        ids.extend(app.conflicts.iter().map(|(_, node, _, _)| node.as_str()));
        for target in &app.recent_moves {
            if let crate::app::MoveTarget::Under(id) = target {
                ids.push(id);
            }
        }
        match &app.overlay {
            Some(crate::app::Overlay::Move { node, .. }) => ids.push(node),
            Some(crate::app::Overlay::Compare { detail }) => {
                ids.push(&detail.node);
                if detail.kind == "move" || detail.kind == "rehomed" {
                    ids.extend(detail.current.as_ref().map(|v| v.text.as_str()));
                    ids.extend(detail.other.as_ref().map(|v| v.text.as_str()));
                }
            }
            _ => {}
        }
        for row in &app.rows {
            if let Row::Tx { entries, .. } = row {
                ids.extend(entries.iter().map(|e| e.entity.as_str()));
            }
        }
        for id in ids {
            if !main.nodes.contains_key(id) {
                if let Some(node) = app.vault.store.node(id).ok().flatten() {
                    main.nodes
                        .insert(id.to_owned(), capture_node(&app.vault.store, &node, app.today, app.view == View::Today, page_counts));
                } else if id.len() == 12 && id.is_ascii() {
                    main.shorts.entry(id.to_owned()).or_insert_with(|| app.vault.store.short(id));
                }
            }
        }
        for row in &app.rows {
            if let Row::Tag { id, .. } = row {
                main.shorts.entry(id.clone()).or_insert_with(|| app.vault.store.short(id));
            }
        }
    }
    app.derived.data.rows.stores = stores;
}
