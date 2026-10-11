//! Screens: each of thc's views as one fictty `Ui`, built from Rust types. A screen names its
//! data (`source`, always a host source the feeds answer) and its actions (`act`, host actions
//! ThcHost does), so the UI value never holds the data itself: the same value renders a snapshot
//! (`App::headless`) and runs live (`App::run`).
//!
//! The frame (tui-handoff.md §3): header, rule, content, bar. Content splits list | detail at
//! 120 columns and up (`when: "$screen.width>=120"`); below that the list has the width.

use fictty::model::{Action, Column, Kind, Node, Pad, Size, Source, Ui};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Which screen, and what it shows. Serializable: it travels as the `open` action's args, so a
/// key that opens a view is data like any other.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Route {
    pub view: View,
    /// The node a Detail or Outline shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The Tasks filter or the Search words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// The Journal day (`2026-10-11`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// Where Esc goes from a Detail or an Outline.
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
    /// One node, full width (Enter below 120 columns).
    Detail,
    /// One page's outline (Enter on a page).
    Outline,
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
        }
    }

    /// The header tab lit for this view: a Detail under the view it came from, an Outline under
    /// Pages.
    fn tab(self, back: Option<View>) -> usize {
        let v = match self {
            View::Detail => back.unwrap_or(View::Today),
            View::Outline => View::Pages,
            v => v,
        };
        View::TABS.iter().position(|t| *t == v).unwrap_or(0)
    }
}

impl Route {
    pub fn to(view: View) -> Route {
        Route { view, ..Route::default() }
    }

    /// The default Tasks filter.
    pub fn query_or_default(&self) -> String {
        match (&self.query, self.view) {
            (Some(q), _) => q.clone(),
            (None, View::Tasks) => "status:open".into(),
            (None, _) => String::new(),
        }
    }
}

// ---- the one place sources and actions are named -------------------------------------------------

/// A data source: the host's feed `name` with `args`. Every source on every screen goes through
/// here.
pub fn source(name: &str, args: Value) -> Source {
    Source { cmd: None, shell: None, stream: None, value: None, path: None, every: None, host: Some(name.into()), args }
}

/// A host action (`done`, `reopen`, `set`, `add`, `mv`, `open`): args are templates over the
/// selected row (`{id}`), the store (`{store.key}`) and `$id`.
pub fn act(name: &str, args: Value) -> Action {
    Action::Host { host: name.into(), args }
}

/// Open a screen.
pub fn open(route: &Route) -> Action {
    act("open", serde_json::to_value(route).unwrap_or_default())
}

/// The keys that work everywhere: 1–7 the views, `/` Search, `t` the next theme.
fn global_keys(route: &Route) -> BTreeMap<String, Action> {
    let mut keys: BTreeMap<String, Action> = View::TABS.iter().enumerate().map(|(i, v)| ((i + 1).to_string(), open(&Route::to(*v)))).collect();
    keys.insert("/".into(), open(&Route::to(View::Search)));
    keys.insert("t".into(), Action::Theme { theme: "next".into() });
    if let Some(back) = route.back.or(matches!(route.view, View::Outline).then_some(View::Pages)) {
        keys.insert("esc".into(), open(&Route::to(back)));
    }
    keys
}

