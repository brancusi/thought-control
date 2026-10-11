//! Screens: thc's views as one fictty `Ui`, built from Rust types. The seven views are the tabs
//! of one tabs node (its titles over the `frame` feed, the mark as its lead and the date as its
//! trail), so a click on a tab or a key switches views without a new UI. A Detail, an Outline, or a
//! page or day being written in opens over its tab's view as a layer, shown while the store says
//! so (`open_<view>`): opening and closing are store writes, so nothing is pushed and no source
//! refetches but the layer's own. Every source is a host source
//! the feeds answer (`source`), every action a host action ThcHost does (`act`), so the UI value
//! never holds data: the same value renders a snapshot (`App::headless`) and runs live (`App::run`).
//!
//! The frame (tui-handoff.md §3): header and rule (the tabs), content, bar. Content splits
//! list | detail at 120 columns and up (`when: "$screen.width>=120"`), the list min(60%, 96).

use fictty::model::{Action, Column, Float, Kind, Node, Pad, Rows, Size, Source, Ui};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Which screen a UI starts on, and what it shows (`thc screen --view …`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Route {
    pub view: View,
    /// The node a Detail, an Outline or a Page shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The Tasks filter or the Search words to start with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// The Journal day (`2026-10-11`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// The view a Detail or a Day opens over (default Today, the Journal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back: Option<View>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    #[default]
    Today,
    Inbox,
    Tasks,
    Pages,
    Journal,
    Search,
    Log,
    /// One node, full width.
    Detail,
    /// One page's outline, as rows (`o` on a page).
    Outline,
    /// One page as text to write in (Enter on a page): `page/<id>`.
    Page,
    /// One journal day as text to write in (Enter on a day): `day/<date>`.
    Day,
}

impl View {
    pub const TABS: [View; 7] = [View::Today, View::Inbox, View::Tasks, View::Pages, View::Journal, View::Search, View::Log];

    pub fn parse(s: &str) -> Option<View> {
        serde_json::from_value(Value::String(s.to_lowercase())).ok()
    }

    pub fn name(self) -> &'static str {
        match self {
            View::Today => "today",
            View::Inbox => "inbox",
            View::Tasks => "tasks",
            View::Pages => "pages",
            View::Journal => "journal",
            View::Search => "search",
            View::Log => "log",
            View::Detail => "detail",
            View::Outline => "outline",
            View::Page => "page",
            View::Day => "day",
        }
    }
}

impl Route {
    pub fn to(view: View) -> Route {
        Route { view, ..Route::default() }
    }

    /// The tab this route shows in: a Detail under the view it came from, an Outline or a Page
    /// under Pages, a Day under the Journal (or Pages, where it was picked).
    pub fn tab(&self) -> usize {
        let v = match self.view {
            View::Detail => self.back.unwrap_or(View::Today),
            View::Outline | View::Page => View::Pages,
            View::Day => self.back.unwrap_or(View::Journal),
            v => v,
        };
        View::TABS.iter().position(|t| *t == v).unwrap_or(0)
    }

}

// ---- the one place sources and actions are named -------------------------------------------------

/// A data source: the host's feed `name` with `args` (templates: `{@today.id}` the selected
/// row's id, `{store.search_q}` what's typed; refetched when they change).
pub fn source(name: &str, args: Value) -> Source {
    Source { cmd: None, shell: None, stream: None, value: None, path: None, every: None, host: Some(name.into()), args }
}

/// A host action (`done`, `reopen`, `set`, `add`, `mv`, `answer`, `day`, `keys`): args are
/// templates over the selected row (`{id}`), the store (`{store.key}`) and `$id`.
pub fn act(name: &str, args: Value) -> Action {
    Action::Host { host: name.into(), args, wait: false }
}

/// A host action that changes the sidebar's stack (open, close, fold, pin, move, reopen, a day
/// panel's day, to main): what the next key acts on is what it leaves, so keys typed while it's
/// in flight wait for it and the rows it refreshes, then go in order (fictty's `wait`).
fn stack_act(name: &str, args: Value) -> Action {
    Action::Host { host: name.into(), args, wait: true }
}

/// What a route focuses first: its list (arriving never takes the caret, navigation.md §6: a
/// finder's prompt gets it when a letter is typed), or what it opens.
pub fn first_focus(route: &Route) -> Option<String> {
    let tab = View::TABS[route.tab()].name();
    Some(match route.view {
        View::Detail => format!("{tab}_detail"),
        View::Outline => "pages_outline_rows".into(),
        View::Page | View::Day => format!("{tab}_doc"),
        View::Today => "today".into(),
        v => v.name().into(),
    })
}

// ---- node helpers ---------------------------------------------------------------------------------

fn node(kind: Kind) -> Node {
    Node { kind, ..Node::default() }
}

fn sized(mut n: Node, size: Size) -> Node {
    n.size = Some(size);
    n
}

fn cells(n: u16) -> Size {
    Size::Cells(n)
}

fn spec(s: &str) -> Size {
    Size::Spec(s.into())
}

fn text(t: &str) -> Node {
    node(Kind::Text { text: t.into() })
}

fn bound(mut n: Node, bind: &str) -> Node {
    n.bind = Some(bind.into());
    n
}

fn when(mut n: Node, cond: &str) -> Node {
    n.when = Some(cond.into());
    n
}

fn col(children: Vec<Node>) -> Node {
    node(Kind::Col { children, gap: None })
}

fn row(children: Vec<Node>) -> Node {
    node(Kind::Row { children, gap: None })
}

fn variant(mut n: Node, v: &str) -> Node {
    n.variant = Some(v.into());
    n
}

fn id(mut n: Node, id: &str) -> Node {
    n.id = Some(id.into());
    n
}

fn keys(mut n: Node, k: BTreeMap<String, Action>) -> Node {
    n.keys.extend(k);
    n
}

/// Show a view's tab. Leaving a view forgets what was done in it (`done_here`, §8), so the next
/// arrival shows its lists in their own order again.
fn show(tab: usize) -> Action {
    Action::Do { steps: vec![Action::Put { put: "done_here".into(), value: json!([]), for_ms: None }, Action::ShowTab { tab, of: "views".into() }] }
}

/// The keys every focusable node in tab `i` takes, since a focused node's keys come first:
/// - Tab and ⇧Tab cycle the views (navigation.md §6, N16), but not in a document, where Tab is
///   the editor's (indent, E22);
/// - `?` and `:` open the view's help and palette, but not where they'd be typed (a document,
///   an input).
fn tab_keys(mut n: Node, i: usize) -> Node {
    let k = View::TABS.len();
    let v = View::TABS[i];
    let editor = matches!(n.kind, Kind::Editor { .. });
    if n.focusable() && !editor {
        n.keys.entry("tab".into()).or_insert_with(|| show((i + 1) % k));
        n.keys.entry("s-tab".into()).or_insert_with(|| show((i + k - 1) % k));
        if !matches!(n.kind, Kind::Input { .. }) {
            for (key, a) in overlay_binds(v) {
                n.keys.entry(key.clone()).or_insert(a);
            }
        }
    }
    if let Some(cs) = n.children_mut() {
        let taken = std::mem::take(cs);
        *cs = taken.into_iter().map(|c| tab_keys(c, i)).collect();
    }
    n
}

/// Keys on every focusable node, editors and inputs too (they're `⌥` chords, which type
/// nothing); a node's own binding of the key stays (the sidebar's `⌥S` goes back).
fn with_keys_everywhere(mut n: Node, binds: &[Bind]) -> Node {
    if n.focusable() {
        for x in binds {
            n.keys.entry(x.key.clone()).or_insert_with(|| x.action.clone());
        }
    }
    if let Some(cs) = n.children_mut() {
        let taken = std::mem::take(cs);
        *cs = taken.into_iter().map(|c| with_keys_everywhere(c, binds)).collect();
    }
    n
}

/// `?` and `:` in a view: its help, its palette.
fn overlay_binds(v: View) -> Vec<(String, Action)> {
    let open = |flag: &str, focus: String| Action::Do { steps: vec![Action::Put { put: flag.into(), value: true.into(), for_ms: None }, Action::Focus { focus }] };
    vec![("?".into(), open("help", format!("{}_help", v.name()))), (":".into(), open("palette", format!("{}_palette_q", v.name())))]
}

/// What a view's main list is: where the focus goes back to from its help and palette.
fn main_id(v: View) -> String {
    first_focus(&Route::to(v)).unwrap_or_else(|| v.name().into())
}

/// Close the help or the palette: the view's list has the focus again.
fn close_overlay(v: View) -> Action {
    Action::Do {
        steps: vec![
            Action::Put { put: "help".into(), value: false.into(), for_ms: None },
            Action::Put { put: "palette".into(), value: false.into(), for_ms: None },
            Action::Put { put: format!("{}_palette_q", v.name()), value: "".into(), for_ms: None },
            // Back to the view's list, or to the page or day open over it (the last that's there
            // wins), so a palette command for writing (⌥L) runs in the text.
            Action::Focus { focus: main_id(v) },
            Action::Focus { focus: format!("{}_doc", v.name()) },
        ],
    }
}

