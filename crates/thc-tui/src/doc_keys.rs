//! Compatibility entry points for the shared editor component.
//! Main and sidebar inputs both run `editor_pane::update`; no editing implementation lives here.
pub use crate::editor_pane::update::{drag_edge, drag_repeat_ms, handle, mouse, paste, run_write};
pub(crate) use crate::editor_pane::update::{base64, drag_hint_pending};
