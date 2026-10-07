//! The TUI's presentation state, all in one serializable value.
//!
//! `UiState` holds every choice the person (or an agent) has made about what the TUI shows: the
//! screen and its document, focus, overlays and prompts, list selections and scrolls, filters,
//! toasts, the history stack, pending keys and the logical clock. Store data (rows, counts,
//! labels) is not here: it is derived from the vault into `Derived`. Runtime handles (the
//! vault, channels, the writer thread, terminal capabilities) are not here either: they live
//! on `App` beside this state. The open document's text and undo live in the editor's own
//! model; `document` here is its presentation (which caret, which scroll).
//!
//! Every field has a default, so a state file needs only the fields it cares about. Unknown
//! fields are refused, so an older thc never silently ignores what a newer one wrote.
//! Maps are ordered (`BTreeMap`/`BTreeSet`), so the same state always serializes to the same
//! bytes. See docs/ui-protocol.md.

use crate::app::{Edit, Focus, MoveTarget, Overlay, PromptKind, Toast, UpdateState, View};
use crate::editor::{BlockPos, Target};
use crate::input::LineInput;
use crate::keymap::Key;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// The version of the state's JSON shape. Within a version, fields are only added.
pub const UI_STATE_VERSION: u32 = 1;

/// The open document's presentation: where the caret is (by note id and byte, so it survives
/// edits elsewhere) and how far it's scrolled. `target`, `dirty` and `revision` are reported
/// on reads and ignored on writes: the document itself comes from `view` with `page_open` or
/// `journal_date`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DocumentState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    /// The caret's note id (empty: a new line not saved yet).
    pub caret_id: String,
    /// The caret's byte offset in that note's text, on a character boundary.
    pub caret_byte: usize,
    /// The first visual row shown.
    pub scroll: usize,
    /// Unsaved typing (read-only).
    pub dirty: bool,
    /// The editor's content revision (read-only).
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiState {
    pub ui_state_version: u32,
    /// The logical clock, in milliseconds since the Unix epoch. Only `Msg::Tick` moves it; toasts,
    /// flashes and the which-key delay are measured against it, never against the wall clock.
    pub now_ms: u64,
    /// The local time zone's offset from UTC at `now_ms`, in minutes (supplied with the tick).
    pub utc_offset_min: i32,
    /// The local date at `now_ms`.
    pub today: NaiveDate,
    /// The vault this TUI is on (read-only here: switching vaults is an action).
    pub vault_name: String,
    /// Entered from a cross-vault view: Esc from the document goes back to this vault.
    pub return_vault: Option<PathBuf>,
    /// `space t v`: Today in a section per vault instead of merged.
    pub today_by_vault: bool,

    // ---- the screen ----------------------------------------------------------------------------
    pub view: View,
    pub cursor: usize,
    pub scroll: usize,
    /// The selected row's stable key (a node id, `tag:x`, `view:x`, `tx:x`). Wins over `cursor`.
    pub selected: Option<String>,
    pub show_detail: bool,
    pub focus: Focus,
    pub focus_mode: bool,
    /// What Focus shows this session (`[tui.focus]`, then live `:focus` changes).
    pub focus_cfg: thc_core::tui_config::Focus,
    /// Outline rows folded away, by node id.
    pub collapsed: BTreeSet<String>,
    /// IDs on page and Journal outline rows.
    pub page_ids: bool,

    // ---- filters and per-view choices ------------------------------------------------------------
    pub tasks_filter: String,
    pub search_terms: String,
    pub pages_filter: String,
    pub log_actor: Option<String>,
    pub log_node: Option<String>,
    pub review_lane: bool,
    pub agenda_mode: bool,
    pub show_all_done: bool,
    /// Turn the context (views.md §2) off for this session.
    pub context_on: bool,
    /// This device's scope for a view, overriding its stored one: by view name.
    pub scope_override: BTreeMap<String, String>,

    // ---- documents -------------------------------------------------------------------------------
    pub page_open: Option<String>,
    pub journal_date: NaiveDate,
    /// The open document's caret and scroll (see `DocumentState`).
    pub document: Option<DocumentState>,
    /// Write (typing) or Navigate inside the document.
    pub doc_write: bool,
    /// Just arrived: Tab and ⇧Tab still change views.
    pub doc_parked: bool,
    /// The document Esc goes back to: (document, the line to land on, the page it was opened for).
    pub doc_back: Option<(Target, String, String)>,
    /// The list view (and row) the open document was reached from.
    pub doc_origin: Option<(View, Option<String>)>,
    /// The caret's line when last looked at (leaving it saves).
    pub doc_line_id: Option<String>,
    /// Navigate's line selection start.
    pub doc_vsel: Option<usize>,
    /// Navigate's cursor in the footer rows.
    pub doc_footer_cur: Option<usize>,
    /// The wheel moved the view: it stays put until a key brings the caret back.
    pub doc_scroll_free: bool,
    /// A remote change landed on the caret's line: Some(edited).
    pub doc_announce: Option<bool>,
    /// The rail's page order, frozen while going document to document.
    pub rail_frozen: Option<Vec<String>>,
    /// The page last opened (where the Pages list's cursor rests).
    pub last_page: Option<String>,
    /// Documents opened this session, most recent first.
    pub recent_docs: Vec<Target>,
    /// The `[[` popup: open, and its selected row.
    pub link_open: bool,
    pub link_sel: Option<usize>,
    /// ⌥V: the next paste is plain text.
    pub paste_plain: bool,
    /// A drop just attached: (its line's id, the pasted text, the undo depth then).
    pub last_drop: Option<(String, String, usize)>,
    /// A new link close to an existing page: (line id, typed, existing, stub id, since ms).
    pub near_miss: Option<(String, String, String, Option<String>, u64)>,

    // ---- input -----------------------------------------------------------------------------------
    /// The row being edited in place.
    pub edit: Option<Edit>,
    pub prompt: Option<(PromptKind, LineInput)>,
    /// A pending confirmation: (key, target).
    pub awaiting: Option<(char, String)>,
    pub overlay: Option<Overlay>,
    /// A key sequence in progress (`g`, `p`, `S`), in keymap notation (`g`, `C-t`).
    #[serde(with = "keys_notation")]
    pub pending_keys: Vec<Key>,
    /// When the pending prefix started (logical ms; the which-key popup's delay).
    pub pending_since: Option<u64>,
    /// The focused input hasn't been edited since it was focused (digits pick saved filters).
    pub input_untouched: bool,
    pub recent_cmds: Vec<String>,
    pub recent_moves: Vec<MoveTarget>,

    // ---- the pointer -----------------------------------------------------------------------------
    pub hover: Option<(u16, u16)>,
    pub scroll_drag: bool,
    #[serde(with = "pos_opt")]
    pub drag_from: Option<BlockPos>,
    #[serde(with = "pos_opt")]
    pub click_link: Option<BlockPos>,
    /// The last left press: (ms, x, y, count), for double and triple clicks.
    pub last_click: Option<(u64, u16, u16, u8)>,

    // ---- notices ---------------------------------------------------------------------------------
    pub toast: Option<Toast>,
    /// Rows flashed by a change: node id → (since ms, changed by an agent).
    pub flashes: BTreeMap<String, (u64, bool)>,
    /// Node the alert toast refers to (x / z / Z act on it).
    pub alert_toast_node: Option<String>,
    pub update_state: UpdateState,
    /// A version you haven't opened About on.
    pub about_new: bool,
    pub write_alt_hint: bool,
    pub meta_hint: bool,
    pub focus_hint_shown: bool,
    pub offline_toast_shown: bool,

    // ---- history ---------------------------------------------------------------------------------
    /// Navigation history, ⌘[ / ⌘] (history.rs).
    pub history: crate::history::History,
}