/// The help overlay (§7.2: 74 × 18, three rows down): the view's bindings by group, from the
/// store entry `keys_<view>` the build writes from the same `Bind`s the keys come from.
fn help_overlay(v: View) -> Node {
    let r = Rows {
        item: Some("<text>{label:14}</><muted>{words}</>".into()),
        variant: Some("kind".into()),
        variants: [("section".to_string(), "<text+b>{title}</>".to_string()), ("blank".to_string(), String::new())].into_iter().collect(),
        skip: vec!["section".into(), "blank".into()],
        mark: Some("<accent+b>▌</> ".into()),
        lead: Some("  ".into()),
        selected: Some("on-selection".into()),
        ..Rows::default()
    };
    let mut l = list(&format!("{}_help", v.name()), &format!("keys_{}", v.name()), r);
    l.keys.insert("esc".into(), close_overlay(v));
    l.keys.insert("?".into(), close_overlay(v));
    l.title = Some(" help ".into());
    l.border = true;
    l.pad = Some(Pad::X(1));
    let mut l = variant(when(l, "help"), "overlay");
    l.float = Some(Float { width: Some(cells(74)), height: Some(cells(18)), at: Some("top".into()), margin: Some(3) });
    l.modal = true;
    l
}

/// The command palette (§7.2: 74 × 9, four rows down): what's typed filters the view's commands
/// (`cmds_<view>`, from the same `Bind`s); Enter runs the chosen one by pressing its keys, back in
/// the view, so a command does exactly what its key does.
fn palette_overlay(v: View) -> Node {
    let lid = format!("{}_palette", v.name());
    let r = Rows {
        item: Some("{words:34}<muted>{group:16}</>\t<text>{label}</> ".into()),
        mark: Some("<accent+b>▌</> ".into()),
        lead: Some("  ".into()),
        selected: Some("on-selection".into()),
        filter: Some(format!("{}_palette_q", v.name())),
        ..Rows::default()
    };
    let run = |from_list: bool| {
        let mut steps = if from_list { vec![] } else { vec![Action::Focus { focus: lid.clone() }] };
        steps.push(act("press", json!({"keys": "{key}"})));
        steps.push(close_overlay(v));
        Action::Do { steps }
    };
    let mut l = list(&lid, &format!("cmds_{}", v.name()), r);
    l.keys.insert("enter".into(), run(true));
    l.keys.insert("esc".into(), close_overlay(v));
    let mut field = variant(input(&format!("{}_palette_q", v.name()), "", "a command"), "input");
    field.keys.insert("enter".into(), run(false));
    field.keys.insert("esc".into(), close_overlay(v));
    for key in ["down", "up"] {
        field.keys.insert(key.into(), Action::Do { steps: vec![Action::Focus { focus: lid.clone() }, Action::Named(key.into())] });
    }
    let line = sized(row(vec![sized(text("<accent+b>:</> "), cells(2)), field]), cells(1));
    let mut c = col(vec![line, l]);
    c.title = Some(" commands ".into());
    c.border = true;
    c.pad = Some(Pad::X(1));
    let mut c = variant(when(c, "palette"), "overlay");
    c.float = Some(Float { width: Some(cells(74)), height: Some(cells(9)), at: Some("top".into()), margin: Some(4) });
    c.modal = true;
    c
}

/// Close the move picker or the delete question: back on the view's list.
fn close_ask(v: View) -> Action {
    Action::Do {
        steps: vec![
            Action::Put { put: "move".into(), value: false.into(), for_ms: None },
            Action::Put { put: "confirm".into(), value: false.into(), for_ms: None },
            Action::Put { put: format!("{}_move_q", v.name()), value: "".into(), for_ms: None },
            Action::Focus { focus: main_id(v) },
        ],
    }
}

/// The move picker (§7.2: min(W-6, 60) wide, six rows down): what's typed finds a page (the
/// host's fuzzy `dest`), Enter moves the row `m` was pressed on there. The row leaves the list,
/// so the cursor is on the next one.
fn move_overlay(v: View, data: &mut BTreeMap<String, Source>) -> Node {
    let lid = format!("{}_move", v.name());
    let name = format!("{}_dest", v.name());
    data.insert(name.clone(), source("dest", json!({"query": format!("{{store.{}_move_q}}", v.name())})));
    let r = Rows { item: Some("{label_m|markup}\t<muted>{short}</> ".into()), mark: Some("<accent+b>▌</> ".into()), lead: Some("  ".into()), key: Some("id".into()), ..Rows::default() };
    let go = |from_list: bool| {
        let mut steps = if from_list { vec![] } else { vec![Action::Focus { focus: lid.clone() }] };
        steps.push(act("mv", json!({"id": "{store.ask_id}", "under": "{id}"})));
        steps.push(close_ask(v));
        Action::Do { steps }
    };
    let mut l = list(&lid, &format!("{name}.rows"), r);
    l.keys.insert("enter".into(), go(true));
    l.keys.insert("esc".into(), close_ask(v));
    let mut field = variant(input(&format!("{}_move_q", v.name()), "", "a page"), "input");
    field.keys.insert("enter".into(), go(false));
    field.keys.insert("esc".into(), close_ask(v));
    for key in ["down", "up"] {
        field.keys.insert(key.into(), Action::Do { steps: vec![Action::Focus { focus: lid.clone() }, Action::Named(key.into())] });
    }
    let what = sized(bound(text("<muted>{.}</>"), "ask_text"), cells(1));
    let line = sized(row(vec![sized(text("<accent+b>›</> "), cells(2)), field]), cells(1));
    let mut c = col(vec![what, line, l]);
    c.title = Some(" move to ".into());
    c.border = true;
    c.pad = Some(Pad::X(1));
    let mut c = variant(when(c, "move"), "overlay");
    c.float = Some(Float { width: Some(cells(60)), height: Some(cells(14)), at: Some("top".into()), margin: Some(6) });
    c.modal = true;
    c
}

/// `D`'s question: y (or Enter) deletes the row it was pressed on, n or Esc doesn't.
fn confirm_overlay(v: View) -> Node {
    let id_ = format!("{}_confirm", v.name());
    let yes = Action::Do { steps: vec![act("delete", json!({"id": "{store.ask_id}"})), close_ask(v)] };
    let mut q = id(bound(node(Kind::Button { label: "Delete “{.}”?".into() }), "ask_text"), &id_);
    for k in ["y", "enter"] {
        q.keys.insert(k.into(), yes.clone());
    }
    for k in ["n", "esc"] {
        q.keys.insert(k.into(), close_ask(v));
    }
    let hint = sized(text(&format!(" {}", crate::feeds::hints(&[("y", "delete"), ("n", "keep it")], "  "))), cells(1));
    let mut c = col(vec![sized(q, cells(1)), hint]);
    c.border = true;
    c.title = Some(" delete ".into());
    c.pad = Some(Pad::X(1));
    let mut c = variant(when(c, "confirm"), "overlay");
    c.float = Some(Float { width: Some(cells(60)), height: Some(cells(4)), at: Some("top".into()), margin: Some(6) });
    c.modal = true;
    c
}

/// The store entries the help and the palette read: rows by group, and every command once.
fn help_rows(groups: &Groups) -> (Value, Value) {
    let mut help = Vec::new();
    let mut cmds = Vec::new();
    for (i, (group, binds)) in groups.iter().enumerate() {
        if binds.is_empty() {
            continue;
        }
        if i > 0 && !help.is_empty() {
            help.push(json!({"kind": "blank", "id": format!("blank:{group}")}));
        }
        help.push(json!({"kind": "section", "id": format!("section:{group}"), "title": group}));
        for x in binds {
            let row = json!({"kind": "key", "id": format!("{group}:{}", x.key), "key": x.key, "label": key_label(&x.key), "words": x.words, "group": group});
            help.push(row.clone());
            if x.key != "?" && x.key != ":" {
                cmds.push(row);
            }
        }
    }
    (Value::Array(help), Value::Array(cmds))
}

/// A finder's prompt row floated over the top of its view: drawn on top, but after the list in
/// the tree, so arriving (a digit, Tab, a click on the tab) focuses the list, never the prompt
/// (navigation.md §6, N14). The view leaves its first row for it.
fn over_list(prompt_row: Node, view: Node) -> Node {
    let mut prompt_row = prompt_row;
    prompt_row.size = None;
    prompt_row.float = Some(Float { width: None, height: Some(cells(1)), at: Some("top".into()), margin: None });
    node(Kind::Stack { children: vec![view, prompt_row] })
}

/// A finder (Pages, Search: navigation.md §6): the list has the focus, and a letter starts the
/// find, the query's first character, with the caret in the prompt from then on. `/` starts an
/// empty find. In the prompt every key types; Esc clears it and goes back to the list, ↑ ↓ move
/// into the list, Enter opens the top match.
fn finder(list: Node, prompt: Node, open_selected: Action) -> (Node, Node) {
    let (lid, q) = (list.id.clone().unwrap_or_default(), prompt.id.clone().unwrap_or_default());
    let mut list = list;
    for c in ('a'..='z').chain('A'..='Z') {
        list.keys.insert(
            c.to_string(),
            Action::Do {
                // A person's `put` leaves the caret after what it put: the next key follows it.
                steps: vec![Action::Put { put: q.clone(), value: format!("{{store.{q}}}{c}").into(), for_ms: None }, Action::Focus { focus: q.clone() }],
            },
        );
    }
    list.keys.insert("/".into(), Action::Do { steps: vec![Action::Put { put: q.clone(), value: "".into(), for_ms: None }, Action::Focus { focus: q.clone() }] });
    // Esc with a query clears it; the list keeps its selection.
    list.keys.insert("esc".into(), Action::Put { put: q.clone(), value: "".into(), for_ms: None });
    let mut prompt = prompt;
    prompt.keys.insert("esc".into(), Action::Do { steps: vec![Action::Put { put: q.clone(), value: "".into(), for_ms: None }, Action::Focus { focus: lid.clone() }] });
    for key in ["down", "up"] {
        prompt.keys.insert(key.into(), Action::Do { steps: vec![Action::Focus { focus: lid.clone() }, Action::Named(key.into())] });
    }
    // The open runs as the list, its selected row the top match.
    prompt.keys.insert("enter".into(), Action::Do { steps: vec![Action::Focus { focus: lid.clone() }, open_selected] });
    (list, prompt)
}

