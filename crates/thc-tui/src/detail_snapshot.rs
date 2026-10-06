//! Frozen store values for detail, review and compare panes. IO is confined to capture.
use crate::app::{App, Overlay, Row, View};
use chrono::{Datelike, NaiveDate};
use std::{collections::HashMap, path::PathBuf};
use thc_core::{
    model::{Alert, HistoryEntry, Node},
    store::Store,
};

#[derive(Default)]
pub(crate) struct TextData {
    pub nodes: HashMap<String, Node>,
    shorts: HashMap<String, String>,
    rendered: HashMap<String, String>,
}

impl TextData {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }
    pub fn short(&self, id: &str) -> String {
        self.shorts.get(id).cloned().unwrap_or_else(|| id.chars().take(5).collect())
    }
    pub fn render_text(&self, text: &str) -> String {
        self.rendered.get(text).cloned().unwrap_or_else(|| text.to_owned())
    }
    fn text(&mut self, store: &Store, text: &str) {
        self.rendered.entry(text.to_owned()).or_insert_with(|| store.render_text(text));
    }
    fn add(&mut self, store: &Store, node: &Node) {
        self.shorts.insert(node.id.clone(), store.short(&node.id));
        self.text(store, &node.text);
        self.text(store, &node.label());
        self.nodes.insert(node.id.clone(), node.clone());
    }
    fn load(&mut self, store: &Store, id: &str) {
        if self.nodes.contains_key(id) {
            return;
        }
        if let Some(node) = store.node(id).ok().flatten() {
            self.add(store, &node);
        } else if id.len() == 12 && id.is_ascii() {
            self.shorts.insert(id.to_owned(), store.short(id));
        }
    }
}

pub(crate) struct NodeDetail {
    pub id: String,
    pub data: TextData,
    pub ancestors: Vec<Node>,
    pub blockers: Vec<Node>,
    pub tags: Vec<String>,
    pub props: serde_json::Map<String, serde_json::Value>,
    pub alerts: Vec<Alert>,
    pub children: Vec<Node>,
    pub backlinks: Vec<Node>,
    pub history: Vec<HistoryEntry>,
    pub history_words: HashMap<String, String>,
    pub reasons: HashMap<NaiveDate, Vec<thc_core::why::Reason>>,
}

#[derive(Default)]
pub(crate) struct CalendarData {
    pub days: std::collections::HashSet<NaiveDate>,
    pub entries: usize,
    pub tasks: usize,
    pub done: usize,
}

#[derive(Default)]
pub(crate) struct DetailSnapshot {
    key: Option<String>,
    pub node: Option<NodeDetail>,
    pub review: Option<thc_core::review::Item>,
    pub review_data: TextData,
    pub change_words: HashMap<String, String>,
    pub tx_words: HashMap<String, String>,
    pub inverse_count: usize,
    pub compare_data: TextData,
    pub compare_added: Option<HistoryEntry>,
    pub yours_is_current: bool,
    pub theirs_name: String,
    pub calendar: CalendarData,
}

fn node_detail(store: &Store, node: &Node, app: &App) -> NodeDetail {
    let mut data = TextData::default();
    data.add(store, node);
    let mut ancestors = vec![];
    let mut parent = node.parent.clone();
    while let Some(n) = parent.and_then(|id| store.node(&id).ok().flatten()) {
        parent = n.parent.clone();
        data.add(store, &n);
        ancestors.push(n);
        if ancestors.len() > 64 {
            break;
        }
    }
    let blockers = store.open_blockers(&node.id).unwrap_or_default();
    let children = store.children(&node.id).unwrap_or_default();
    let backlinks = store.backlinks(&node.id).unwrap_or_default();
    for node in blockers.iter().chain(&children).chain(&backlinks) {
        data.add(store, node);
    }
    let history = store.history(&node.id, 5).unwrap_or_default();
    let history_words =
        history.iter().map(|e| (e.eid.clone(), crate::app::tx_detail(store, std::slice::from_ref(e), app.theme.glyphs()))).collect();
    let mut reasons = HashMap::new();
    if app.view == View::Today {
        let days = std::iter::once(app.today).chain(
            [node.scheduled.as_deref(), node.due.as_deref()]
                .into_iter()
                .flatten()
                .filter_map(|date| thc_core::dates::DateVal::from_stored(date).map(|d| d.date())),
        );
        for day in days {
            reasons.insert(day, thc_core::why::reasons(store, node, day, app.today));
        }
    }
    NodeDetail {
        id: node.id.clone(),
        data,
        ancestors,
        blockers,
        children,
        backlinks,
        history,
        history_words,
        reasons,
        tags: store.tags_of(&node.id).unwrap_or_default(),
        props: store.props_of(&node.id).unwrap_or_default(),
        alerts: store.alerts_of(&node.id).unwrap_or_default(),
    }
}