impl Default for UiState {
    fn default() -> Self {
        let today = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
        UiState {
            ui_state_version: UI_STATE_VERSION,
            now_ms: 0,
            utc_offset_min: 0,
            today,
            vault_name: String::new(),
            return_vault: None,
            today_by_vault: false,
            view: View::Today,
            cursor: 0,
            scroll: 0,
            selected: None,
            show_detail: true,
            focus: Focus::List,
            focus_mode: false,
            focus_cfg: Default::default(),
            collapsed: BTreeSet::new(),
            page_ids: false,
            tasks_filter: DEFAULT_TASKS_FILTER.into(),
            search_terms: String::new(),
            pages_filter: String::new(),
            log_actor: None,
            log_node: None,
            review_lane: false,
            agenda_mode: false,
            show_all_done: false,
            context_on: true,
            scope_override: BTreeMap::new(),
            page_open: None,
            journal_date: today,
            document: None,
            doc_write: false,
            doc_parked: false,
            doc_back: None,
            doc_origin: None,
            doc_line_id: None,
            doc_vsel: None,
            doc_footer_cur: None,
            doc_scroll_free: false,
            doc_announce: None,
            rail_frozen: None,
            last_page: None,
            recent_docs: Vec::new(),
            link_open: false,
            link_sel: None,
            paste_plain: false,
            last_drop: None,
            near_miss: None,
            edit: None,
            prompt: None,
            awaiting: None,
            overlay: None,
            pending_keys: Vec::new(),
            pending_since: None,
            input_untouched: false,
            recent_cmds: Vec::new(),
            recent_moves: Vec::new(),
            hover: None,
            scroll_drag: false,
            drag_from: None,
            click_link: None,
            last_click: None,
            toast: None,
            flashes: BTreeMap::new(),
            alert_toast_node: None,
            update_state: UpdateState::Idle,
            about_new: false,
            write_alt_hint: false,
            meta_hint: false,
            focus_hint_shown: false,
            offline_toast_shown: false,
            history: Default::default(),
        }
    }
}