fn input(id_: &str, seed: &str, placeholder: &str) -> Node {
    id(node(Kind::Input { text: seed.into(), placeholder: Some(placeholder.into()), accept: None }), id_)
}

/// The main area is wide enough for a split (§3.4). It reads the width of the area it's laid out
/// in, so beside the sidebar it's the main pane's width that counts (sidebar.md §6.2).
const WIDE: &str = "$width>=120";

/// A status cell (§4): `[ ]` `[/]` `[w]` `[x]` `[-]`, a note's ` · `, and Search's ` ¶ ` / ` # `.
const STATUS: &str = "{status|todo=<text>[ ]</> ;doing=<doing>[/]</> ;waiting=<waiting>[w]</> ;done=<done>[x]</> ;cancelled=<muted>[-]</> ;note=<muted> · </> ;page=<muted> ¶ </> ;tag=<muted> # </> }";

/// The gutter of a row without the cursor, and with it (§4: conflict > cursor > live).
const GUTTER: &str = "{gutter|conflict=<conflict+b>≠</> ;live=<agent>•</> ;*=  }";
const GUTTER_CURSOR: &str = "{gutter|conflict=<conflict+b>≠</> ;*=<accent+b>▌</> }";

/// A node row (§4): id, status cell, text, and the meta right-aligned a cell from the edge.
fn node_item() -> String {
    format!("<muted>{{short}}</>  {STATUS}{{text_m|markup}}\t{{meta_m|markup}} ")
}