pub(crate) fn capture(app: &mut App, revision: &str) {
    let pane = crate::ui::split_width(app, app.screen_width).is_some();
    let selected = app.selected_node();
    let source = app.store_of_selected();
    let selected_tx = app.rows.get(app.cursor).and_then(|row| match row {
        Row::Tx { tx, .. } => Some(tx),
        _ => None,
    });
    let compare = match &app.overlay {
        Some(Overlay::Compare { detail }) => Some(detail),
        _ => None,
    };
    let source_path: &PathBuf =
        app.row_from(app.cursor).and_then(|i| app.others.get(i)).map_or(&app.vault.paths.vault, |o| &o.vault.paths.vault);
    let source_revision = app.derived.data.rows.revision(source_path).unwrap_or_default();
    let key = format!(
        "{:?}",
        (
            &app.vault.paths.vault,
            revision,
            source_path,
            source_revision,
            pane.then(|| selected.map(|n| &n.id)),
            pane.then_some(selected_tx),
            (
                app.view,
                app.today,
                app.journal_date,
                pane,
                app.review_lane,
                &app.log_node,
                compare,
                app.theme.glyphs().arrow,
                compare.and_then(|_| app.doc.as_ref().map(|d| d.revision()))
            )
        )
    );
    if app.derived.data.detail.key.as_ref() == Some(&key) {
        return;
    }
    let mut out = DetailSnapshot { key: Some(key), ..Default::default() };
    if pane {
        if let Some(node) = selected {
            out.node = Some(node_detail(source, node, app));
        }
        if let Some(Row::Tx { tx, entries }) = app.rows.get(app.cursor) {
            let store = &app.vault.store;
            out.review = thc_core::review::item(store, tx, 0).ok();
            if let Some(item) = &out.review {
                for change in &item.changes {
                    out.review_data.load(store, &change.node);
                    out.change_words.insert(change.node.clone(), crate::app::change_words(store, change, app.theme.glyphs(), app.today));
                    for (field, old, new) in crate::app::review_field_lines(change, app.theme.glyphs(), app.today) {
                        if field == "text" {
                            out.review_data.text(store, &new);
                            if let Some(old) = old {
                                out.review_data.text(store, &old);
                            }
                        }
                    }
                }
            }
            for entry in entries {
                out.review_data.load(store, &entry.entity);
                if let Some(parent) = entry.body.get("parent").and_then(|v| v.as_str()) {
                    out.review_data.load(store, parent);
                }
                if let Some(text) = entry.body.get("text").and_then(|v| v.as_str()) {
                    out.review_data.text(store, text);
                }
                out.tx_words.insert(entry.eid.clone(), crate::app::tx_detail(store, std::slice::from_ref(entry), app.theme.glyphs()));
            }
            out.inverse_count = store.tx_inverse(tx).map(|v| v.len()).unwrap_or(0);
        }
    }
    if app.view == View::Journal {
        let store = &app.vault.store;
        if matches!(app.doc.as_ref().map(|d| &d.target), Some(crate::doc::Target::Journal { date }) if *date == app.journal_date) {
            out.calendar.days = app.derived.data.document.populated_days.clone();
        } else {
            let first = app.journal_date.with_day(1).unwrap();
            let start = first.min(app.journal_date - chrono::Duration::days(3));
            let end = first
                .iter_days()
                .take_while(|day| day.month() == first.month())
                .last()
                .unwrap()
                .max(app.journal_date + chrono::Duration::days(3));
            for day in start.iter_days().take_while(|day| *day <= end) {
                if store.journal_entries(&day.format("%Y-%m-%d").to_string()).is_ok_and(|v| !v.is_empty()) {
                    out.calendar.days.insert(day);
                }
            }
        }
        let entries = store.journal_entries(&app.journal_date.format("%Y-%m-%d").to_string()).unwrap_or_default();
        out.calendar.entries = entries.len();
        out.calendar.tasks = entries.iter().filter(|n| n.status.is_some()).count();
        out.calendar.done = entries.iter().filter(|n| n.status.as_deref() == Some("done")).count();
    }
    if let Some(detail) = compare {
        out.yours_is_current = app.yours_is_current(detail);
        out.theirs_name = app.theirs_name(detail);
        let store = &app.vault.store;
        out.compare_data.load(store, &detail.node);
        if let Some(parent) = out.compare_data.node(&detail.node).and_then(|n| n.parent.clone()) {
            out.compare_data.load(store, &parent);
        }
        for version in detail.current.iter().chain(&detail.other) {
            out.compare_data.text(store, &version.text);
            if detail.kind != "text" {
                out.compare_data.load(store, &version.text);
            }
        }
        if let Some(base) = &detail.base {
            out.compare_data.text(store, base);
        }
        out.compare_added =
            store.history_where("entity = ?1 AND op = 'node.create' LIMIT 1", &[&detail.node]).ok().and_then(|v| v.into_iter().next());
    }
    app.derived.data.detail = out;
}
