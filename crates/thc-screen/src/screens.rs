//! Screens: thc's views as one fictty `Ui`, built from Rust types. The seven views are the tabs
//! of one tabs node (its titles over the `frame` feed, the mark as its lead and the date as its
//! trail), so a click on a tab or a key switches views without a new UI. A Detail, an Outline, or a
//! page or day being written in takes the place of its tab's view. Every source is a host source
//! the feeds answer (`source`), every action a host action ThcHost does (`act`), so the UI value
//! never holds data: the same value renders a snapshot (`App::headless`) and runs live (`App::run`).
//!
//! The frame (tui-handoff.md §3): header and rule (the tabs), content, bar. Content splits
//! list | detail at 120 columns and up (`when: "$screen.width>=120"`), the list min(60%, 96).

use fictty::model::{Action, Column, Float, Kind, Node, Pad, Rows, Size, Source, Ui};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Which screen, and what it shows. Serializable: it travels as the `open` action's args, so a
/// key that opens a view is data like any other.
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
    /// Where Esc goes from a Detail, an Outline, a Page or a Day.
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

    /// Where Esc goes, if anywhere.
    fn back(&self) -> Option<Route> {
        match self.view {
            View::Outline | View::Page => Some(Route::to(self.back.unwrap_or(View::Pages))),
            View::Day => Some(match self.back {
                Some(View::Pages) => Route::to(View::Pages),
                _ => Route { view: View::Journal, date: self.date.clone(), ..Route::default() },
            }),
            View::Detail => Some(Route::to(self.back.unwrap_or(View::Today))),
            _ => None,
        }
    }
}

// ---- the one place sources and actions are named -------------------------------------------------

/// A data source: the host's feed `name` with `args` (templates: `{@today.id}` the selected
/// row's id, `{store.search_q}` what's typed; refetched when they change).
pub fn source(name: &str, args: Value) -> Source {
    Source { cmd: None, shell: None, stream: None, value: None, path: None, every: None, host: Some(name.into()), args }
}

/// A host action (`done`, `reopen`, `set`, `add`, `mv`, `open`, `answer`): args are
/// templates over the selected row (`{id}`), the store (`{store.key}`) and `$id`.
pub fn act(name: &str, args: Value) -> Action {
    Action::Host { host: name.into(), args }
}

/// Open a screen.
pub fn open(route: &Route) -> Action {
    act("open", serde_json::to_value(route).unwrap_or_default())
}

/// What a view focuses first: its list, or its input where typing comes first (§6.3).
pub fn first_focus(route: &Route) -> Option<&'static str> {
    Some(match route.view {
        View::Today => "today",
        View::Inbox => "inbox",
        View::Tasks => "tasks",
        View::Pages => "pages_q",
        View::Journal => "journal",
        View::Search => "search_q",
        View::Log => "log",
        View::Detail => return None,
        View::Outline => "outline",
        View::Page | View::Day => "doc",
    })
}