pub const DEFAULT_TASKS_FILTER: &str = "status:open sort:due";

/// Fields the runtime owns: a state that leaves them out keeps the session's values (the
/// clock and the vault are facts about the session, not choices).
pub const SESSION_FIELDS: &[&str] = &["now_ms", "utc_offset_min", "today", "vault_name", "journal_date"];

/// Fields `state.get` with `history: false` leaves out (they can be large and few clients need
/// them). A state without them rehydrates with the session's own.
pub const HISTORY_FIELDS: &[&str] = &["history", "recent_docs", "recent_cmds", "recent_moves"];

impl UiState {
    /// How long ago `since` (logical ms) was.
    pub fn age(&self, since: u64) -> std::time::Duration {
        std::time::Duration::from_millis(self.now_ms.saturating_sub(since))
    }

    /// The local wall-clock time at `now_ms`.
    pub fn local_time(&self) -> chrono::NaiveDateTime {
        let utc = chrono::DateTime::from_timestamp_millis(self.now_ms as i64).unwrap_or_default().naive_utc();
        utc + chrono::Duration::minutes(self.utc_offset_min as i64)
    }

    /// The clock moves: `now_ms` and the offset, and `today` with them (a new day keeps the
    /// journal on the day it showed unless it showed today).
    pub fn tick(&mut self, now_ms: u64, utc_offset_min: i32) {
        self.now_ms = now_ms;
        self.utc_offset_min = utc_offset_min;
        self.today = self.local_time().date();
    }

    /// Whether the clock moving to `now_ms` changes what's on screen: a toast or a flash to
    /// expire, a which-key delay, a near-miss notice, or a new minute for the bar's clock.
    pub fn wants_clock(&self, now_ms: u64) -> bool {
        now_ms / 60_000 != self.now_ms / 60_000
            || self.toast.is_some()
            || !self.flashes.is_empty()
            || !self.pending_keys.is_empty()
            || self.near_miss.is_some()
    }

