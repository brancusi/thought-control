//! The editor pane (docs/design/panes.md): one component for every place a document is
//! edited, the main view and each sidebar panel. Each instance is a small `EditorState`
//! pointing at a shared document; the document (text, undo) and its views (caret, selection,
//! scroll, folds) are the editor engine's, never copied here.
//!
//! `EditorState` holds only what the engine doesn't: whether the pane writes or is parked, the
//! `[[` popup, the pointer's press, and the editing bookkeeping around the caret's line. It is
//! serializable: the main view's is part of `UiState` (its v1 fields, flattened at the top
//! level under their v1 names), so it replays, patches and restores like the rest of the state.

pub(crate) mod clicks;
pub(crate) mod update;
pub(crate) mod view;

use crate::editor::BlockPos;
use serde::{Deserialize, Serialize};

/// One editor pane's own state (see the module docs). The serde names are the v1 wire names
/// of the main view's fields.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditorState {
    /// Write (typing) or Navigate inside the document.
    #[serde(rename = "doc_write")]
    pub write: bool,
    /// Just arrived: Tab and ⇧Tab still change views.
    #[serde(rename = "doc_parked")]
    pub parked: bool,
    /// The caret's line when last looked at (leaving it saves).
    #[serde(rename = "doc_line_id")]
    pub line_id: Option<String>,
    /// Navigate's line selection start.
    #[serde(rename = "doc_vsel")]
    pub vsel: Option<usize>,
    /// Navigate's cursor in the footer rows.
    #[serde(rename = "doc_footer_cur")]
    pub footer_cur: Option<usize>,
    /// A remote change landed on the caret's line: Some(edited).
    #[serde(rename = "doc_announce")]
    pub announce: Option<bool>,
    /// The `[[` popup: open, and its selected row.
    pub link_open: bool,
    pub link_sel: Option<usize>,
    /// A drop just attached: (its line's id, the pasted text, the undo depth then).
    pub last_drop: Option<(String, String, usize)>,
    /// A new link close to an existing page: (line id, typed, existing, stub id, since ms).
    pub near_miss: Option<(String, String, String, Option<String>, u64)>,
    /// Where a drag that selects started (a press on text in this pane).
    #[serde(with = "crate::ui_state::pos_opt")]
    pub drag_from: Option<BlockPos>,
    /// Where the held pointer last dragged to (screen cell), and when (`now_ms`): held on the
    /// view's edge, the drag repeats on ticks and the view scrolls (Session::held_drag).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drag_at: Option<(u16, u16)>,
    #[serde(skip_serializing_if = "is_zero")]
    pub drag_ms: u64,
    /// A press on a link: it follows on release where it was pressed.
    #[serde(with = "crate::ui_state::pos_opt")]
    pub click_link: Option<BlockPos>,
    /// A line-leave save waiting for the first frame, in this pane only.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub save_after_frame: bool,
}

/// Host operations requested by a borrowed editor view. They run only after that borrow
/// ends, so navigation cannot confuse a panel's document with main's history/origin.
#[derive(Clone, Debug, PartialEq)]
pub enum PaneEffect {
    Action(String),
    Follow(String),
    Aside(crate::sidebar::PanelKey),
    Compare(String),
    OpenIssue { target: crate::editor::Target, line: String },
    Leave,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}
