//! Pure presentation updates. Fields exposes only presentation values, so this
//! reducer cannot reach a Vault, terminal, channel, clock, or filesystem.
//! This is the first P3 migration; legacy input/persistence paths remain outside it.
use crate::{
    app::{Toast, ToastKind},
    doc::{Doc, Pos, Target},
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
    /// An editing or motion command at the caret. `seed` names the lines it creates; `widths`
    /// is the text column per depth (the layout motion moves over).
    Edit { cmd: caretline::Command, at: Instant, seed: String, widths: Widths },
    /// Text typed at the caret (over the selection).
    Type { text: String, at: Instant, seed: String },
}

/// The text column's width for each depth, as laid out when the message was made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Widths(pub Vec<usize>);

impl Widths {
    pub fn of(&self, depth: usize) -> usize {
        self.0.get(depth).or(self.0.last()).copied().unwrap_or(1)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    WritePageIds { visible: bool },
    WriteClipboard { text: String, notice: String },
    /// Save the document (`all`: the caret's line too).
    Save { all: bool },
    /// Re-read lines an undo or redo brought back: they may have changed elsewhere meanwhile.
    Patch,
}

pub(crate) struct DocumentFields<'a> {
    pub identity: DocumentIdentity,
    /// The document: its text and undo are model data, edited only through messages here.
    pub doc: &'a mut Doc,
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
            let scroll = &mut document.doc.scroll;
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
        Msg::Edit { cmd, at, seed, widths } => {
            let Some(document) = state.document else { return vec![] };
            let doc = document.doc;
            doc.stamp(at, &seed);
            let out = doc.apply(cmd, &|l| widths.of(l.depth));
            doc.unstamp();
            match out {
                caretline::Outcome::Done => {}
                caretline::Outcome::Nothing(why) => *state.toast = Some(Toast { kind: ToastKind::Info, parts: vec![(why.into(), Token::Muted)], at }),
                caretline::Outcome::Completed => return vec![Effect::Save { all: true }],
                caretline::Outcome::Restored => return vec![Effect::Patch],
            }
        }
        Msg::Type { text, at, seed } => {
            let Some(document) = state.document else { return vec![] };
            document.doc.stamp(at, &seed);
            document.doc.insert(&text);
            document.doc.unstamp();
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
    fn day() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap()
    }
    fn doc() -> Doc {
        Doc::new(Target::Journal { date: day() }, None, &[], day())
    }
    fn apply(scroll: &mut usize, doc: &mut Doc, toast: &mut Option<Toast>, msg: Msg) -> Vec<Effect> {
        update(Fields { page_ids: None, cursor: 4, scroll, document: Some(DocumentFields { identity: identity(), doc }), toast }, msg)
    }

    #[test]
    fn document_follow_replays_and_rejects_another_vault_or_revision() {
        let mut left = (0, doc(), None);
        let mut right = (0, doc(), None);
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
        assert_eq!((left.0, left.1.scroll), (right.0, right.1.scroll));
        assert_eq!(left.1.scroll, 92);
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
            assert_eq!(left.1.scroll, 92);
        }
    }

    /// Editing is a pure function of the messages: the same messages on the same document give
    /// the same text, line ids, caret and undo steps, whenever they're replayed.
    #[test]
    fn edits_replay_exactly() {
        use caretline::{Command as C, Motion};
        let t0 = Instant::now();
        let widths = Widths(vec![60, 56, 52]);
        let ms = |n: u64| t0 + std::time::Duration::from_millis(n);
        let msgs = vec![
            Msg::Type { text: "- Plan the offsite".into(), at: ms(0), seed: "a".into() },
            Msg::Edit { cmd: C::Newline, at: ms(100), seed: "b".into(), widths: widths.clone() },
            Msg::Type { text: "book".into(), at: ms(200), seed: "c".into() },
            Msg::Type { text: " the venue".into(), at: ms(300), seed: "d".into() },
            Msg::Edit { cmd: C::Newline, at: ms(5_000), seed: "e".into(), widths: widths.clone() },
            Msg::Edit { cmd: C::Indent, at: ms(5_100), seed: "f".into(), widths: widths.clone() },
            Msg::Type { text: "call them".into(), at: ms(5_200), seed: "g".into() },
            Msg::Edit { cmd: C::Move { motion: Motion::Up, select: true }, at: ms(5_300), seed: "h".into(), widths: widths.clone() },
            Msg::Edit { cmd: C::Undo, at: ms(5_400), seed: "i".into(), widths: widths.clone() },
        ];
        let run = || {
            let (mut scroll, mut d, mut toast) = (0, doc(), None);
            d.caret_to_end(true);
            // The first line is the host's (a fresh day's line): name it the same in both runs.
            d.lines_mut()[0].block.id = "firstline000".into();
            for m in msgs.clone() {
                apply(&mut scroll, &mut d, &mut toast, m);
            }
            let lines: Vec<(String, usize, String)> = d.lines().iter().map(|l| (l.id.clone(), l.depth, l.text.clone())).collect();
            (lines, d.view.caret, d.view.anchor, d.undo_depth())
        };
        let (a, b) = (run(), run());
        assert_eq!(a, b);
        let texts: Vec<(usize, &str)> = a.0.iter().map(|l| (l.1, l.2.as_str())).collect();
        assert_eq!(texts, [(0, "Plan the offsite"), (0, "book the venue"), (1, "")], "⌃Z undid the typing run, not the indent");
        let ids: Vec<&String> = a.0.iter().map(|l| &l.0).collect();
        assert!(ids.contains(&&thc_core::id::from_key("edit:b:0")), "a new line id comes from its message: {a:?}");
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
        let mut doc_scroll = doc();
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