    /// The state as JSON.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("UiState serializes")
    }

    /// The state without the fields in `HISTORY_FIELDS`.
    pub fn to_json_without_history(&self) -> serde_json::Value {
        let mut v = self.to_json();
        if let Some(o) = v.as_object_mut() {
            for f in HISTORY_FIELDS {
                o.remove(*f);
            }
        }
        v
    }

    /// Reads a (possibly partial) state over `base`: fields the JSON leaves out take the
    /// defaults, except the session's own (`SESSION_FIELDS`, and with `keep_history` the
    /// `HISTORY_FIELDS`), which keep `base`'s values. Refuses unknown fields, wrong types and
    /// an unsupported version, with the field's path.
    pub fn from_json_over(base: &UiState, json: &serde_json::Value, keep_history: bool) -> Result<UiState, String> {
        let Some(given) = json.as_object() else { return Err("a UI state is a JSON object".into()) };
        let mut merged = UiState::default().to_json();
        let base_json = base.to_json();
        let obj = merged.as_object_mut().unwrap();
        let keep = SESSION_FIELDS.iter().chain(if keep_history { HISTORY_FIELDS } else { &[] }.iter());
        for f in keep {
            if let Some(v) = base_json.get(*f) {
                obj.insert((*f).to_string(), v.clone());
            }
        }
        for (k, v) in given {
            obj.insert(k.clone(), v.clone());
        }
        Self::parse(&merged)
    }

    /// Strict parse of a whole state object.
    pub fn parse(json: &serde_json::Value) -> Result<UiState, String> {
        if let Some(v) = json.get("ui_state_version") {
            if v.as_u64() != Some(UI_STATE_VERSION as u64) {
                return Err(format!("ui_state_version {v} isn't supported (this thc reads {UI_STATE_VERSION})"));
            }
        }
        let state: UiState = serde_json::from_value(json.clone()).map_err(|e| explain(&e.to_string(), json))?;
        state.validate()?;
        Ok(state)
    }

    /// Checks what types alone can't.
    pub fn validate(&self) -> Result<(), String> {
        if self.history.entries.len() > crate::history::CAP {
            return Err(format!("history: at most {} entries", crate::history::CAP));
        }
        if self.history.pos > self.history.entries.len() {
            return Err("history.pos: past the last entry".into());
        }
        for s in [&self.tasks_filter, &self.search_terms, &self.pages_filter] {
            if s.len() > MAX_INPUT {
                return Err(format!("a filter is at most {MAX_INPUT} bytes"));
            }
        }
        Ok(())
    }

    /// RFC 7396 JSON merge patch onto this state: objects merge, arrays and values replace,
    /// null clears. The result is validated as a whole before anything changes.
    pub fn patched(&self, patch: &serde_json::Value) -> Result<UiState, String> {
        if !patch.is_object() {
            return Err("a patch is a JSON object (RFC 7396 merge patch)".into());
        }
        let mut v = self.to_json();
        merge_patch(&mut v, patch);
        Self::parse(&v)
    }

    /// The top-level fields whose values differ between two states.
    pub fn changed_fields(&self, other: &UiState) -> Vec<String> {
        let (a, b) = (self.to_json(), other.to_json());
        let (Some(a), Some(b)) = (a.as_object(), b.as_object()) else { return vec![] };
        a.keys().filter(|k| a.get(*k) != b.get(*k)).cloned().collect()
    }
}

/// The longest filter or search text a state may carry.
pub const MAX_INPUT: usize = 4096;