/// Rows of nodes, as every list and tree draws them: the `▌` gutter on the selected row, the
/// selection following the row's id, the text keeping 24 cells (§4.2) before the meta gives
/// way. The focused pane's cursor row is on `selection`, the other pane's shows only `▌` (§3.4:
/// the theme's `row.selected` and `row.selected.blur`). Sections, empty states, blanks and an
/// agenda's empty days take no cursor.
fn rows(item: String) -> Rows {
    Rows {
        item: Some(item),
        variant: Some("kind".into()),
        variants: [
            ("section", "{title_m|markup}{count|?  <muted>$</>}\t{note_m|markup} "),
            ("empty", "  {m|markup}"),
            ("blank", ""),
            ("line", "{m|markup}"),
            ("gap", "{m|markup}\t{note_m|markup} "),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect(),
        skip: ["section", "empty", "blank", "line", "gap"].map(String::from).to_vec(),
        // The gutter (§4): `≠` on a conflict, else the cursor, else `•` while an agent's change
        // is live. The row's `gutter` says which (the host's `feeds::gutter`).
        mark: Some(GUTTER_CURSOR.into()),
        lead: Some(GUTTER.into()),
        key: Some("id".into()),
        keep: Some(37),
        ..Rows::default()
    }
}

fn tree(id_: &str, bind: &str) -> Node {
    id(bound(node(Kind::Tree { items: vec![], nest: None, open: true, leaf: Some(String::new()), rows: rows(node_item()) }), bind), id_)
}

fn list(id_: &str, bind: &str, rows: Rows) -> Node {
    id(bound(node(Kind::List { items: vec![], rows }), bind), id_)
}

/// A key bound with its words. Every key the screen lists is bound through one of these, so the
/// help overlay (`?`) and the command palette (`:`) are the bindings themselves, never a second
/// table of what the keys do.
#[derive(Clone, Debug)]
pub struct Bind {
    pub key: String,
    pub words: String,
    pub action: Action,
}

fn b(key: &str, words: &str, action: Action) -> Bind {
    Bind { key: key.into(), words: words.into(), action }
}

/// The bindings as a node's (or the UI's) keys.
fn key_map(binds: &[Bind]) -> BTreeMap<String, Action> {
    binds.iter().map(|x| (x.key.clone(), x.action.clone())).collect()
}

/// A key as the help shows it: `Enter`, `⇧Tab`, `p h`.
fn key_label(key: &str) -> String {
    match key {
        "enter" => "Enter".into(),
        "esc" => "Esc".into(),
        "tab" => "Tab".into(),
        "s-tab" => "⇧Tab".into(),
        k => k.into(),
    }
}

/// A view's bindings in groups (the view's own, on a row, everywhere): what its help and its
/// palette list.
type Groups = Vec<(String, Vec<Bind>)>;

/// The keys on rows of nodes:
/// - x done, X reopen, Enter the detail;
/// - p then h, m, l or - the priority (§6.2);
/// - d a due date and m a place, asked in the bar (the prompt recipe, `prompt`).
fn node_binds(v: View) -> Vec<Bind> {
    // In the Inbox, triage moves on: after `x` or `t` the cursor goes to the next row (§5.2); `m`
    // and `D` take the row away, so the next one is under the cursor already.
    let then_next = |a: Action| if v == View::Inbox { Action::Do { steps: vec![a, Action::Named("down".into())] } } else { a };
    let mut k = vec![
        // Done stays on screen where it was until you leave (navigation.md §8): the id joins
        // `done_here`, which the lists' feeds keep.
        b("x", "done", then_next(Action::Do { steps: vec![act("done", json!({"id": "{id}"})), Action::Append { append: "done_here".into(), value: "{id}".into() }] })),
        b("X", "reopen", act("reopen", json!({"id": "{id}"}))),
        b("t", "task on / off", then_next(act("task", json!({"id": "{id}"})))),
        b("enter", "open", open_over(v, json!({"kind": "detail", "id": "{id}"}), &format!("{}_detail", v.name()))),
    ];
    for (key, p, words) in [("h", "high", "priority high"), ("m", "med", "priority medium"), ("l", "low", "priority low"), ("-", "", "no priority")] {
        k.push(b(&format!("p {key}"), words, act("set", json!({"id": "{id}", "props": {"priority": p}}))));
    }
    k.push(b(
        "d",
        "due date",
        Action::Do {
            steps: vec![
                Action::Put { put: "ask_id".into(), value: "{id}".into(), for_ms: None },
                Action::Put { put: "prompt".into(), value: "due".into(), for_ms: None },
                Action::Focus { focus: "prompt_in".into() },
            ],
        },
    ));
    // `m`: the move picker (§7.2) for this row; `D`: delete it, once you've said yes.
    let ask = |flag: &str, focus: String| {
        Action::Do {
            steps: vec![
                Action::Put { put: "ask_id".into(), value: "{id}".into(), for_ms: None },
                Action::Put { put: "ask_text".into(), value: "{text}".into(), for_ms: None },
                Action::Put { put: flag.into(), value: true.into(), for_ms: None },
                Action::Focus { focus },
            ],
        }
    };
    k.push(b("m", "move to a page", ask("move", format!("{}_move_q", v.name()))));
    // Its page or day beside the view (sidebar.md §2).
    k.push(b("o", "open aside", aside_row(v)));
    k.push(b("a-o", "open aside", aside_row(v)));
    k.push(b("s-enter", "open aside", aside_row(v)));
    k.push(b("D", "delete", ask("confirm", format!("{}_confirm", v.name()))));
    k
}

fn node_keys(v: View) -> BTreeMap<String, Action> {
    key_map(&node_binds(v))
}

/// Flip a store entry between two values (`w` agenda / sections, `\` the detail pane, `@` who):
/// the host writes the other one.
fn flip(key: &str, a: &str, b_: &str) -> Action {
    act("flip", json!({"key": key, "now": format!("{{store.{key}}}"), "a": a, "b": b_}))
}

/// `U`: undo the last undo (a redo), or the last redo, naming the change first undone.
fn redo() -> Action {
    act("undo", json!({"tx": "{store.redo_tx}", "of": "{store.redo_of}", "word": "{store.redo_word}"}))
}

/// The keys that work in every view (§6.1), but for `?` and `:`, which open the view's own help
/// and palette (`tab_keys`).
fn global_binds() -> Vec<Bind> {
    let names = ["Today", "Inbox", "Tasks", "Pages", "Journal", "Search", "Log"];
    let mut k: Vec<Bind> = (0..View::TABS.len()).map(|i| b(&(i + 1).to_string(), names[i], show(i))).collect();
    k.push(b("/", "search", show(5)));
    // `a` captures into today's journal, `A` into the inbox (tui-handoff §7.1), in capture syntax.
    for (key, ask, words) in [("a", "capture", "capture to today"), ("A", "inbox", "capture to the inbox")] {
        k.push(b(key, words, Action::Do { steps: vec![Action::Put { put: "prompt".into(), value: ask.into(), for_ms: None }, Action::Focus { focus: "prompt_in".into() }] }));
    }
    // The agent's last change, as its toast offers (§7.3): u undoes it, U redoes what u undid,
    // L reviews what agents did (the Log, agents only).
    k.push(b("u", "undo the agent's change", act("undo", json!({"tx": "{store.agent_tx}"}))));
    k.push(b("U", "redo", redo()));
    k.push(b("L", "review agents' changes", Action::Do { steps: vec![Action::Put { put: "log_by".into(), value: "agents".into(), for_ms: None }, show(6)] }));
    // `t` is a row's (task on / off, §6.2); the themes cycle on `~` (`--theme` picks one).
    k.push(b("~", "next theme", Action::Theme { theme: "next".into() }));
    k.push(b("\\", "detail pane on / off", flip("no_detail", "", "off")));
    k.push(b("esc", "close the page or day", close_documents()));
    k
}

/// The bar prompt (a recipe, §3.3's "bar prompts"): a line floated over the bar while the store's
/// `prompt` is set, its input's Enter answering the host with what was typed and the row it was
/// asked about, Esc closing it. Closing puts the focus back where it was (`$prev`).
fn prompt() -> Node {
    let clear = || {
        vec![
            Action::Put { put: "prompt".into(), value: "".into(), for_ms: None },
            Action::Put { put: "prompt_in".into(), value: "".into(), for_ms: None },
            Action::Focus { focus: "$prev".into() },
        ]
    };
    let mut answer = vec![act("answer", json!({"ask": "{store.prompt}", "id": "{store.ask_id}", "text": "{store.prompt_in}"}))];
    answer.extend(clear());
    let mut field = variant(input("prompt_in", "", "a thought, [ ] a task due:fri #tag · a date · a page"), "input");
    field.keys.insert("enter".into(), Action::Do { steps: answer });
    field.keys.insert("esc".into(), Action::Do { steps: clear() });
    let label = sized(bound(text(" {.|due=<accent+b>due ›</>;move=<accent+b>move to ›</>;capture=<accent+b>capture ›</>;inbox=<accent+b>inbox ›</>} "), "prompt"), cells(12));
    let mut line = variant(row(vec![variant(label, "input"), field]), "input");
    line.when = Some("prompt".into());
    line.float = Some(Float { width: None, height: Some(cells(1)), at: Some("bottom".into()), margin: Some(1) });
    line
}

/// The detail pane (§5.8): the lines of the node selected in `of`, from the `detail` feed (or
/// `tx` in the Log) with the selection's id as its argument.
fn pane(of: &str, feed: &str, data: &mut BTreeMap<String, Source>) -> Node {
    pane_with(of, feed, json!({"id": format!("{{@{of}.id}}")}), data)
}

/// A pane over a feed with its own args (Search's preview reads the query too). ⌃W moves the
/// focus into it and back (`split`).
fn pane_with(of: &str, feed: &str, args: Value, data: &mut BTreeMap<String, Source>) -> Node {
    let name = format!("{of}_pane");
    data.insert(name.clone(), source(feed, args));
    let mut lines = list(&name, &format!("{name}.lines"), lines_rows());
    lines.pad = Some(Pad::X(1));
    lines
}

/// How a detail pane draws its lines, by kind.
fn lines_rows() -> Rows {
    Rows {
        item: Some("{m|markup}".into()),
        empty: Some(String::new()),
        variant: Some("k".into()),
        variants: [
            ("title", format!("{STATUS}{{text_m|markup}}")),
            ("field", "<muted>{key:11}</>{m|markup}".into()),
            ("sub", format!(" {STATUS}{{text_m|markup}}\t{{meta_m|markup}} ")),
            ("hist", "<muted>{time}</>  {who_m|markup}  {m|markup}".into()),
            ("blank", String::new()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect(),
        mark: Some(String::new()),
        lead: Some(String::new()),
        ..Rows::default()
    }
}

/// A list | detail split (§3.4): at 120 columns and up a `│` and the pane (40%) beside the list,
/// the list at most 96; below that the list has the width. (The design's list is min(60%, 96):
/// the list fills, so at 120 it's 71 and the pane 48, a cell off the artboard.)
fn split(main: Node, side: Node) -> Node {
    let (mut main, mut side) = (main, side);
    // ⌃W: the focus to the other pane (§3.4).
    if let (Some(m), Some(s)) = (main.id.clone(), side.id.clone()) {
        main.keys.insert("c-w".into(), Action::Focus { focus: s });
        side.keys.insert("c-w".into(), Action::Focus { focus: m });
    }
    let mut main = sized(main, spec("*"));
    main.max = Some(96);
    let rule = sized(node(Kind::Rule { glyph: Some("│".into()) }), cells(1));
    // `\` takes the pane away (and puts it back).
    let shown = format!("{WIDE} & !no_detail & !sidebar_beside");
    row(vec![main, when(rule, &shown), when(sized(side, spec("40%")), &shown)])
}

/// The bar (§3.3): the view's name and counts on the left (`bar_m` of its feed), the daemon's
/// dot and the view's hints on the right. A toast owns the bar while it shows (§3.3, §7.3): it
/// draws over the whole row, daemon state included, on its tone's `toast.<tone>` part.
fn bar(feed: &str, hints: &[(&str, &str)]) -> Node {
    let right = format!(" {{live|true=<done>●</> <muted>live</>;*=<muted>○ local</>}}   {} ", crate::feeds::hints(hints, "  "));
    let plain: String = hints.iter().map(|(k, w)| format!("{k} {w}")).collect::<Vec<_>>().join("  ");
    let width = unicode_width::UnicodeWidthStr::width(format!(" ○ local   {plain} ").as_str()) as u16;
    let view = variant(row(vec![variant(bound(text(" {bar_m|markup}"), feed), "bar"), variant(sized(bound(text(&format!("\t{right}")), "frame"), cells(width)), "bar")]), "bar");
    // Pages and the Journal open a page or a day to write in over the view: while one is open
    // the bar is the document's (`¶ Q4 Planning · 3 open · writing`, the writing keys), from
    // the same source as its heading row.
    let mut lines = vec![view];
    if matches!(feed, "pages" | "journal") {
        let open = format!("open_{feed}.key");
        let keys = [("⌃T", "task"), ("⌃O", "open"), ("⌥O", "aside"), ("⌥L", "list"), ("⌥S", "sidebar"), ("Esc", "done")];
        let right = format!(" {{live|true=<done>●</> <muted>live</>;*=<muted>○ local</>}}   {} ", crate::feeds::hints(&keys, "  "));
        let plain: String = keys.iter().map(|(k, w)| format!("{k} {w}")).collect::<Vec<_>>().join("  ");
        let width = unicode_width::UnicodeWidthStr::width(format!(" ○ local   {plain} ").as_str()) as u16;
        let writing = variant(row(vec![variant(bound(text(" {bar_m|markup}"), &format!("{feed}_heading")), "bar"), variant(sized(bound(text(&format!("\t{right}")), "frame"), cells(width)), "bar")]), "bar");
        lines = vec![when(lines.remove(0), &format!("!{open}")), when(writing, &open)];
    }
    lines.push(node(Kind::Toasts {}));
    sized(node(Kind::Stack { children: lines }), cells(1))
}

/// The conflict banner (§7.4): one row under the tabs while the vault has open conflicts.
fn banner() -> Node {
    when(variant(sized(bound(text("{banner_m|markup}"), "frame"), cells(1)), "banner"), "frame.conflicts")
}

/// One view's place in the tabs: the conflict banner (while there are conflicts), its content,
/// its bar.
fn view(content: Node, feed: &str, hints: &[(&str, &str)]) -> (Node, Node) {
    (col(vec![banner(), content]), bar(feed, hints))
}

// ---- the views ------------------------------------------------------------------------------------

/// The store entry saying what's open over a view (`open_pages`: `{"kind": "page", "id", "key"}`),
/// null when nothing is. Opening and closing are writes to it: the UI stays, no source refetches.
fn open_key(v: View) -> String {
    format!("open_{}", v.name())
}

/// Open something over a view (a page or a day to write in, a node's detail, an outline): its
/// store entry says what, its layer shows it, the focus goes in.
fn open_over(v: View, what: Value, focus: &str) -> Action {
    Action::Do { steps: vec![Action::Put { put: open_key(v), value: what, for_ms: None }, Action::Focus { focus: focus.into() }] }
}

/// Close what's open over a view: the view again, its list focused, still on the row it opened
/// (navigation.md §2). (Not `$prev`: an open from the finder's prompt moves the focus twice in one
/// key, so `$prev` is the prompt, N04.)
fn close_over(v: View) -> Action {
    Action::Do { steps: vec![Action::Put { put: open_key(v), value: Value::Null, for_ms: None }, Action::Focus { focus: main_id(v) }] }
}

/// Esc anywhere it isn't bound closes a page or a day being written in. A document doesn't bind
/// Esc: there fictty drops a selection first (E12), and only Esc with nothing selected reaches
/// this. Each close focuses its own view's list, which is only there in its own tab.
fn close_documents() -> Action {
    let vs = [View::Pages, View::Journal];
    let mut steps: Vec<Action> = vs.iter().map(|v| Action::Put { put: open_key(*v), value: Value::Null, for_ms: None }).collect();
    steps.extend(vs.iter().map(|v| Action::Focus { focus: main_id(*v) }));
    // A page ⌥M opened from the sidebar goes back to where ⌥M was pressed (`return_tab`).
    steps.push(act("return", json!({"tab": "{store.return_tab}", "open": "{store.return_open}"})));
    Action::Do { steps }
}

/// What opens over a view, as layers shown while its `open_` entry says so (each modal: the focus
/// stays in it). Pages and the Journal open pages and days to write in, Pages an outline too;
/// the lists open a node's detail. Their sources read the entry, so they fetch when it changes.
fn layers(v: View, data: &mut BTreeMap<String, Source>) -> Vec<Node> {
    let k = open_key(v);
    let mut out = Vec::new();
    match v {
        View::Pages | View::Journal => {
            // A page or a day as Markdown to write in (tui-editor.md): the host's entry for it,
            // loaded when shown, saved half a second after the last change, reloaded when the
            // vault changes. `- [ ]` items tick with Ctrl-T or a click on the box. Tab keeps no
            // view-cycling binding here: in a document it's the editor's (indent, E22).
            // Tab nests and un-nests the lines (the outliner Tab, E22); ⌃T cycles a line through
            // task, done and text (E75).
            let editor = Kind::Editor { text: String::new(), doc: Some("{key}".into()), placeholder: Some("write here".into()), syntax: Some("markdown".into()), readonly: false, tab: Some("indent-lines".into()), tasks: Some("cycle".into()), caret: Some("end".into()), links: doc_links(v), ordered: ordered() };
            let mut doc = id(bound(node(editor), &k), &format!("{}_doc", v.name()));
            doc.pad = Some(Pad::X(1));
            // ⌃O follows the link at the caret, ⌥O opens it aside (E66).
            doc.keys.extend(key_map(&link_binds(v)));
            // No Esc here: the UI's Esc (`close_documents`) closes it, after fictty has let Esc
            // drop a selection (E12).
            // The title is a heading row above the text, not a line of it (writing.md): ⌘A, ⌘↑
            // and ⇧⌘↑ stay in the content. (Renaming a page is its own action.)
            let heading = format!("{}_heading", v.name());
            data.insert(heading.clone(), source("doc", json!({"key": format!("{{store.{k}.key}}")})));
            let title = sized(bound(text(" {title_m|markup}\t{meta_m|markup} "), &heading), cells(1));
            out.push(when(col(vec![title, sized(text(""), cells(1)), doc]), &format!("{k}.key")));
            if v == View::Pages {
                data.insert("pages_outline".into(), source("outline", json!({"id": format!("{{store.{k}.id}}"), "live": "{store.live}"})));
                let title = sized(bound(text("{title_m|markup}\t{meta_m|markup} "), "pages_outline"), cells(1));
                let mut rows = keys(tree("pages_outline_rows", "pages_outline.rows"), node_keys(View::Pages));
                rows.keys.remove("enter");
                rows.keys.insert("esc".into(), close_over(v));
                out.push(when(col(vec![title, sized(text(""), cells(1)), rows]), &format!("{k}.kind=outline")));
            }
        }
        View::Log => {}
        _ => {
            let name = format!("{}_open", v.name());
            data.insert(name.clone(), source("detail", json!({"id": format!("{{store.{k}.id}}")})));
            let mut lines = list(&format!("{}_detail", v.name()), &format!("{name}.lines"), lines_rows());
            lines.pad = Some(Pad::X(1));
            lines.keys.insert("esc".into(), close_over(v));
            lines.keys.insert("x".into(), act("done", json!({"id": format!("{{store.{k}.id}}")})));
            lines.keys.insert("X".into(), act("reopen", json!({"id": format!("{{store.{k}.id}}")})));
            out.push(when(lines, &format!("{k}.id")));
        }
    }
    // Each covers its view (a float over the whole of it) without holding the focus there, so
    // the sidebar beside it can take the focus (⌥S) and give it back.
    out.into_iter()
        .map(|mut l| {
            l.float = Some(Float { width: None, height: None, at: None, margin: None });
            l
        })
        .collect()
}

/// What a route opens at the start: the `open_` entry it seeds, if any.
fn opened(route: &Route, today: chrono::NaiveDate) -> Option<(View, Value)> {
    let id_ = route.id.clone().unwrap_or_default();
    let tab = View::TABS[route.tab()];
    Some(match route.view {
        View::Detail => (tab, json!({"kind": "detail", "id": id_})),
        View::Page => (tab, json!({"kind": "page", "id": id_, "key": format!("page/{id_}")})),
        View::Outline => (tab, json!({"kind": "outline", "id": id_})),
        View::Day => (tab, json!({"kind": "day", "key": format!("day/{}", journal_date(route, today))})),
        _ => return None,
    })
}

/// The Journal's day: the route's, else today.
fn journal_date(route: &Route, today: chrono::NaiveDate) -> String {
    route.date.clone().unwrap_or_else(|| today.format("%Y-%m-%d").to_string())
}

/// The `Ui` for a route: every view in its tab, what the route opens open over its view, the
/// keys. `today` resolves a Journal or a Day without a date.
pub fn build(route: &Route, today: chrono::NaiveDate) -> Ui {
    let mut data = BTreeMap::new();
    data.insert("frame".to_string(), source("frame", json!({})));
    let tab = route.tab();
    let mut store = BTreeMap::new();
    store.insert("done_here".into(), json!([]));
    store.insert("journal_date".into(), journal_date(route, today).into());
    // What agents just changed (`live`: id → true, the `•` gutter), the transaction `u` undoes
    // and the one `U` redoes, and the Log's filter (`L`: agents).
    store.insert("live".into(), json!({}));
    store.insert("agent_tx".into(), "".into());
    store.insert("redo_tx".into(), "".into());
    store.insert("redo_of".into(), "".into());
    store.insert("redo_word".into(), "redo".into());
    store.insert("log_by".into(), "".into());
    // Today's sections or agenda (`w`), the detail pane on or off (`\`), and what the move
    // picker and the delete question are about.
    store.insert("today_mode".into(), "".into());
    // The view ⌥M left, for Esc to come back to (null: none).
    store.insert("return_tab".into(), Value::Null);
    store.insert("return_open".into(), Value::Null);
    // The active panel (its id) and what in it takes the focus (`panel-…`, `rows-…`, `head-…`).
    store.insert("sidebar_active".into(), "".into());
    store.insert("sidebar_target".into(), "".into());
    // The sidebar's stack (sidebar::Stack), whether it shows, and whether it's beside the view.
    store.insert("sidebar".into(), crate::sidebar::Stack::default().to_value());
    store.insert("sidebar_shown".into(), false.into());
    store.insert("sidebar_beside".into(), false.into());
    data.insert("sidebar_rows".into(), source("sidebar", json!({"stack": "{store.sidebar}", "active": "{store.sidebar_active}", "width": "{$screen.width}", "height": "{$screen.height}"})));
    store.insert("no_detail".into(), "".into());
    for k in ["ask_id", "ask_text"] {
        store.insert(k.into(), "".into());
    }
    if let Some((v, what)) = opened(route, today) {
        store.insert(open_key(v), what);
    }
    let globals = global_binds();
    let children: Vec<Node> = View::TABS
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let base = if i == tab && View::TABS.contains(&route.view) { route.clone() } else { Route::to(*v) };
            let mut groups: Groups = Vec::new();
            let (under, view_bar) = tab_view(&base, today, &mut data, &mut store, &mut groups);
            let over = layers(*v, &mut data);
            // A modal layer clears what it covers.
            let mut all = vec![under];
            all.extend(over);
            // The view (and what's open over it) with the sidebar beside it, or over it when narrow.
            let (column, drawer, replace) = sidebar_places(*v);
            let rule = when(sized(node(Kind::Rule { glyph: Some("│".into()) }), cells(1)), &format!("$screen.width>=120 & {BESIDE}"));
            let main = sized(node(Kind::Stack { children: all }), spec("*"));
            let beside = node(Kind::Stack { children: vec![row(vec![main, rule, column]), drawer, replace] });
            // The bar spans the width under them all (sidebar.md §0, frame B).
            let mut content = tab_keys(col(vec![beside, view_bar]), i);
            // ⌥S, ⌥⇧T, ⌥\ from anywhere in the tab, writing included.
            let anywhere = everywhere_binds(*v);
            content = with_keys_everywhere(content, &anywhere);
            groups.push(("sidebar".into(), anywhere));
            // In a page or a day (here, or in a panel): its links and its lists.
            groups.push(("writing".into(), link_binds(*v)));
            // The view's help and palette, over everything in its tab.
            let mut everywhere: Vec<Bind> = overlay_binds(*v).into_iter().zip(["help", "commands"]).map(|((k, a), w)| b(&k, w, a)).collect();
            everywhere.extend(globals.iter().cloned());
            groups.push(("everywhere".into(), everywhere));
            let (help, cmds) = help_rows(&groups);
            store.insert(format!("keys_{}", v.name()), help);
            store.insert(format!("cmds_{}", v.name()), cmds);
            node(Kind::Stack { children: vec![content, help_overlay(*v), palette_overlay(*v), move_overlay(*v, &mut data), confirm_overlay(*v)] })
        })
        .collect();
    let focus = first_focus(route);
    let titles = ["Today", "Inbox{inbox|? <text>$</>}", "Tasks", "Pages", "Journal", "Search", "Log"].map(String::from).to_vec();
    let views = id(
        bound(
            node(Kind::Tabs {
                tabs: titles,
                children,
                tab: Some(tab),
                lead: Some(" <muted>[</><accent+b>•</><muted>]</> <muted>thc</> ".into()),
                trail: Some("<muted>{date}</> ".into()),
            }),
            "frame",
        ),
        "views",
    );
    // Below 60×24 (§3) only one centred line shows: two of them, as `when` has no "or". Each
    // is a modal layer over the views, so it clears them and keys don't reach them. It shows
    // only once the terminal has said its size (`>0`): a real screen gets its UI before its
    // size, and views hidden until then would drop the route's focus (a page's editor) for the
    // first list.
    let small = |cond: &str| {
        let mut t = bound(text("<muted>thc needs at least 60×24 · now {width}×{height}</>"), "$screen");
        t.align = Some("center".into());
        t.float = Some(Float { width: None, height: Some(cells(1)), at: Some("center".into()), margin: None });
        let mut layer = node(Kind::Stack { children: vec![text(""), t] });
        layer.float = Some(Float { width: None, height: None, at: None, margin: None });
        layer.modal = true;
        when(layer, &format!("$screen.width>0 & {cond}"))
    };
    let mut bar_prompt = prompt();
    bar_prompt.when = Some("prompt".into());
    let root = node(Kind::Stack { children: vec![views, bar_prompt, small("$screen.width<60"), small("$screen.width>=60 & $screen.height<24")] });
    Ui { root, data, store, keys: key_map(&globals), focus, status_bar: Some(false), ..Ui::default() }
}

// ---- the sidebar (sidebar.md) ----------------------------------------------------------------------

/// The sidebar has panels and shows.
const BESIDE: &str = "sidebar.open & sidebar_shown";

/// An act on the stack (sidebar.md §3): `op` on a panel (`{$item.…}`: the one its node is in),
/// against the stack as it is, from tab `v` (the focus follows the panel there).
fn panel_op(v: View, op: &str) -> Action {
    stack_act("panel", json!({"op": op, "key": "{$item.key}", "at": "{$item.at}", "stack": "{store.sidebar}", "shown": "{store.sidebar_shown}", "view": v.name()}))
}

/// A day panel to another day: `by` days, or null for today.
fn day_op(v: View, by: Value) -> Action {
    stack_act("panel", json!({"op": "day", "by": by, "key": "{$item.key}", "stack": "{store.sidebar}", "shown": "{store.sidebar_shown}", "view": v.name()}))
}

/// `⌥S`: the sidebar shows and its active panel has the focus, both at once, here in the UI
/// (`sidebar_target` ends its node ids; of the three places only the one showing takes it), so a
/// key pressed right after lands in the panel (S02c). The act records the active panel, or with
/// none it knows, makes the top one active and focuses it.
fn to_sidebar(v: View) -> Action {
    let mut steps = vec![Action::Put { put: "sidebar_shown".into(), value: true.into(), for_ms: None }];
    for place in crate::sidebar::PLACES {
        steps.push(Action::Focus { focus: format!("{place}_{}_{{store.sidebar_target}}", v.name()) });
    }
    steps.push(act("panel", json!({"op": "focus", "id": "{store.sidebar_active}", "stack": "{store.sidebar}", "view": v.name()})));
    Action::Do { steps }
}

/// `⌥J` `⌥K` in a panel at `place`: the focus to the next or previous panel (`to`: `next` or
/// `prev`, from the sidebar's rows), at once; the act makes it the active one.
fn step_panel(v: View, place: &str, to: &str) -> Action {
    Action::Do {
        steps: vec![
            Action::Focus { focus: format!("{place}_{}_{{$item.{to}}}", v.name()) },
            act("panel", json!({"op": "focus", "key": format!("{{$item.{to}_key}}"), "stack": "{store.sidebar}", "view": v.name()})),
        ],
    }
}

/// Open a row's page or day aside (§2: `o`, `⌥O`, `⇧Enter`). Narrow, the drawer (or replace)
/// opens at once and the new panel takes the focus (§2 rule 1, §6.4); beside, it stays.
fn aside_row(v: View) -> Action {
    Action::Do {
        steps: vec![
            Action::Put { put: "sidebar_shown".into(), value: true.into(), for_ms: None },
            stack_act("aside", json!({"id": "{id}", "stack": "{store.sidebar}", "view": v.name()})),
        ],
    }
}

/// The keys that work wherever the focus is in a tab, writing included (`⌥` chords type
/// nothing): `⌥S` to the sidebar (showing it), `⌥⇧T` reopen, `⌥\` hide or show.
fn everywhere_binds(v: View) -> Vec<Bind> {
    vec![
        b("a-s", "sidebar", to_sidebar(v)),
        b("a-s-t", "reopen a panel", stack_act("panel", json!({"op": "reopen", "stack": "{store.sidebar}", "shown": "{store.sidebar_shown}"}))),
        b("a-\\", "sidebar hide / show", stack_act("panel", json!({"op": "toggle", "stack": "{store.sidebar}", "shown": "{store.sidebar_shown}"}))),
    ]
}

/// Back to the main view from the sidebar (§5.1): its list, or what's open over it. The column
/// stays; the drawer and replace close.
fn sidebar_back(v: View, place: &str) -> Action {
    let mut steps = Vec::new();
    if place != "sidebar" {
        steps.push(Action::Put { put: "sidebar_shown".into(), value: "".into(), for_ms: None });
    }
    // The last that's there wins: the view's list, its detail, its page or day, its outline.
    for id in [main_id(v), format!("{}_detail", v.name()), format!("{}_doc", v.name()), "pages_outline_rows".into()] {
        steps.push(Action::Focus { focus: id });
    }
    Action::Do { steps }
}

/// The keys of a panel, on its header and its body alike (§5.3): `⌥J` `⌥K` the next and
/// previous panel, `⌥C` fold, `⌥W` close, `⌥P` pin, `⌥⇧K` `⌥⇧J` move, `⌥M` to main, `⌥S` and
/// Esc back to main.
fn panel_binds(v: View, place: &str) -> Vec<Bind> {
    let back = sidebar_back(v, place);
    vec![
        b("a-j", "next panel", step_panel(v, place, "next")),
        b("a-k", "previous panel", step_panel(v, place, "prev")),
        b("a-c", "fold", panel_op(v, "fold")),
        b("a-w", "close", panel_op(v, "close")),
        b("a-p", "pin / unpin", panel_op(v, "pin")),
        b("a-s-k", "move up", panel_op(v, "up")),
        b("a-s-j", "move down", panel_op(v, "down")),
        b("a-m", "to main", stack_act("to_main", json!({"key": "{$item.key}", "stack": "{store.sidebar}", "from": View::TABS.iter().position(|t| *t == v).unwrap_or(0)}))),
        b("a-s-t", "reopen a panel", stack_act("panel", json!({"op": "reopen", "stack": "{store.sidebar}", "shown": "{store.sidebar_shown}"}))),
        // A day panel to the day before or after (§8.1: ⌃P ⌃N, ⌥[ ⌥]).
        b("c-p", "the day before", day_op(v, json!(-1))),
        b("c-n", "the day after", day_op(v, json!(1))),
        b("a-[", "the day before", day_op(v, json!(-1))),
        b("a-]", "the day after", day_op(v, json!(1))),
        b("a-s", "back to main", back.clone()),
        b("esc", "back to main", back),
    ]
}

/// One panel (§4): its header row, then its body unless folded: a page or a day as an editor on
/// its hosted entry (the same entry the main view writes in, so one text and one undo), or a
/// list panel's rows. A template, one instance per panel (`each`); ids carry the place, the tab
/// and the panel.
fn panel_template(v: View, place: &str) -> Node {
    let pre = format!("{place}_{}", v.name());
    let binds = panel_binds(v, place);
    // The header: one row, on `selection` when it has the focus (a list's mark shows on its
    // selected row focused or not, so it has none here); Enter folds.
    let mut head = node(Kind::List {
        items: vec![json!({"id": "{$item.id}", "m": "{$item.header_m}"})],
        rows: Rows { item: Some("{m|markup}".into()), mark: Some(" ".into()), lead: Some(" ".into()), ..Rows::default() },
    });
    head.id = Some(format!("{pre}_head-{{$item.id}}"));
    head.keys.extend(key_map(&binds));
    head.keys.insert("enter".into(), panel_op(v, "fold"));
    for (k, by) in [("[", json!(-1)), ("]", json!(1)), ("T", Value::Null)] {
        head.keys.insert(k.into(), day_op(v, by));
    }
    // A click on the header folds it (§12).
    head.click = Some(panel_op(v, "fold"));
    let head = sized(head, cells(1));
    // A page or a day: the editor (Tab nests, ⌃T cycles a line, links as in the main view).
    let editor = Kind::Editor { text: String::new(), doc: Some("{$item.doc}".into()), placeholder: Some("Type to start.".into()), syntax: Some("markdown".into()), readonly: false, tab: Some("indent-lines".into()), tasks: Some("cycle".into()), caret: None, links: doc_links(v), ordered: ordered() };
    let mut doc = id(node(editor), &format!("{pre}_panel-{{$item.id}}"));
    doc.keys.extend(key_map(&binds));
    doc.keys.extend(key_map(&link_binds(v)));
    doc.pad = Some(Pad::X(2));
    let doc = when(sized(doc, spec("*")), "sidebar_rows.show.{$item.id}.doc");
    // A list: its rows, with the row keys (x done, o aside…), and the panel's.
    let mut rows_ = keys(tree(&format!("{pre}_rows-{{$item.id}}"), "sidebar_rows.lists.{$item.id}"), node_keys(v));
    rows_.keys.remove("enter");
    rows_.keys.insert("enter".into(), act("to_main_node", json!({"id": "{id}", "from": View::TABS.iter().position(|t| *t == v).unwrap_or(0)})));
    rows_.keys.extend(key_map(&binds));
    let rows_ = when(sized(rows_, spec("*")), "sidebar_rows.show.{$item.id}.list");
    let mut panel = col(vec![head, doc, rows_]);
    panel.size = Some(spec("{$item.total}"));
    panel
}

/// The stack, a panel per item (§3); `nothing beside you` when there's none.
fn sidebar_panels(v: View, place: &str) -> Node {
    let each = bound(node(Kind::Each { template: Box::new(panel_template(v, place)), row: false, gap: Some(1) }), "sidebar_rows.rows");
    let mut c = col(vec![each, sized(text(""), spec("*"))]);
    c.id = None;
    c
}

/// The sidebar's three places in a tab (§6.1, §6.4): the column beside the view (a third of the
/// width, 40–64), the drawer over its right 60 columns, replace over all of it. The same stack
/// in each; they differ only in what Esc does.
fn sidebar_places(v: View) -> (Node, Node, Node) {
    let mut column = sized(sidebar_panels(v, "sidebar"), spec("33%"));
    column.min = Some(40);
    column.max = Some(64);
    let column = when(column, &format!("$screen.width>=120 & {BESIDE}"));
    // The drawer: a `│` left edge in `line`, the stack on `raised` (§6.4). Narrow, it shows
    // while `sidebar_shown` (an open puts it there at once, so the open can focus it).
    let edge = sized(node(Kind::Rule { glyph: Some("│".into()) }), cells(1));
    let mut drawer = variant(row(vec![edge, sidebar_panels(v, "drawer")]), "overlay");
    drawer.float = Some(Float { width: Some(cells(60)), height: None, at: Some("right".into()), margin: None });
    let drawer = when(drawer, "$screen.width>=90 & $screen.width<120 & sidebar_shown");
    let back_row = sized(bound(text(" <accent>‹</> <muted+u>main</>   <muted>beside it: {count} panels</>"), "sidebar_rows"), cells(1));
    let mut replace = col(vec![back_row, sidebar_panels(v, "replace")]);
    replace.float = Some(Float { width: None, height: None, at: None, margin: None });
    let replace = when(replace, "$screen.width<90 & sidebar_shown");
    (column, drawer, replace)
}

// ---- links in documents (sidebar.md §12.1, editing.md E63–E69) -----------------------------------

/// How a document's links act: a click on `[[Page]]` follows it in the main view (saving
/// first; Esc comes back), a ⇧-click opens it aside; a tag only places the caret on a click,
/// and a ⇧-click opens its list aside. Addresses place the caret (the spec has nothing more).
fn doc_links(v: View) -> Vec<fictty::model::LinkSpec> {
    let from = View::TABS.iter().position(|t| *t == v).unwrap_or(0);
    vec![
        fictty::model::LinkSpec { kind: "wiki".into(), click: Some(follow(v, from, "{$match}")), s_click: Some(aside_title(v, "page", "{$match}")) },
        fictty::model::LinkSpec { kind: "tag".into(), click: None, s_click: Some(aside_title(v, "tag", "{$match}")) },
        fictty::model::LinkSpec { kind: "url".into(), click: None, s_click: None },
    ]
}

/// The keys of a page or day being written in (main view or a panel): ⌃O follows the link at the
/// caret and ⌥O opens it aside (E66); ⌥L cycles the caret's list through number, letter, roman
/// and bullet.
fn link_binds(v: View) -> Vec<Bind> {
    let from = View::TABS.iter().position(|t| *t == v).unwrap_or(0);
    vec![
        b("c-o", "follow the link", follow(v, from, "{$link}")),
        b("a-o", "the link aside", aside_title(v, "link", "{$link}")),
        b("a-l", "list style: 1. a. i. -", Action::ListStyle { list_style: "next".into() }),
    ]
}

/// Ordered lists in pages and days, by depth (thc_core::list's depth sequence, so what the
/// editor numbers is what the vault renders): `1.`, then `a.`, then `i.`.
fn ordered() -> Vec<String> {
    (1..=3).map(|d| thc_core::list::ListStyle::for_depth(d).as_str()).collect()
}

/// Follow a link in the main view: the page, open to write in, and Esc back to where it was.
fn follow(v: View, from: usize, title: &str) -> Action {
    act("follow", json!({"title": title, "from": from, "back": format!("{{store.{}}}", open_key(v))}))
}

/// Open a link's page (or a tag's list) aside.
fn aside_title(v: View, kind: &str, title: &str) -> Action {
    Action::Do {
        steps: vec![
            Action::Put { put: "sidebar_shown".into(), value: true.into(), for_ms: None },
            stack_act("aside", json!({"title": title, "as": kind, "stack": "{store.sidebar}", "view": v.name()})),
        ],
    }
}

/// A tab's view (what layers open over), the sources it reads, the store it seeds, and its
/// bindings by group (for its help and palette).
fn tab_view(route: &Route, today: chrono::NaiveDate, data: &mut BTreeMap<String, Source>, store: &mut BTreeMap<String, Value>, groups: &mut Groups) -> (Node, Node) {
    // A list's feed also reads `live`, so the rows an agent just changed carry their `•`.
    let mut feed = |name: &str, args: Value| {
        let mut args = args;
        args["live"] = "{store.live}".into();
        data.insert(name.to_string(), source(name, args));
    };
    let v = route.view;
    match route.view {
        View::Inbox => {
            feed("inbox", json!({}));
            let binds = node_binds(v);
            // The Triage keys (§5.2), under the detail at 120+: the same bindings the rows have.
            let triage: Vec<Value> = ["m", "t", "d", "x", "D"]
                .iter()
                .filter_map(|k| binds.iter().find(|x| x.key == *k))
                .map(|x| json!({"id": format!("triage:{}", x.key), "key": x.key, "words": x.words}))
                .collect();
            store.insert("triage".into(), Value::Array(triage));
            groups.push(("on a row".into(), binds));
            let l = keys(list("inbox", "inbox.rows", rows(node_item())), node_keys(View::Inbox));
            let tri = Rows { item: Some("<text>{key:5}</><muted>{words:18}</>".into()), mark: Some(String::new()), lead: Some(String::new()), ..Rows::default() };
            let mut triage = bound(node(Kind::List { items: vec![], rows: tri }), "triage");
            triage.pad = Some(Pad::X(1));
            let head = sized(text(" <text+b>Triage</>"), cells(1));
            // ⌃W goes into the detail lines and back.
            let mut lines = pane("inbox", "detail", data);
            lines.keys.insert("c-w".into(), Action::Focus { focus: "inbox".into() });
            let mut l = l;
            l.keys.insert("c-w".into(), Action::Focus { focus: "inbox_pane".into() });
            // The node (as far as 14 rows of it), then Triage right under it.
            let mut lines = sized(lines, spec("*"));
            lines.max = Some(14);
            let side = col(vec![lines, head, sized(triage, cells(6)), sized(text(""), spec("*"))]);
            view(split(l, side), "inbox", &[("m", "move"), ("t", "task"), ("d", "date"), ("x", "done"), ("D", "delete")])
        }
        View::Tasks => {
            // The filter is the query: the table follows what's typed. The rows have the focus;
            // `f` edits the filter (§6.3), Enter or Esc goes back to the rows.
            store.insert("tasks_q".into(), route.query.clone().unwrap_or_else(|| "status:open".into()).into());
            feed("tasks", json!({"query": "{store.tasks_q}", "keep": "{store.done_here}"}));
            let done = || Action::Do { steps: vec![Action::Put { put: "tasks_edit".into(), value: "".into(), for_ms: None }, Action::Focus { focus: "$prev".into() }] };
            let mut field = when(input("tasks_q", "", "a thc query"), "tasks_edit");
            field.keys.insert("enter".into(), done());
            field.keys.insert("esc".into(), done());
            let shown = when(bound(text("{filter_m|markup}"), "tasks"), "!tasks_edit");
            let filter = variant(sized(row(vec![sized(text(" <accent+b>q›</> "), cells(4)), shown, field, sized(bound(text("\t<muted>{count}</> "), "tasks"), cells(12))]), cells(1)), "input");
            let edit = Action::Do { steps: vec![Action::Put { put: "tasks_edit".into(), value: true.into(), for_ms: None }, Action::Focus { focus: "tasks_q".into() }] };
            let col_ = |title: &str, value: &str, size: Size| Column { title: title.into(), value: value.into(), size: Some(size), align: None, heat: None, total: None };
            let mut table = keys(
                id(
                    bound(
                        node(Kind::Table {
                            columns: vec![
                                col_("id", "<muted>{short}</>", cells(9)),
                                col_("task", &format!("{STATUS}{{task_m|markup}}"), spec("*")),
                                col_("due", "{due_m|markup}", cells(17)),
                                col_("sched", "{sched_m|markup}", cells(11)),
                                col_("pri", "{pri_m|markup}", cells(6)),
                                col_("where", "{where_m|markup}", cells(14)),
                                col_("by", "{by_m|markup}", cells(10)),
                            ],
                            items: vec![],
                            filter: None,
                        }),
                        "tasks.rows",
                    ),
                    "tasks",
                ),
                node_keys(View::Tasks),
            );
            let mut narrow = keys(list("tasks-list", "tasks.rows", rows(format!("<muted>{{short}}</>  {STATUS}{{task_m|markup}}\t{{due_m|markup}} "))), node_keys(View::Tasks));
            let own = vec![b("f", "edit the filter", edit)];
            table.keys.extend(key_map(&own));
            narrow.keys.extend(key_map(&own));
            groups.push(("Tasks".into(), own));
            groups.push(("on a row".into(), node_binds(v)));
            let content = col(vec![filter, sized(text(""), cells(1)), when(table, WIDE), when(narrow, "$width<120")]);
            view(content, "tasks", &[("f", "filter"), ("x", "done"), ("enter", "open")])
        }
        View::Pages => {
            feed("pages", json!({}));
            let mut r = rows("<muted>{short}</>  {title_m|markup}\t{meta_m|markup} ".into());
            r.variants.insert("day".into(), "{title_m|markup}\t{meta_m|markup} ".into());
            r.filter = Some("pages_q".into());
            // Enter writes in the page or day, over the list (which keeps its selection and query).
            let open_row = open_over(View::Pages, json!({"kind": "{open}", "id": "{id}", "key": "{doc}"}), "pages_doc");
            let mut l = list("pages", "pages.rows", r);
            let own = vec![b("enter", "write in it", open_row.clone()), b("a-o", "open aside", aside_row(View::Pages)), b("s-enter", "open aside", aside_row(View::Pages))];
            l.keys.extend(key_map(&own));
            let (l, prompt) = finder(l, variant(input("pages_q", "", "type to find a page, tag or day"), "input"), open_row);
            groups.push(("Pages".into(), own));
            let finder_row = variant(sized(row(vec![sized(text(" <accent+b>›</> "), cells(3)), prompt]), cells(1)), "input");
            // The preview: the highlighted page or day, read-only, from the same host entry its
            // editor writes (`doc`), reloaded when the vault changes.
            let mut preview = bound(node(Kind::Editor { text: String::new(), doc: Some("{doc}".into()), placeholder: Some(String::new()), syntax: Some("markdown".into()), readonly: true, tab: None, tasks: None, caret: None, links: vec![], ordered: vec![] }), "@pages");
            preview.id = Some("preview".into());
            preview.pad = Some(Pad::X(1));
            let content = over_list(finder_row, col(vec![sized(text(""), cells(2)), split(l, preview)]));
            view(content, "pages", &[("type", "find"), ("enter", "write"), ("tab", "next view")])
        }
        View::Journal => {
            // The day is the store's `journal_date`: [ ] T move it (the host works out the date),
            // the feed follows; no new UI.
            feed("journal", json!({"date": "{store.journal_date}"}));
            let title = sized(bound(text("{title_m|markup}\t<text>[</> <muted>prev</>  <text>]</> <muted>next</>  <text>T</> <muted>today</> "), "journal"), cells(1));
            // Row 3 the day strip, row 4 the accent under the shown day, entries from row 6 (§5.5).
            let strip = sized(bound(text("{strip_m|markup}"), "journal"), cells(1));
            let under = sized(bound(text("{under_m|markup}"), "journal"), cells(1));
            let mut calendar = list("", "journal.calendar", Rows { item: Some("{m|markup}".into()), mark: Some(String::new()), lead: Some(String::new()), ..Rows::default() });
            calendar.id = None;
            calendar.pad = Some(Pad::X(1));
            // Enter writes in the day shown; x and X still act on the row.
            let mut entries = keys(tree("journal", "journal.rows"), node_keys(View::Journal));
            let mut own = vec![b("enter", "write in the day", open_over(View::Journal, json!({"kind": "day", "key": "day/{store.journal_date}"}), "journal_doc"))];
            for (key, by, words) in [("[", json!(-1), "the day before"), ("]", json!(1), "the day after"), ("T", json!("today"), "today")] {
                own.push(b(key, words, act("day", json!({"from": "{store.journal_date}", "by": by}))));
            }
            entries.keys.extend(key_map(&own));
            groups.push(("Journal".into(), own));
            groups.push(("on a row".into(), node_binds(v).into_iter().filter(|x| x.key != "enter").collect()));
            let _ = today;
            let content = col(vec![title, strip, under, sized(text(""), cells(1)), split(entries, calendar)]);
            view(content, "journal", &[("enter", "write"), ("[", "prev"), ("]", "next")])
        }
        View::Search => {
            // The results follow what's typed.
            store.insert("search_q".into(), route.query.clone().unwrap_or_default().into());
            feed("search", json!({"query": "{store.search_q}"}));
            // A finder: letters find, so the rows have no single-letter commands (§6).
            let open_row = open_over(View::Search, json!({"kind": "detail", "id": "{id}"}), "search_detail");
            let mut results = tree("search", "search.rows");
            // ⌃Q: the search as a Tasks filter (§5.6).
            let as_query = Action::Do { steps: vec![Action::Put { put: "tasks_q".into(), value: "text:{store.search_q}".into(), for_ms: None }, show(2)] };
            let own = vec![b("enter", "open", open_row.clone()), b("c-q", "as a Tasks filter", as_query.clone()), b("a-o", "open aside", aside_row(View::Search)), b("s-enter", "open aside", aside_row(View::Search))];
            results.keys.extend(key_map(&own));
            let (results, mut field) = finder(results, variant(input("search_q", "", "search every note"), "input"), open_row);
            field.keys.insert("c-q".into(), as_query);
            groups.push(("Search".into(), own));
            let bar_row = variant(sized(row(vec![sized(text(" <accent+b>/</> "), cells(3)), field, sized(bound(text("\t{count_m|markup} "), "search"), cells(32))]), cells(1)), "input");
            let hint = sized(text(" <muted>deleted nodes are searchable too: add is:deleted</>"), cells(1));
            let content = over_list(bar_row, col(vec![sized(text(""), cells(2)), split(results, pane_with("search", "context", json!({"id": "{@search.id}", "query": "{store.search_q}"}), data)), hint]));
            view(content, "search", &[("type", "find"), ("enter", "open"), ("tab", "next view")])
        }
        View::Log => {
            // `L` (anywhere) and `@` here filter it to agents' transactions and back.
            feed("log", json!({"by": "{store.log_by}"}));
            let title = sized(bound(text("{title_m|markup}\t<muted>last 24h · {who}</>   <text>@</> <muted>{other}</> "), "log"), cells(1));
            // Two lines a transaction (§5.7): time, who, tx and summary; then what it did.
            let line2 = "\n                          <muted>{detail}</>";
            let mut r = Rows {
                item: Some(format!("<muted>{{time}}</>  <muted>{{who:8}}</>  <muted>{{tx}}</>   {{summary_m|markup}}{line2}")),
                lines: Some(2),
                ..rows(String::new())
            };
            r.variants.insert("agent".into(), format!("<muted>{{time}}</>  <agent>{{who:8}}</>  <muted>{{tx}}</>   {{summary_m|markup}}{line2}"));
            let mut l = list("log", "log.rows", r);
            // u undoes the selected transaction (a new one, §5.7); U redoes what it undid.
            let own = vec![
                b("u", "undo this transaction", act("undo", json!({"tx": "{id}"}))),
                b("U", "redo", redo()),
                b("@", "agents / everyone", flip("log_by", "", "agents")),
            ];
            l.keys.extend(key_map(&own));
            groups.push(("Log".into(), own));
            let content = col(vec![title, sized(text(""), cells(1)), split(l, pane("log", "tx", data))]);
            view(content, "log", &[("u", "undo"), ("@", "who")])
        }
        // Today, and anything that isn't a tab of its own.
        _ => {
            // `w` swaps the sections for the agenda (§5.1); one list, its feed says which.
            feed("today", json!({"mode": "{store.today_mode}"}));
            let own = vec![b("w", "agenda / sections", flip("today_mode", "", "agenda"))];
            groups.push(("Today".into(), own.clone()));
            groups.push(("on a row".into(), node_binds(View::Today)));
            let mut t = keys(tree("today", "today.rows"), node_keys(View::Today));
            t.keys.extend(key_map(&own));
            view(split(t, pane("today", "detail", data)), "today", &[("a", "add"), ("w", "agenda"), ("p", "priority"), ("d", "date")])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_view_builds_and_round_trips_as_json() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
        for v in [View::Today, View::Inbox, View::Tasks, View::Pages, View::Journal, View::Search, View::Log, View::Detail, View::Outline, View::Page, View::Day] {
            let ui = build(&Route { view: v, id: Some("x".into()), ..Route::default() }, today);
            // A UI pushed over the socket or restored is the same UI.
            let back: Ui = serde_json::from_value(serde_json::to_value(&ui).unwrap()).unwrap();
            assert_eq!(ui, back, "{v:?}");
            assert!(ui.data.values().all(|s| s.host.is_some()), "{v:?}: every source is the host's");
            // Ids are unique: a key, a focus or a store entry names one node (per-tab overlays
            // name theirs by view).
            let mut all = Vec::new();
            ui.root.walk(&mut all);
            let mut seen = std::collections::HashSet::new();
            for id in all.iter().filter_map(|n| n.id.as_deref()) {
                assert!(seen.insert(id), "{v:?}: id {id} twice");
            }
        }
        assert_eq!(View::parse("Journal"), Some(View::Journal));
        assert_eq!(Route { view: View::Day, back: Some(View::Pages), ..Route::default() }.tab(), 3);
    }
}