/// A page's or a day's store key (`page/<id>`, `day/<date>`): the host keeps those entries.
pub fn doc_key(route: &Route, today: chrono::NaiveDate) -> String {
    match route.view {
        View::Day => format!("day/{}", route.date.clone().unwrap_or_else(|| today.format("%Y-%m-%d").to_string())),
        _ => format!("page/{}", route.id.clone().unwrap_or_default()),
    }
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

fn input(id_: &str, seed: &str, placeholder: &str) -> Node {
    id(node(Kind::Input { text: seed.into(), placeholder: Some(placeholder.into()), accept: None }), id_)
}

const WIDE: &str = "$screen.width>=120";

/// A status cell (§4): `[ ]` `[/]` `[w]` `[x]` `[-]`, a note's ` · `, and Search's ` ¶ ` / ` # `.
const STATUS: &str = "{status|todo=<text>[ ]</> ;doing=<doing>[/]</> ;waiting=<waiting>[w]</> ;done=<done>[x]</> ;cancelled=<muted>[-]</> ;note=<muted> · </> ;page=<muted> ¶ </> ;tag=<muted> # </> }";

/// A node row (§4): id, status cell, text, and the meta right-aligned a cell from the edge.
fn node_item() -> String {
    format!("<muted>{{short}}</>  {STATUS}{{text_m|markup}}\t{{meta_m|markup}} ")
}

/// Rows of nodes, as every list and tree draws them: the `▌` gutter on the selected row, the
/// selection on `selection` and following the row's id, the text keeping 24 cells (§4.2) before
/// the meta gives way. Sections, empty states and blanks take no cursor.
fn rows(item: String) -> Rows {
    Rows {
        item: Some(item),
        variant: Some("kind".into()),
        variants: [
            ("section", "{title_m|markup}{count|?  <muted>$</>}\t{note_m|markup} "),
            ("empty", "  {m|markup}"),
            ("blank", ""),
            ("line", "{m|markup}"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect(),
        skip: ["section", "empty", "blank", "line"].map(String::from).to_vec(),
        mark: Some("<accent+b>▌</> ".into()),
        lead: Some("  ".into()),
        selected: Some("on-selection".into()),
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

/// The keys on rows of nodes:
/// - x done, X reopen, Enter the detail;
/// - p then h, m, l or - the priority (§6.2);
/// - d a due date and m a place, asked in the bar (the prompt recipe, `prompt`).
fn node_keys(back: View) -> BTreeMap<String, Action> {
    let mut k = BTreeMap::from([
        ("x".to_string(), act("done", json!({"id": "{id}"}))),
        ("X".to_string(), act("reopen", json!({"id": "{id}"}))),
        ("enter".to_string(), act("open", json!({"view": "detail", "id": "{id}", "back": back.name()}))),
    ]);
    for (key, p) in [("h", "high"), ("m", "med"), ("l", "low"), ("-", "")] {
        k.insert(format!("p {key}"), act("set", json!({"id": "{id}", "props": {"priority": p}})));
    }
    for (key, ask) in [("d", "due"), ("m", "move")] {
        k.insert(
            key.into(),
            Action::Do {
                steps: vec![
                    Action::Put { put: "ask_id".into(), value: "{id}".into() },
                    Action::Put { put: "prompt".into(), value: ask.into() },
                    Action::Focus { focus: "prompt_in".into() },
                ],
            },
        );
    }
    k
}

/// The bar prompt (a recipe, §3.3's "bar prompts"): a line floated over the bar while the store's
/// `prompt` is set, its input's Enter answering the host with what was typed and the row it was
/// asked about, Esc closing it. Closing puts the focus back where it was (`$prev`).
fn prompt() -> Node {
    let clear = || {
        vec![
            Action::Put { put: "prompt".into(), value: "".into() },
            Action::Put { put: "prompt_in".into(), value: "".into() },
            Action::Focus { focus: "$prev".into() },
        ]
    };
    let mut answer = vec![act("answer", json!({"ask": "{store.prompt}", "id": "{store.ask_id}", "text": "{store.prompt_in}"}))];
    answer.extend(clear());
    let mut field = variant(input("prompt_in", "", "fri, +3d, 2026-10-20 · a page title"), "input");
    field.keys.insert("enter".into(), Action::Do { steps: answer });
    field.keys.insert("esc".into(), Action::Do { steps: clear() });
    let label = sized(bound(text(" {.|due=<accent+b>due ›</>;move=<accent+b>move to ›</>} "), "prompt"), cells(11));
    let mut line = variant(row(vec![variant(label, "input"), field]), "input");
    line.when = Some("prompt".into());
    line.float = Some(Float { width: None, height: Some(cells(1)), at: Some("bottom".into()), margin: Some(1) });
    line
}

/// The detail pane (§5.8): the lines of the node selected in `of`, from the `detail` feed (or
/// `tx` in the Log) with the selection's id as its argument.
fn pane(of: &str, feed: &str, data: &mut BTreeMap<String, Source>) -> Node {
    let name = format!("{of}_pane");
    data.insert(name.clone(), source(feed, json!({"id": format!("{{@{of}.id}}")})));
    let mut lines = list(&name, &format!("{name}.lines"), lines_rows());
    // A pane shows; it takes no cursor.
    lines.id = None;
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
    let mut main = sized(main, spec("*"));
    main.max = Some(96);
    let rule = sized(node(Kind::Rule { glyph: Some("│".into()) }), cells(1));
    row(vec![main, when(rule, WIDE), when(sized(side, spec("40%")), WIDE)])
}

/// The bar (§3.3): the view's name and counts on the left (`bar_m` of its feed), the daemon's
/// dot and the view's hints on the right.
fn bar(feed: &str, hints: &[(&str, &str)]) -> Node {
    let right = format!(" {{live|true=<done>●</> <muted>live</>;*=<muted>○ local</>}}   {} ", crate::feeds::hints(hints, "  "));
    let plain: String = hints.iter().map(|(k, w)| format!("{k} {w}")).collect::<Vec<_>>().join("  ");
    let width = unicode_width::UnicodeWidthStr::width(format!(" ○ local   {plain} ").as_str()) as u16;
    sized(
        row(vec![variant(bound(text(" {bar_m|markup}"), feed), "bar"), variant(sized(bound(text(&format!("\t{right}")), "frame"), cells(width)), "bar")]),
        cells(1),
    )
}

/// One view's place in the tabs: its content above its bar.
fn view(content: Node, feed: &str, hints: &[(&str, &str)]) -> Node {
    col(vec![content, bar(feed, hints)])
}

// ---- the views ------------------------------------------------------------------------------------

/// The `Ui` for a route: every view in its tab, the route's own content in its tab, the keys.
/// `today` resolves a Journal or a Day without a date.
pub fn build(route: &Route, today: chrono::NaiveDate) -> Ui {
    let mut data = BTreeMap::new();
    data.insert("frame".to_string(), source("frame", json!({})));
    let tab = route.tab();
    let mut store = BTreeMap::new();
    let children: Vec<Node> = View::TABS
        .iter()
        .enumerate()
        .map(|(i, v)| tab_view(&if i == tab { route.clone() } else { Route::to(*v) }, today, &mut data, &mut store))
        .collect();
    let focus = first_focus(route).map(String::from);
    let titles = ["Today", "Inbox{inbox|? <text>$</>}", "Tasks", "Pages", "Journal", "Search", "Log"].map(String::from).to_vec();
    let views = id(
        bound(
            node(Kind::Tabs {
                tabs: titles,
                children,
                // The route's tab first; an `open` lands in the tab it came from, so a push keeps it.
                tab: Some(tab),
                lead: Some(" <muted>[</><accent+b>•</><muted>]</> <muted>thc</> ".into()),
                trail: Some("<muted>{date}</> ".into()),
            }),
            "frame",
        ),
        "views",
    );
    let root = node(Kind::Stack { children: vec![views, prompt()] });
    let show = |tab: usize| Action::ShowTab { tab, of: "views".into() };
    let mut keys: BTreeMap<String, Action> = (0..View::TABS.len()).map(|i| ((i + 1).to_string(), show(i))).collect();
    keys.insert("/".into(), show(5));
    keys.insert("t".into(), Action::Theme { theme: "next".into() });
    if let Some(back) = route.back() {
        keys.insert("esc".into(), open(&back));
    }
    // A Detail acts on its node from anywhere it has no row under the cursor.
    if let (View::Detail, Some(id_)) = (route.view, &route.id) {
        keys.insert("x".into(), act("done", json!({"id": id_})));
        keys.insert("X".into(), act("reopen", json!({"id": id_})));
    }
    Ui { root, data, store, keys, focus, status_bar: Some(false), ..Ui::default() }
}

/// A tab's content for a route, the sources it reads and the store it seeds.
fn tab_view(route: &Route, today: chrono::NaiveDate, data: &mut BTreeMap<String, Source>, store: &mut BTreeMap<String, Value>) -> Node {
    let mut feed = |name: &str, args: Value| {
        data.insert(name.to_string(), source(name, args));
    };
    match route.view {
        View::Today => {
            feed("today", json!({}));
            let t = keys(tree("today", "today.rows"), node_keys(View::Today));
            view(split(t, pane("today", "detail", data)), "today", &[("a", "add"), ("p", "priority"), ("d", "date")])
        }
        View::Inbox => {
            feed("inbox", json!({}));
            let l = keys(list("inbox", "inbox.rows", rows(node_item())), node_keys(View::Inbox));
            view(split(l, pane("inbox", "detail", data)), "inbox", &[("m", "move"), ("x", "done"), ("d", "date")])
        }
        View::Tasks => {
            // The filter is the query: the table follows what's typed. The rows have the focus;
            // `f` edits the filter (§6.3), Enter or Esc goes back to the rows.
            store.insert("tasks_q".into(), route.query.clone().unwrap_or_else(|| "status:open".into()).into());
            feed("tasks", json!({"query": "{store.tasks_q}"}));
            let done = || Action::Do { steps: vec![Action::Put { put: "tasks_edit".into(), value: "".into() }, Action::Focus { focus: "$prev".into() }] };
            let mut field = when(input("tasks_q", "", "a thc query"), "tasks_edit");
            field.keys.insert("enter".into(), done());
            field.keys.insert("esc".into(), done());
            let shown = when(bound(text("{filter_m|markup}"), "tasks"), "!tasks_edit");
            let filter = variant(sized(row(vec![sized(text(" <accent+b>q›</> "), cells(4)), shown, field, sized(bound(text("\t<muted>{count}</> "), "tasks"), cells(12))]), cells(1)), "input");
            let edit = Action::Do { steps: vec![Action::Put { put: "tasks_edit".into(), value: true.into() }, Action::Focus { focus: "tasks_q".into() }] };
            let col_ = |title: &str, value: &str, size: Size| Column { title: title.into(), value: value.into(), size: Some(size), align: None, heat: None, total: None };
            let table = keys(
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
            let narrow = keys(list("tasks-list", "tasks.rows", rows(format!("<muted>{{short}}</>  {STATUS}{{task_m|markup}}\t{{due_m|markup}} "))), node_keys(View::Tasks));
            let (mut table, mut narrow) = (table, narrow);
            table.keys.insert("f".into(), edit.clone());
            narrow.keys.insert("f".into(), edit);
            let content = col(vec![filter, sized(text(""), cells(1)), when(table, WIDE), when(narrow, "$screen.width<120")]);
            view(content, "tasks", &[("f", "filter"), ("x", "done"), ("enter", "open")])
        }
        View::Pages => {
            feed("pages", json!({}));
            let mut finder = variant(input("pages_q", "", "type to find a page, tag or day"), "input");
            finder.keys.insert("enter".into(), Action::Focus { focus: "pages".into() });
            // ↓ moves into the list, a row at a time (§6.3: ↑ ↓ move from the finder).
            finder.keys.insert("down".into(), Action::Do { steps: vec![Action::Focus { focus: "pages".into() }, Action::Named("down".into())] });
            let finder_row = variant(sized(row(vec![sized(text(" <accent+b>›</> "), cells(3)), finder]), cells(1)), "input");
            let mut r = rows("<muted>{short}</>  {title_m|markup}\t{meta_m|markup} ".into());
            r.variants.insert("day".into(), "{title_m|markup}\t{meta_m|markup} ".into());
            r.filter = Some("pages_q".into());
            let mut l = list("pages", "pages.rows", r);
            // Enter writes in the page or day; `o` shows a page's outline as rows.
            l.keys.insert("enter".into(), act("open", json!({"view": "{open}", "id": "{id}", "date": "{date}", "back": "pages"})));
            l.keys.insert("o".into(), act("open", json!({"view": "outline", "id": "{id}"})));
            // The preview: the highlighted page or day, read-only, from the same host entry its
            // editor writes (`doc`), reloaded when the vault changes.
            let mut preview = bound(node(Kind::Editor { text: String::new(), doc: Some("{doc}".into()), placeholder: Some(String::new()), syntax: Some("markdown".into()), readonly: true }), "@pages");
            preview.id = Some("preview".into());
            preview.pad = Some(Pad::X(1));
            let content = col(vec![finder_row, sized(text(""), cells(1)), split(l, preview)]);
            view(content, "pages", &[("enter", "write"), ("o", "outline"), ("↓", "list")])
        }
        View::Journal => {
            let date = route.date.as_deref().and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).unwrap_or(today);
            let day = |d: chrono::NaiveDate| open(&Route { view: View::Journal, date: Some(d.format("%Y-%m-%d").to_string()), ..Route::default() });
            feed("journal", json!({"date": date.format("%Y-%m-%d").to_string()}));
            let title = sized(bound(text("{title_m|markup}\t<text>[</> <muted>prev</>  <text>]</> <muted>next</>  <text>T</> <muted>today</> "), "journal"), cells(1));
            let strip = sized(bound(text("  {strip_m|markup}"), "journal"), cells(1));
            let mut calendar = list("", "journal.calendar", Rows { item: Some("{m|markup}".into()), mark: Some(String::new()), lead: Some(String::new()), ..Rows::default() });
            calendar.id = None;
            calendar.pad = Some(Pad::X(1));
            // Enter writes in the day shown; x and X still act on the row; [ ] and T move the day.
            let mut entries = keys(tree("journal", "journal.rows"), node_keys(View::Journal));
            entries.keys.insert("enter".into(), open(&Route { view: View::Day, date: Some(date.format("%Y-%m-%d").to_string()), ..Route::default() }));
            entries.keys.insert("[".into(), day(date - chrono::Duration::days(1)));
            entries.keys.insert("]".into(), day(date + chrono::Duration::days(1)));
            entries.keys.insert("T".into(), day(today));
            let content = col(vec![title, strip, sized(text(""), cells(1)), split(entries, calendar)]);
            view(content, "journal", &[("enter", "write"), ("[", "prev"), ("]", "next")])
        }
        View::Search => {
            // The results follow what's typed.
            store.insert("search_q".into(), route.query.clone().unwrap_or_default().into());
            feed("search", json!({"query": "{store.search_q}"}));
            let mut field = variant(input("search_q", "", "search every note"), "input");
            field.keys.insert("enter".into(), Action::Focus { focus: "search".into() });
            field.keys.insert("down".into(), Action::Focus { focus: "search".into() });
            let bar_row = variant(sized(row(vec![sized(text(" <accent+b>/</> "), cells(3)), field, sized(bound(text("\t{count_m|markup} "), "search"), cells(32))]), cells(1)), "input");
            let results = keys(tree("search", "search.rows"), node_keys(View::Search));
            let hint = sized(text(" <muted>deleted nodes are searchable too: add is:deleted</>"), cells(1));
            let content = col(vec![bar_row, sized(text(""), cells(1)), split(results, pane("search", "detail", data)), hint]);
            view(content, "search", &[("enter", "results"), ("x", "done")])
        }
        View::Log => {
            feed("log", json!({}));
            let title = sized(bound(text("{title_m|markup}\t<muted>last 24h · everyone</> "), "log"), cells(1));
            // Two lines a transaction (§5.7): time, who, tx and summary; then what it did.
            let line2 = "\n                          <muted>{detail}</>";
            let mut r = Rows {
                item: Some(format!("<muted>{{time}}</>  <muted>{{who:8}}</>  <muted>{{tx}}</>   {{summary_m|markup}}{line2}")),
                lines: Some(2),
                ..rows(String::new())
            };
            r.variants.insert("agent".into(), format!("<muted>{{time}}</>  <agent>{{who:8}}</>  <muted>{{tx}}</>   {{summary_m|markup}}{line2}"));
            let l = list("log", "log.rows", r);
            let content = col(vec![title, sized(text(""), cells(1)), split(l, pane("log", "tx", data))]);
            view(content, "log", &[("u", "undo"), ("enter", "open")])
        }
        View::Detail => {
            let id_ = route.id.clone().unwrap_or_default();
            feed("detail", json!({"id": id_}));
            let mut lines = list("", "detail.lines", lines_rows());
            lines.id = None;
            lines.pad = Some(Pad::X(1));
            // Its x and X are the screen's keys (`route_keys`): the detail takes no cursor.
            let back = route.back.unwrap_or(View::Today).name();
            view(lines, "detail", &[("x", "done"), ("X", "reopen"), ("esc", back)])
        }
        View::Outline => {
            let id_ = route.id.clone().unwrap_or_default();
            feed("outline", json!({"id": id_}));
            let title = sized(bound(text("{title_m|markup}\t{meta_m|markup} "), "outline"), cells(1));
            let t = keys(tree("outline", "outline.rows"), node_keys(View::Outline));
            view(col(vec![title, sized(text(""), cells(1)), t]), "outline", &[("x", "done"), ("space", "fold"), ("esc", "pages")])
        }
        // A page or a day as Markdown to write in (tui-editor.md): the host's entry for it,
        // loaded when shown, saved half a second after the last change, reloaded when the vault
        // changes. `- [ ]` items tick with Ctrl-T or a click on the box; the keys are the Mac's.
        View::Page | View::Day => {
            let key = doc_key(route, today);
            feed("doc", json!({"key": key}));
            let mut doc = id(bound(node(Kind::Editor { text: String::new(), doc: None, placeholder: Some("write here".into()), syntax: Some("markdown".into()), readonly: false }), &key), "doc");
            doc.pad = Some(Pad::X(1));
            let back = if route.view == View::Day && route.back != Some(View::Pages) { "journal" } else { "pages" };
            view(doc, "doc", &[("esc", back), ("c-t", "tick"), ("⌘z", "undo")])
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
        }
        assert_eq!(View::parse("Journal"), Some(View::Journal));
        assert_eq!(Route { view: View::Day, back: Some(View::Pages), ..Route::default() }.tab(), 3);
    }
}