pub fn merge_patch(target: &mut serde_json::Value, patch: &serde_json::Value) {
    use serde_json::Value;
    match patch {
        Value::Object(p) => {
            if !target.is_object() {
                *target = Value::Object(Default::default());
            }
            let t = target.as_object_mut().unwrap();
            for (k, v) in p {
                if v.is_null() {
                    t.remove(k);
                } else {
                    merge_patch(t.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        _ => *target = patch.clone(),
    }
}

/// serde's message, with a nearest known field for an unknown one.
fn explain(msg: &str, json: &serde_json::Value) -> String {
    let Some(rest) = msg.strip_prefix("unknown field `") else { return msg.to_string() };
    let Some(name) = rest.split('`').next() else { return msg.to_string() };
    let known: Vec<String> = UiState::default().to_json().as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
    let top = json.as_object().is_some_and(|o| o.contains_key(name));
    let best = known.iter().map(|k| (strsim(name, k), k)).filter(|(d, _)| *d <= 3).min_by_key(|(d, _)| *d).map(|(_, k)| k.clone());
    match best.filter(|_| top) {
        Some(b) => format!("unknown field `{name}` · did you mean `{b}`?"),
        None => msg.split(", expected").next().unwrap_or(msg).to_string(),
    }
}

/// Levenshtein distance (small inputs: field names).
fn strsim(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + (ca != *cb) as usize).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Keys in keymap notation (`g`, `space`, `C-t`, `A-up`).
mod keys_notation {
    use crate::keymap::Key;
    use serde::{Deserialize, Deserializer, Serializer, de::Error, ser::SerializeSeq};
    pub fn serialize<S: Serializer>(keys: &[Key], s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(keys.len()))?;
        for k in keys {
            seq.serialize_element(&k.notation())?;
        }
        seq.end()
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Key>, D::Error> {
        let names = Vec::<String>::deserialize(d)?;
        names.iter().map(|n| Key::parse(n).ok_or_else(|| D::Error::custom(format!("pending_keys: `{n}` isn't a key")))).collect()
    }
}

/// A caret position as `{"line", "byte"}`.
mod pos_opt {
    use crate::editor::BlockPos;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        line: usize,
        byte: usize,
    }
    pub fn serialize<S: Serializer>(p: &Option<BlockPos>, s: S) -> Result<S::Ok, S::Error> {
        p.map(|p| P { line: p.line, byte: p.byte }).serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<BlockPos>, D::Error> {
        Ok(Option::<P>::deserialize(d)?.map(|p| BlockPos { line: p.line, byte: p.byte }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_state_round_trips_byte_for_byte() {
        let s = UiState::default();
        let a = serde_json::to_string(&s).unwrap();
        let back: UiState = serde_json::from_str(&a).unwrap();
        assert_eq!(back, s);
        assert_eq!(serde_json::to_string(&back).unwrap(), a);
    }

    #[test]
    fn a_rich_state_round_trips() {
        let mut s = UiState::default();
        s.view = View::Tasks;
        s.tasks_filter = "status:open #work".into();
        s.selected = Some("abc123def456".into());
        s.collapsed.insert("n1".into());
        s.flashes.insert("n2".into(), (5, true));
        s.pending_keys = vec![Key::parse("g").unwrap(), Key::parse("C-t").unwrap(), Key::parse("A-up").unwrap(), Key::parse("space").unwrap()];
        s.prompt = Some((PromptKind::Due("n1".into()), LineInput { buf: "fri".into(), cur: 3 }));
        s.overlay = Some(Overlay::Palette { input: LineInput { buf: "do".into(), cur: 2 }, sel: 1 });
        s.toast = Some(Toast { kind: crate::app::ToastKind::Info, parts: vec![("hi".into(), crate::theme::Token::Muted)], at: 9 });
        s.drag_from = Some(BlockPos { line: 3, byte: 2 });
        s.doc_back = Some((Target::Journal { date: NaiveDate::from_ymd_opt(2026, 10, 7).unwrap() }, "x".into(), "y".into()));
        s.document = Some(DocumentState { caret_id: "n1".into(), caret_byte: 2, scroll: 4, ..Default::default() });
        s.update_state = UpdateState::Downloading { version: "1.0".into() };
        let json = s.to_json();
        let back = UiState::parse(&json).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.to_json(), json);
    }

    #[test]
    fn unknown_fields_are_refused_with_a_suggestion() {
        let e = UiState::parse(&serde_json::json!({"veiw": "tasks"})).unwrap_err();
        assert!(e.contains("did you mean `view`"), "{e}");
        let e = UiState::parse(&serde_json::json!({"view": "taskz"})).unwrap_err();
        assert!(e.contains("taskz"), "{e}");
        let e = UiState::parse(&serde_json::json!({"ui_state_version": 2})).unwrap_err();
        assert!(e.contains("isn't supported"), "{e}");
    }

    #[test]
    fn a_partial_state_keeps_the_session_fields() {
        let mut base = UiState::default();
        base.now_ms = 42;
        base.vault_name = "acme".into();
        base.view = View::Log;
        let s = UiState::from_json_over(&base, &serde_json::json!({"view": "tasks"}), true).unwrap();
        assert_eq!((s.now_ms, s.vault_name.as_str(), s.view), (42, "acme", View::Tasks));
    }

    #[test]
    fn merge_patch_merges_objects_and_null_clears() {
        let mut s = UiState::default();
        s.selected = Some("x".into());
        let p = s.patched(&serde_json::json!({"view": "pages", "selected": null, "focus_cfg": {"custom_width": 60}})).unwrap();
        assert_eq!((p.view, p.selected.clone(), p.focus_cfg.custom_width), (View::Pages, None, Some(60)));
        assert_eq!(s.changed_fields(&p), vec!["view", "selected", "focus_cfg"]);
        assert!(s.patched(&serde_json::json!({"cursor": "no"})).is_err());
    }
}