/// Keys on a list of nodes: x done, X reopen, Enter the node's detail (or what the row opens).
fn node_keys(back: View) -> BTreeMap<String, Action> {
    BTreeMap::from([
        ("x".to_string(), act("done", json!({"id": "{id}"}))),
        ("X".to_string(), act("reopen", json!({"id": "{id}"}))),
        ("enter".to_string(), act("open", json!({"view": "detail", "id": "{id}", "back": back.name()}))),
    ])
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

const WIDE: &str = "$screen.width>=120";
const NARROW: &str = "$screen.width<120";

/// A status cell (§4): `[ ]` `[/]` `[w]` `[x]` `[-]`, a note's ` · `, and Search's ` ¶ ` / ` # `.
const STATUS: &str = "{status|todo=<text>[ ]</> ;doing=<doing>[/]</> ;waiting=<waiting>[w]</> ;done=<done>[x]</> ;cancelled=<muted>[-]</> ;note=<muted> · </> ;page=<muted> ¶ </> ;tag=<muted> # </> }";

/// A row of a tree (§4 as far as a tree draws it): a section's bold title and count, a node's id,
/// status cell and text, an empty state's line; then the meta right-aligned, a cell from the edge.
fn tree_item() -> String {
    format!("{{title_m|markup}}{{count|?  <muted>$</>}}{{note_m|?  }}{{note_m|markup}}{{short|?<muted>$</>  }}{STATUS}{{text_m|markup}}{{m|markup}}\t{{meta_m|markup}} ")
}

/// A row of a flat list: the `▌` gutter is the list's mark, then the id, status cell and text.
fn list_item() -> String {
    format!("<muted>{{short}}</>  {STATUS}{{text_m|markup}}\t{{meta_m|markup}} ")
}

fn tree(id_: &str, bind: &str, back: View) -> Node {
    keys(id(bound(node(Kind::Tree { items: vec![], item: Some(tree_item()), nest: None, key: None, open: true, empty: None, filter: None }), bind), id_), node_keys(back))
}

fn list(id_: &str, bind: &str, item: String, variants: &[(&str, &str)], skip: &[&str]) -> Node {
    id(
        bound(
            node(Kind::List {
                items: vec![],
                item: Some(item),
                empty: None,
                variant: Some("kind".into()),
                variants: variants.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
                skip: skip.iter().map(|s| s.to_string()).collect(),
                mark: Some("<accent+b>▌</> ".into()),
                lead: Some("  ".into()),
                selected: Some("on-selection".into()),
                filter: None,
            }),
            bind,
        ),
        id_,
    )
}

/// Section and empty-state rows in a flat list (§3.5, §8.1).
const LIST_VARIANTS: [(&str, &str); 4] = [
    ("section", " {title_m|markup}{count|?  <muted>$</>}\t{note_m|markup} "),
    ("empty", "   {m|markup}"),
    ("blank", ""),
    ("line", " {m|markup}"),
];

/// The detail pane (§5.8): lines of the selected row (`@list.lines`), each by kind.
fn detail_pane(of: &str) -> Node {
    let lines = node(Kind::List {
        items: vec![],
        item: Some("{m|markup}".into()),
        empty: Some(String::new()),
        variant: Some("k".into()),
        variants: [
            ("title", format!("{STATUS}{{text_m|markup}}")),
            ("field", "<muted>{key}</>{m|markup}".into()),
            ("sub", format!(" {STATUS}{{text_m|markup}}\t{{meta_m|markup}} ")),
            ("blank", String::new()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect(),
        skip: vec![],
        mark: Some(String::new()),
        lead: Some(String::new()),
        selected: None,
        filter: None,
    });
    let mut pane = bound(lines, &format!("@{of}.lines"));
    pane.pad = Some(Pad::X(1));
    pane
}

/// A list | detail split (§3.4): at 120 columns and up a `│` and the detail pane take 40%.
/// TODO(fictty): the design's list width is min(60%, 96); sizes have no min/max yet, so above
/// 160 columns the list grows past 96.
fn split(main: Node, side: Node, side_size: Size) -> Node {
    let rule = sized(node(Kind::Rule { glyph: Some("│".into()) }), cells(1));
    id(row(vec![main, when(rule, WIDE), when(sized(side, side_size), WIDE)]), "split")
}

/// The size of the content's split side, if it has one (so the rule above can join it).
fn side_of(content: &Node) -> Option<Size> {
    let mut all = Vec::new();
    content.walk(&mut all);
    let split = all.into_iter().find(|n| n.id.as_deref() == Some("split"))?;
    split.children().last().and_then(|n| n.size.clone())
}

// ---- the frame ------------------------------------------------------------------------------------

/// Header and rule (§3.1–3.2), from the `frame` feed (which knows the lit tab). The rule's `┬`
/// sits over the split when there is one: its row has the content row's sizes.
fn header(side: Option<Size>) -> Vec<Node> {
    let frame = |t: &str| bound(text(t), "frame");
    let rule_wide = || frame("{rule_wide|markup}<line>{*─}</>");
    // At 120 and up, a view with a split puts `┬` where the split's `│` is: a row with the
    // content row's sizes, laid over the whole rule (empty text draws nothing), so the accent
    // segment can sit on either side of it.
    let split_rule = match side {
        Some(size) => node(Kind::Stack { children: vec![rule_wide(), row(vec![text(""), sized(text("<line>┬</>"), cells(1)), sized(text(""), size)])] }),
        None => rule_wide(),
    };
    vec![
        sized(when(frame("{wide|markup}\t{date_wide|markup}"), "$screen.width>=110"), cells(1)),
        sized(when(frame("{narrow|markup}\t{date_narrow|markup}"), "$screen.width<110"), cells(1)),
        sized(
            when(col(vec![sized(when(split_rule, WIDE), cells(1)), sized(when(rule_wide(), NARROW), cells(1))]), "$screen.width>=110"),
            cells(1),
        ),
        sized(when(frame("{rule_narrow|markup}<line>{*─}</>"), "$screen.width<110"), cells(1)),
    ]
}

/// The bar (§3.3): the view's name and counts on the left (`bar_m` of the view's feed), the
/// daemon's dot and the view's hints on the right.
fn bar(feed: &str, hints: &[(&str, &str)]) -> Node {
    let right = format!(" {{live|true=<done>●</> <muted>live</>;*=<muted>○ local</>}}   {} ", crate::feeds::hints(hints, "  "));
    let plain: String = hints.iter().map(|(k, w)| format!("{k} {w}")).collect::<Vec<_>>().join("  ");
    let width = unicode_width::UnicodeWidthStr::width(format!(" ○ local   {plain} ").as_str()) as u16;
    sized(
        row(vec![
            variant(bound(text(" {bar_m|markup}"), feed), "bar"),
            variant(sized(bound(text(&format!("\t{right}")), "frame"), cells(width)), "bar"),
        ]),
        cells(1),
    )
}

/// A whole screen: frame around `content`, the sources it reads, the keys.
fn screen(route: &Route, feed: (&str, Value), content: Node, hints: &[(&str, &str)]) -> Ui {
    let tab = route.view.tab(route.back);
    let mut children = header(side_of(&content));
    children.push(content);
    children.push(bar(feed.0, hints));
    let mut data = BTreeMap::new();
    data.insert("frame".to_string(), source("frame", json!({"tab": tab})));
    data.insert(feed.0.to_string(), source(feed.0, feed.1));
    Ui { root: col(children), data, keys: global_keys(route), status_bar: Some(false), ..Ui::default() }
}

// ---- the views ------------------------------------------------------------------------------------

/// The `Ui` for a route. `today` resolves a Journal without a date.
pub fn build(route: &Route, today: chrono::NaiveDate) -> Ui {
    match route.view {
        View::Today => screen(
            route,
            ("today", json!({})),
            split(tree("today", "today.rows", View::Today), detail_pane("today"), spec("40%")),
            &[("a", "add"), (":", "cmd"), ("?", "help")],
        ),
        View::Inbox => screen(
            route,
            ("inbox", json!({})),
            split(keys(list("inbox", "inbox.rows", list_item(), &LIST_VARIANTS, &["section", "empty", "blank", "line"]), node_keys(View::Inbox)), detail_pane("inbox"), spec("40%")),
            &[("m", "move"), ("t", "task"), ("x", "done")],
        ),
        View::Tasks => tasks(route),
        View::Pages => pages(route),
        View::Outline => {
            let id_ = route.id.clone().unwrap_or_default();
            let title = sized(bound(text(" {title_m|markup}\t{meta_m|markup} "), "outline"), cells(1));
            let content = col(vec![title, sized(text(""), cells(1)), tree("outline", "outline.rows", View::Outline)]);
            screen(route, ("outline", json!({"id": id_})), content, &[("x", "done"), ("space", "fold"), ("esc", "pages")])
        }
        View::Journal => journal(route, today),
        View::Search => search(route),
        View::Log => {
            let title = sized(bound(text(" {title_m|markup}\t<muted>last 24h · everyone</> "), "log"), cells(1));
            let log = list(
                "log",
                "log.rows",
                "<muted>{time}</>  {who_m|markup}  <muted>{tx}</>   {summary_m|markup}".into(),
                &[("txdetail", "                            {m|markup}"), ("empty", "   {m|markup}"), ("blank", "")],
                &["txdetail", "empty", "blank"],
            );
            let content = col(vec![title, sized(text(""), cells(1)), split(log, detail_pane("log"), spec("40%"))]);
            screen(route, ("log", json!({})), content, &[("u", "undo"), ("enter", "open")])
        }
        View::Detail => {
            let id_ = route.id.clone().unwrap_or_default();
            let mut pane = detail_pane("detail");
            pane.bind = Some("detail.lines".into());
            let back = route.back.unwrap_or(View::Today);
            let mut ui = screen(route, ("detail", json!({"id": id_})), pane, &[("x", "done"), ("X", "reopen"), ("esc", back.name())]);
            ui.keys.insert("x".into(), act("done", json!({"id": id_})));
            ui.keys.insert("X".into(), act("reopen", json!({"id": id_})));
            ui
        }
    }
}

/// Tasks (§5.3): the filter row, then a table at 120 and up, a list below.
fn tasks(route: &Route) -> Ui {
    let q = route.query_or_default();
    let filter = variant(sized(bound(text(" <accent+b>q›</> {filter_m|markup}\t<muted>{count}</> {error_m|markup}"), "tasks"), cells(1)), "input");
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
    let narrow = keys(list("tasks-list", "tasks.rows", format!("<muted>{{short}}</>  {STATUS}{{task_m|markup}}\t{{due_m|markup}} "), &[], &[]), node_keys(View::Tasks));
    let content = col(vec![filter, sized(text(""), cells(1)), when(table, WIDE), when(narrow, NARROW)]);
    screen(route, ("tasks", json!({"query": q})), content, &[("f", "filter"), ("x", "done"), ("enter", "open")])
}

/// Pages (§5.4): the finder (typing filters the list as you go), the pages, tags and recent
/// days; at 120 and up the highlighted page beside it as Markdown, task boxes and all.
fn pages(route: &Route) -> Ui {
    let mut finder = id(node(Kind::Input { text: String::new(), placeholder: Some("type to find a page, tag or day".into()), accept: None }), "pages_q");
    finder.keys.insert("enter".into(), Action::Focus { focus: "pages".into() });
    finder.keys.insert("down".into(), Action::Focus { focus: "pages".into() });
    let finder_row = variant(sized(row(vec![sized(text(" <accent+b>›</> "), cells(3)), variant(finder, "input")]), cells(1)), "input");
    let mut l = list(
        "pages",
        "pages.rows",
        "<muted>{short}</>  {title_m|markup}\t{meta_m|markup} ".into(),
        &[
            ("section", " {title_m|markup}{count|?  <muted>$</>}\t{note_m|markup} "),
            ("day", "{title_m|markup}\t{meta_m|markup} "),
            ("line", " {m|markup}"),
            ("empty", "   {m|markup}"),
            ("blank", ""),
        ],
        &["section", "line", "empty", "blank"],
    );
    if let Kind::List { filter, .. } = &mut l.kind {
        *filter = Some("pages_q".into());
    }
    l.keys.insert("enter".into(), act("open", json!({"view": "{open}", "id": "{id}", "date": "{date}"})));
    // The preview: the page as Markdown in a readonly editor (headings, bullets, `- [ ]` tasks).
    // TODO(fictty): an editor's text follows its seed only while the entry is new; a page that
    // changes after it was first shown keeps the old preview until the store entry is rewritten.
    let mut preview = bound(node(Kind::Editor { text: "{md}".into(), doc: Some("page/{id}".into()), placeholder: Some(String::new()), syntax: Some("markdown".into()), readonly: true }), "@pages");
    preview.id = Some("preview".into());
    preview.pad = Some(Pad::X(1));
    let content = col(vec![finder_row, sized(text(""), cells(1)), split(l, preview, spec("70%"))]);
    screen(route, ("pages", json!({})), content, &[("enter", "open"), ("tab", "list"), ("esc", "clear")])
}

/// Journal (§5.5): the day's title, the day strip, its entries; the month beside it at 120 and up.
fn journal(route: &Route, today: chrono::NaiveDate) -> Ui {
    let date = route.date.as_deref().and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).unwrap_or(today);
    let day = |d: chrono::NaiveDate| open(&Route { view: View::Journal, date: Some(d.format("%Y-%m-%d").to_string()), ..Route::default() });
    let title = sized(bound(text(" {title_m|markup}\t<text>[</> <muted>prev</>  <text>]</> <muted>next</>  <text>T</> <muted>today</> "), "journal"), cells(1));
    let strip = sized(bound(text("   {strip_m|markup}"), "journal"), cells(1));
    let calendar = node(Kind::List {
        items: vec![],
        item: Some("{m|markup}".into()),
        empty: None,
        variant: None,
        variants: BTreeMap::new(),
        skip: vec![],
        mark: Some(String::new()),
        lead: Some(String::new()),
        selected: None,
        filter: None,
    });
    let mut calendar = bound(calendar, "journal.calendar");
    calendar.pad = Some(Pad::X(1));
    let content = col(vec![title, strip, sized(text(""), cells(1)), split(tree("journal", "journal.rows", View::Journal), calendar, cells(36))]);
    let mut ui = screen(route, ("journal", json!({"date": date.format("%Y-%m-%d").to_string()})), content, &[("[", "prev"), ("]", "next"), ("T", "today")]);
    ui.keys.insert("[".into(), day(date - chrono::Duration::days(1)));
    ui.keys.insert("]".into(), day(date + chrono::Duration::days(1)));
    ui.keys.insert("T".into(), day(today));
    ui
}

/// Search (§5.6): the words, then Pages, Tags and Nodes; the match's detail beside them.
fn search(route: &Route) -> Ui {
    let q = route.query_or_default();
    let mut input = id(node(Kind::Input { text: q.clone(), placeholder: Some("search every note".into()), accept: None }), "search_q");
    // Enter searches: the screen opens again with the words in the store as its query.
    input.keys.insert("enter".into(), open(&Route { view: View::Search, query: Some("{store.search_q}".into()), ..Route::default() }));
    input.keys.insert("down".into(), Action::Focus { focus: "search".into() });
    let bar_row = variant(
        sized(row(vec![sized(text(" <accent+b>/</> "), cells(3)), variant(input, "input"), variant(sized(bound(text("\t{count_m|markup} "), "search"), cells(32)), "input")]), cells(1)),
        "input",
    );
    let hint = sized(text(" <muted>deleted nodes are searchable too: add is:deleted</>"), cells(1));
    let content = col(vec![bar_row, sized(text(""), cells(1)), split(tree("search", "search.rows", View::Search), detail_pane("search"), spec("40%")), hint]);
    screen(route, ("search", json!({"query": q})), content, &[("enter", "search"), ("tab", "results")])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_view_builds_and_round_trips_as_json() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
        for v in [View::Today, View::Inbox, View::Tasks, View::Pages, View::Journal, View::Search, View::Log, View::Detail, View::Outline] {
            let ui = build(&Route::to(v), today);
            let json = serde_json::to_value(&ui).unwrap();
            let back: Ui = serde_json::from_value(json.clone()).unwrap();
            // The JSON survives; the structs needn't. TODO(fictty): a list's `variant` (the row
            // field naming its template) and a node's `variant` (a theme look) share one JSON key,
            // and the node's takes it when read back, so a list read from JSON loses its row
            // variants. Reported; until it's fixed, compare the JSON.
            assert_eq!(json, serde_json::to_value(&back).unwrap(), "{v:?}");
            assert!(ui.data.values().all(|s| s.host.is_some()), "{v:?}: every source is the host's");
        }
        assert_eq!(View::parse("Journal"), Some(View::Journal));
    }
}
