//! Immutable store-derived render inputs. Capture runs in the coordinator; these
//! values have no database, filesystem, clock or runtime handles.
use crate::{app::App, doc::Target};
use chrono::{Datelike, NaiveDate};
use std::{collections::HashSet, path::PathBuf};

#[derive(Default)]
pub(crate) struct DataSnapshot {
    key: Option<DocumentKey>,
    pub input: crate::input_snapshot::InputSnapshot,
    pub clock: crate::clock_snapshot::ClockSnapshot,
    pub detail: crate::detail_snapshot::DetailSnapshot,
    pub document: DocumentChrome,
    pub rows: crate::row_snapshot::RowSnapshot,
    pub overlay: crate::overlay_snapshot::OverlaySnapshot,
    pub presentation: crate::presentation_snapshot::PresentationSnapshot,
    pub bindings: crate::binding_snapshot::BindingSnapshot,
}

#[derive(PartialEq, Eq)]
struct DocumentKey {
    vault: PathBuf,
    revision: String,
    target: Target,
}

#[derive(Default)]
pub(crate) struct DocumentChrome {
    pub populated_days: HashSet<NaiveDate>,
    pub issue_status: Option<String>,
    pub issue_meta: Vec<String>,
    pub label: String,
    pub parent_label: Option<String>,
}

pub(crate) fn capture(app: &mut App, revision: &str) {
    let Some(doc) = &app.doc else {
        app.derived.data.key = None;
        app.derived.data.document = DocumentChrome::default();
        return;
    };
    let key = DocumentKey { vault: app.vault.paths.vault.clone(), revision: revision.to_owned(), target: doc.target.clone() };
    if app.derived.data.key.as_ref() == Some(&key) {
        return;
    }
    let store = &app.vault.store;
    let mut document = DocumentChrome::default();
    match &doc.target {
        Target::Journal { date } => {
            // One capture serves both the seven-day strip and the month calendar.
            let first = date.with_day(1).unwrap();
            let next =
                if first.month() == 12 { NaiveDate::from_ymd_opt(first.year() + 1, 1, 1) } else { NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1) }
                    .unwrap();
            let start = first.min(*date - chrono::Duration::days(3));
            let end = (next - chrono::Duration::days(1)).max(*date + chrono::Duration::days(3));
            for day in start.iter_days().take_while(|d| *d <= end) {
                let has = store
                    .journal_node(&day.format("%Y-%m-%d").to_string())
                    .ok()
                    .flatten()
                    .is_some_and(|id| store.children(&id).is_ok_and(|children| !children.is_empty()));
                if has {
                    document.populated_days.insert(day);
                }
            }
        }
        Target::Page { id, .. } => {
            if let Some(node) = store.node(id).ok().flatten() {
                document.label = store.render_text(&node.label());
                document.parent_label = node.parent.as_ref().map(|id| app.node_label(id));
                document.issue_status = node.status.clone();
                if node.status.is_some() {
                    if let Some(owner) = store.get_field(id, "owner").ok().and_then(|v| v.as_str().map(str::to_owned)).filter(|s| !s.is_empty()) {
                        document.issue_meta.push(format!("◆ {owner}"));
                    }
                    if let Some(priority) = node.priority {
                        document.issue_meta.push(format!("!{priority}"));
                    }
                    if let Some(opened) = chrono::DateTime::from_timestamp_millis(node.created_ms)
                        .map(|d| d.with_timezone(&chrono::Local).format("%b %-d").to_string().to_lowercase())
                    {
                        document.issue_meta.push(format!("opened {opened}"));
                    }
                }
            }
        }
    }
    app.derived.data.key = Some(key);
    app.derived.data.document = document;
}

#[cfg(test)]
mod tests {
    use super::*;
    use thc_core::{builder::TxBuilder, capture};

    #[test]
    fn document_chrome_is_stable_until_a_new_store_revision_is_captured() {
        let (_scratch, mut vault) = crate::fuzz::scratch("document-data-snapshot");
        let today = thc_core::dates::today();
        let (_, (page, issue)) = vault
            .transact(|store| {
                let mut b = TxBuilder::new(store, today);
                let page = b.create_page("Project", &[])?;
                let issue = b.create_from_capture(Some(page.clone()), &capture::parse("[ ] Ship it !high", today)?, None)?;
                Ok((b.finish(), (page, issue)))
            })
            .unwrap();
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut app = App::new(vault).unwrap();
        app.doc = Some(crate::doc::Doc::new(Target::Page { id: issue.clone(), title: "Ship it".into() }, Some(issue), &[], today));
        let rev = app.vault.store.max_okey().unwrap().unwrap();
        capture(&mut app, &rev);
        assert_eq!(app.derived.data.document.parent_label.as_deref(), Some("Project"));
        assert_eq!(app.derived.data.document.issue_status.as_deref(), Some("todo"));
        assert!(app.derived.data.document.issue_meta.contains(&"!high".into()));
        let (_, ()) = app
            .vault
            .transact(|store| {
                let mut b = TxBuilder::new(store, today);
                b.set_props(&page, &[("title".into(), "Renamed".into())])?;
                Ok((b.finish(), ()))
            })
            .unwrap();
        // Frozen render inputs remain usable independently of later database updates.
        assert_eq!(app.derived.data.document.parent_label.as_deref(), Some("Project"));
        capture(&mut app, &rev);
        assert_eq!(app.derived.data.document.parent_label.as_deref(), Some("Project"));
        let rev = app.vault.store.max_okey().unwrap().unwrap();
        capture(&mut app, &rev);
        assert!(app.derived.data.key.as_ref().is_some_and(|key| key.revision == rev));
        assert_eq!(app.derived.data.document.parent_label.as_deref(), Some("Renamed"));
    }
}
