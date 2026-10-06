//! Pure presentation updates. Fields exposes only presentation values, so this
//! reducer cannot reach a Vault, terminal, channel, clock, or filesystem.
//! This is the first P3 migration; legacy input/persistence paths remain outside it.
use crate::{
    app::{Toast, ToastKind},
    doc::{Pos, Target},
    theme::Token,
};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DocumentIdentity {
    pub vault: std::path::PathBuf,
    pub target: Target,
    pub revision: u64,
    pub caret: Pos,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Viewport {
    Document { identity: DocumentIdentity, rows: usize, caret: Option<usize>, height: usize, free: bool, typewriter: bool },
    List { cursor: usize, scroll: usize, height: usize, previous_section: bool, heights: Vec<usize> },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Msg {
    ViewportPrepared(Viewport),
    TogglePageIds { at: Instant },
    PageIdsPersisted { result: Result<(), String> },
    Copy { text: String, notice: String },
    ClipboardResult { result: Result<(), String>, notice: String, at: Instant },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    WritePageIds { visible: bool },
    WriteClipboard { text: String, notice: String },
}

pub(crate) struct DocumentFields<'a> {
    pub identity: DocumentIdentity,
    pub scroll: &'a mut usize,
}
pub(crate) struct Fields<'a> {
    pub page_ids: Option<&'a mut bool>,
    pub cursor: usize,
    pub scroll: &'a mut usize,
    pub document: Option<DocumentFields<'a>>,
    pub toast: &'a mut Option<Toast>,
}
/// The initial list position also tells derivation which row heights are needed.
/// The complete follow policy runs in update; preparation never assigns scroll.
pub(crate) fn list_start(mut scroll: usize, cursor: usize, height: usize, previous_section: bool) -> usize {
    if cursor < scroll {
        scroll = cursor;
    }
    if cursor >= scroll.saturating_add(height) {
        scroll = cursor.saturating_add(1).saturating_sub(height);
    }
    if scroll > 0 && scroll == cursor && previous_section {
        scroll -= 1;
    }
    scroll
}
pub(crate) fn update(state: Fields<'_>, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::ViewportPrepared(Viewport::Document { identity, rows, caret, height, free, typewriter }) => {
            let Some(document) = state.document.filter(|d| d.identity == identity) else { return vec![] };
            let scroll = document.scroll;
            if free {
                *scroll = (*scroll).min(rows.saturating_sub(1));
            } else if let Some(caret) = caret {
                if typewriter {
                    *scroll = caret.saturating_sub(height * 45 / 100);
                } else {
                    let context = 2.min(height.saturating_sub(1) / 2);
                    if caret < scroll.saturating_add(context) {
                        *scroll = caret.saturating_sub(context);
                    } else if caret.saturating_add(context) >= scroll.saturating_add(height) {
                        *scroll = caret
                            .saturating_add(context + 1)
                            .saturating_sub(height)
                            .min(rows.saturating_sub(height))
                            .max(caret.saturating_add(1).saturating_sub(height));
                    }
                }
            }
        }
        Msg::ViewportPrepared(Viewport::List { cursor, scroll, height, previous_section, heights }) => {
            if state.document.is_some() || state.cursor != cursor || *state.scroll != scroll {
                return vec![];
            }
            let mut start = list_start(scroll, cursor, height, previous_section);
            let mut total: usize = heights.iter().sum();
            for h in heights {
                if start >= cursor || total <= height {
                    break;
                }
                total = total.saturating_sub(h);
                start += 1;
            }
            *state.scroll = start;
        }
        Msg::TogglePageIds { at } => {
            let Some(page_ids) = state.page_ids else { return vec![] };
            *page_ids = !*page_ids;
            let text = if *page_ids { "IDs shown · . to hide" } else { "IDs hidden · . to show" };
            *state.toast = Some(Toast { kind: ToastKind::Info, parts: vec![(text.into(), Token::Muted)], at });
            return vec![Effect::WritePageIds { visible: *page_ids }];
        }
        // This preference historically ignored cache failures: the session choice
        // still takes effect. The explicit result can be recorded/replayed.
        Msg::PageIdsPersisted { result: _ } => {}
        Msg::Copy { text, notice } => return vec![Effect::WriteClipboard { text, notice }],
        Msg::ClipboardResult { result, notice, at } => {
            let (kind, token, text) = match result {
                Ok(()) => (ToastKind::Info, Token::Muted, notice),
                Err(error) => (ToastKind::Error, Token::Overdue, error),
            };
            *state.toast = Some(Toast { kind, parts: vec![(text, token)], at });
        }
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> DocumentIdentity {
        DocumentIdentity {
            vault: "/scratch/one".into(),
            target: Target::Journal { date: chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap() },
            revision: 7,
            caret: Pos { line: 110, byte: 0 },
        }
    }
    fn apply(scroll: &mut usize, doc_scroll: &mut usize, toast: &mut Option<Toast>, msg: Msg) -> Vec<Effect> {
        update(
            Fields {
                page_ids: None,
                cursor: 4,
                scroll,
                document: Some(DocumentFields { identity: identity(), scroll: doc_scroll }),
                toast,
            },
            msg,
        )
    }

    #[test]
    fn document_follow_replays_and_rejects_another_vault_or_revision() {
        let mut left = (0, 0, None);
        let mut right = (0, 0, None);
        let message = Msg::ViewportPrepared(Viewport::Document {
            identity: identity(),
            rows: 200,
            caret: Some(110),
            height: 40,
            free: false,
            typewriter: true,
        });
        assert!(apply(&mut left.0, &mut left.1, &mut left.2, message.clone()).is_empty());
        assert!(apply(&mut right.0, &mut right.1, &mut right.2, message).is_empty());
        assert_eq!((left.0, left.1), (right.0, right.1));
        assert_eq!(left.1, 92);
        for other in [DocumentIdentity { vault: "/scratch/two".into(), ..identity() }, DocumentIdentity { revision: 8, ..identity() }] {
            let message = Msg::ViewportPrepared(Viewport::Document {
                identity: other,
                rows: 200,
                caret: Some(190),
                height: 40,
                free: false,
                typewriter: false,
            });
            apply(&mut left.0, &mut left.1, &mut left.2, message);
            assert_eq!(left.1, 92);
        }
    }

    #[test]
    fn wrapped_list_rows_follow_and_stale_metrics_do_not_move_scroll() {
        let mut scroll = 0;
        let mut toast = None;
        let message = Msg::ViewportPrepared(Viewport::List {
            cursor: 4,
            scroll: 0,
            height: 5,
            previous_section: false,
            heights: vec![1, 1, 4, 1, 2],
        });
        update(Fields { page_ids: None, cursor: 4, scroll: &mut scroll, document: None, toast: &mut toast }, message.clone());
        assert_eq!(scroll, 3);
        update(Fields { page_ids: None, cursor: 4, scroll: &mut scroll, document: None, toast: &mut toast }, message);
        assert_eq!(scroll, 3);
    }

    #[test]
    fn page_ids_change_before_persistence_and_a_cache_failure_keeps_the_choice() {
        let mut page_ids = false;
        let mut scroll = 0;
        let mut toast = None;
        let at = Instant::now();
        let effects = update(
            Fields { page_ids: Some(&mut page_ids), cursor: 0, scroll: &mut scroll, document: None, toast: &mut toast },
            Msg::TogglePageIds { at },
        );
        assert!(page_ids);
        assert_eq!(effects, vec![Effect::WritePageIds { visible: true }]);
        update(
            Fields { page_ids: Some(&mut page_ids), cursor: 0, scroll: &mut scroll, document: None, toast: &mut toast },
            Msg::PageIdsPersisted { result: Err("cache unavailable".into()) },
        );
        assert!(page_ids);
        assert_eq!(toast.unwrap().parts, vec![("IDs shown · . to hide".into(), Token::Muted)]);
    }

    #[test]
    fn clipboard_emits_one_value_effect_and_waits_for_an_explicit_result() {
        let mut scroll = 0;
        let mut doc_scroll = 0;
        let mut toast = None;
        let effects = apply(&mut scroll, &mut doc_scroll, &mut toast, Msg::Copy { text: "bé🙂".into(), notice: "copied 3 chars".into() });
        assert_eq!(effects, vec![Effect::WriteClipboard { text: "bé🙂".into(), notice: "copied 3 chars".into() }]);
        assert!(toast.is_none());
        let at = Instant::now(); // supplied replay input; update itself never samples time
        apply(&mut scroll, &mut doc_scroll, &mut toast, Msg::ClipboardResult { result: Ok(()), notice: "copied 3 chars".into(), at });
        let success = toast.take().unwrap();
        assert_eq!((success.kind, success.parts, success.at), (ToastKind::Info, vec![("copied 3 chars".into(), Token::Muted)], at));
        apply(
            &mut scroll,
            &mut doc_scroll,
            &mut toast,
            Msg::ClipboardResult { result: Err("clipboard refused".into()), notice: "unused".into(), at },
        );
        assert_eq!(toast.unwrap().kind, ToastKind::Error);
    }
}
