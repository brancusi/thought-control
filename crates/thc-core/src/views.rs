//! Saved views (docs/design/views.md §1): named queries stored as nodes under a hidden system
//! page, `¶ Views`, so they sync, show in history and undo like anything else.
//!
//! The page and the built-in views have deterministic ids (`id::from_key`), so two devices that
//! seed on first run write the same `node.create`s, which are idempotent: no duplicates.

use crate::builder::TxBuilder;
use crate::error::{invalid, not_found};
use crate::event::Op;
use crate::id;
use crate::ord::key_between;
use crate::store::Store;
use anyhow::Result;
use serde::Serialize;
use serde_json::{Map, Value, json};

/// The system page's `system` prop value.
pub const SYSTEM: &str = "views";

/// The built-in filters, seeded as editable views on first run: (name, title, query, tasks slot).
/// Names stay short for typing; the saved row shows the title.
pub const BUILTIN: [(&str, &str, &str, u32); 5] = [
    ("open", "open", "status:open sort:due", 1),
    ("week", "this week", "status:open due<=+7d sort:due", 2),
    ("waiting", "waiting", "status:waiting sort:updated-", 3),
    ("claude", "by claude", "status:open by:claude sort:due", 4),
    ("ready", "ready", "is:ready sort:priority", 5),
];

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct View {
    pub id: String,
    pub name: String,
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Position on the TUI Tasks saved row (1–9).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks: Option<u32>,
    /// A ThoughtBar preset.
    pub bar: bool,
    /// Capture defaults when this view is the context (views.md §2.2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture: Option<String>,
}

pub fn page_id() -> String {
    id::from_key("thc:system:views")
}

fn view_id(name: &str) -> String {
    id::from_key(&format!("thc:view:{name}"))
}

/// Names are lowercase letters, digits and `-`, written with `@` (which is optional on input).
pub fn normalize_name(raw: &str) -> Result<String> {
    let n = raw.trim().trim_start_matches('@').to_string();
    if n.is_empty() || !n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(invalid(format!("view names are lowercase letters, digits and -, got {raw:?}")));
    }
    Ok(n)
}

pub fn page_exists(store: &Store) -> Result<bool> {
    store.node_exists(&page_id())
}

/// Every view, by name. Deleted views are gone (undo brings them back).
pub fn list(store: &Store) -> Result<Vec<View>> {
    let nodes = store.nodes_where("n.parent = ?1 AND n.deleted = 0 ORDER BY n.ord", &[&page_id()])?;
    let mut out = Vec::new();
    for n in nodes {
        let p = store.props_of(&n.id)?;
        let Some(name) = p.get("view").and_then(|v| v.as_str()) else { continue };
        out.push(View {
            id: n.id.clone(),
            name: name.to_string(),
            query: p.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            title: n.title.clone(),
            tasks: p.get("tasks").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).map(|t| t as u32),
            bar: p.get("bar").is_some_and(|v| v == &Value::Bool(true) || v == "true"),
            capture: p.get("capture").and_then(|v| v.as_str()).map(str::to_string),
        });
    }
    out.sort_by(|a, b| (a.tasks.unwrap_or(99), &a.name).cmp(&(b.tasks.unwrap_or(99), &b.name)));
    Ok(out)
}

pub fn find(store: &Store, name: &str) -> Result<Option<View>> {
    let name = name.trim_start_matches('@');
    Ok(list(store)?.into_iter().find(|v| v.name == name))
}

/// The closest view name, for "did you mean".
pub fn closest(store: &Store, name: &str) -> Option<String> {
    let views = list(store).ok()?;
    views.iter().map(|v| (crate::query::distance(&v.name, name), v.name.clone())).filter(|(d, _)| *d <= 2).min().map(|(_, n)| n)
}

fn last_ord(b: &TxBuilder) -> Result<Option<String>> {
    let page = page_id();
    // Views created earlier in this transaction come after the stored ones.
    let pending = b.ops.iter().rev().find_map(|o| match o {
        Op::NodeCreate { parent: Some(p), order, .. } if *p == page => Some(order.clone()),
        _ => None,
    });
    if pending.is_some() {
        return Ok(pending);
    }
    Ok(b.store.conn.query_row("SELECT max(ord) FROM nodes WHERE parent = ?1", [&page], |r| r.get::<_, Option<String>>(0))?)
}

fn view_props(name: &str, query: &str, tasks: Option<u32>, bar: bool, capture: Option<&str>) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("view".into(), json!(name));
    m.insert("query".into(), json!(query));
    if let Some(t) = tasks {
        m.insert("tasks".into(), json!(t));
    }
    if bar {
        m.insert("bar".into(), json!(true));
    }
    if let Some(c) = capture {
        m.insert("capture".into(), json!(c));
    }
    m
}

/// Create the system page and the built-in views, unless the page already exists (so a view the
/// user removed never comes back).
pub fn ensure_seeded(b: &mut TxBuilder) -> Result<bool> {
    if page_exists(b.store)? || b.ops.iter().any(|o| matches!(o, Op::NodeCreate { id, .. } if *id == page_id())) {
        return Ok(false);
    }
    let mut props = Map::new();
    props.insert("system".into(), json!(SYSTEM));
    b.ops.push(Op::NodeCreate { id: page_id(), parent: None, order: key_between(None, None), text: String::new(), title: Some("Views".into()), props });
    let mut prev: Option<String> = None;
    for (name, title, query, slot) in BUILTIN {
        let ord = key_between(prev.as_deref(), None);
        b.ops.push(Op::NodeCreate {
            id: view_id(name),
            parent: Some(page_id()),
            order: ord.clone(),
            text: format!("@{name}"),
            title: Some(title.to_string()),
            props: view_props(name, query, Some(slot), false, None),
        });
        prev = Some(ord);
    }
    Ok(true)
}

/// Check that a query compiles (unknown fields, values and `@views` exit 6 with a hint).
pub fn validate_query(store: &Store, query: &str, today: chrono::NaiveDate) -> Result<()> {
    crate::query::compile(query, store, today).map(|_| ())
}

/// The `@names` a query mentions (whole tokens only, as `expand` reads them).
pub fn refs(query: &str) -> Vec<String> {
    let chars: Vec<char> = query.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let start = i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == '(' || chars[i - 1] == '-';
        if chars[i] == '@' && start {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_ascii_lowercase() || chars[j].is_ascii_digit() || chars[j] == '-') {
                j += 1;
            }
            if j > i + 1 && (j == chars.len() || chars[j].is_whitespace() || chars[j] == ')') {
                out.push(chars[i + 1..j].iter().collect());
            }
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

/// Reject a query that would make `name` refer to itself: `@a would refer to itself through @b`.
pub fn check_cycle(store: &Store, name: &str, query: &str) -> Result<()> {
    fn walk(store: &Store, target: &str, query: &str, path: &mut Vec<String>) -> Result<()> {
        for r in refs(query) {
            if r == target {
                return Err(if path.is_empty() {
                    invalid(format!("@{target} can't use itself"))
                } else {
                    invalid(format!("@{target} would refer to itself through {}", path.iter().map(|p| format!("@{p}")).collect::<Vec<_>>().join(", ")))
                });
            }
            if path.contains(&r) || path.len() > 8 {
                continue;
            }
            if let Some(v) = find(store, &r)? {
                path.push(r);
                walk(store, target, &v.query, path)?;
                path.pop();
            }
        }
        Ok(())
    }
    walk(store, name, query, &mut Vec::new())
}

pub fn add(b: &mut TxBuilder, name: &str, query: &str, title: Option<&str>, tasks: Option<u32>, bar: bool, capture: Option<&str>) -> Result<String> {
    let name = normalize_name(name)?;
    check_cycle(b.store, &name, query)?;
    ensure_seeded(b)?;
    if find(b.store, &name)?.is_some() || pending_has(b, &name) {
        return Err(invalid(format!("@{name} already exists · thc view set {name} '<query>' to change it")));
    }
    if let Some(t) = tasks {
        if !(1..=9).contains(&t) {
            return Err(invalid("--tasks takes 1 to 9 (a digit key on the Tasks saved row)"));
        }
    }
    let ord = key_between(last_ord(b)?.as_deref(), None);
    // A fresh id, not the deterministic one: a removed built-in can be re-added by name.
    let id = id::new_id();
    b.ops.push(Op::NodeCreate {
        id: id.clone(),
        parent: Some(page_id()),
        order: ord,
        text: format!("@{name}"),
        title: title.map(str::to_string),
        props: view_props(&name, query, tasks, bar, capture),
    });
    Ok(id)
}

fn pending_has(b: &TxBuilder, name: &str) -> bool {
    b.ops.iter().any(|o| matches!(o, Op::NodeCreate { props, .. } if props.get("view").and_then(|v| v.as_str()) == Some(name)))
}

/// Changes to a view; `None` leaves a field alone, `Some(None)` clears it.
#[derive(Default)]
pub struct Update<'a> {
    pub query: Option<&'a str>,
    pub title: Option<Option<&'a str>>,
    pub tasks: Option<Option<u32>>,
    pub bar: Option<bool>,
    pub capture: Option<Option<&'a str>>,
}

pub fn set(b: &mut TxBuilder, name: &str, u: Update) -> Result<View> {
    let name = normalize_name(name)?;
    let v = find(b.store, &name)?.ok_or_else(|| not_found(format!("no view @{name}")))?;
    if let Some(q) = u.query {
        check_cycle(b.store, &name, q)?;
    }
    let mut props = Map::new();
    if let Some(q) = u.query {
        props.insert("query".into(), json!(q));
    }
    if let Some(t) = u.title {
        props.insert("title".into(), t.map(|s| json!(s)).unwrap_or(Value::Null));
    }
    if let Some(t) = u.tasks {
        if let Some(n) = t {
            if !(1..=9).contains(&n) {
                return Err(invalid("--tasks takes 1 to 9 (a digit key on the Tasks saved row)"));
            }
        }
        props.insert("tasks".into(), t.map(|n| json!(n)).unwrap_or(Value::Null));
    }
    if let Some(bar) = u.bar {
        props.insert("bar".into(), if bar { json!(true) } else { Value::Null });
    }
    if let Some(c) = u.capture {
        props.insert("capture".into(), c.map(|s| json!(s)).unwrap_or(Value::Null));
    }
    if !props.is_empty() {
        b.ops.push(Op::NodeSet { id: v.id.clone(), props });
    }
    Ok(v)
}

pub fn remove(b: &mut TxBuilder, name: &str) -> Result<View> {
    let name = normalize_name(name)?;
    let v = find(b.store, &name)?.ok_or_else(|| not_found(format!("no view @{name}")))?;
    b.ops.push(Op::NodeDelete { id: v.id.clone() });
    Ok(v)
}

/// SQL that keeps system pages and their children (the `¶ Views` page and the views) out of
/// listings, search and queries.
pub const HIDDEN_SQL: &str = "n.id NOT IN (SELECT node FROM props WHERE key = 'system') \
     AND (n.parent IS NULL OR n.parent NOT IN (SELECT node FROM props WHERE key = 'system'))";

/// Expand every `@name` in a query to `(its query)`. `@name` only counts as a whole token (start,
/// space or `(` before it; end, space or `)` after), so `text:a@b.c` is left alone. Views may use
/// other views, up to a depth of 4 (a loop is an error). Returns the expanded text and the views used.
pub fn expand(store: &Store, query: &str) -> Result<(String, Vec<View>)> {
    let mut used = Vec::new();
    let out = expand_depth(store, query, 0, &mut used)?;
    Ok((out, used))
}

fn expand_depth(store: &Store, query: &str, depth: usize, used: &mut Vec<View>) -> Result<String> {
    if !query.contains('@') {
        return Ok(query.to_string());
    }
    if depth > 4 {
        return Err(invalid("views nest too deep (a view that uses itself?)"));
    }
    let chars: Vec<char> = query.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let at_token_start = i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == '(' || chars[i - 1] == '-';
        if c == '@' && at_token_start {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_ascii_lowercase() || chars[j].is_ascii_digit() || chars[j] == '-') {
                j += 1;
            }
            let ends = j == chars.len() || chars[j].is_whitespace() || chars[j] == ')';
            if j > i + 1 && ends {
                let name: String = chars[i + 1..j].iter().collect();
                let v = find(store, &name)?.ok_or_else(|| {
                    let hint = closest(store, &name).map(|c| format!(" · did you mean @{c}?")).unwrap_or_default();
                    not_found(format!("no view @{name}{hint}"))
                })?;
                if store.props_of(&v.id)?.contains_key("sections") {
                    return Err(invalid(format!("@{name} has sections, so it can't go inside a query · run it alone: thc q @{name}")));
                }
                let inner = expand_depth(store, &v.query, depth + 1, used)?;
                if !used.iter().any(|u| u.name == v.name) {
                    used.push(v);
                }
                out.push('(');
                out.push_str(&inner);
                out.push(')');
                i = j;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    Ok(out)
}

// ---- Sectioned views (vaults.md §3.2) -------------------------------------------------------

/// One named section of a sectioned view: a title and its query.
#[derive(Clone, Debug, Serialize, PartialEq, Eq, serde::Deserialize)]
pub struct Section {
    pub title: String,
    pub query: String,
}

/// A view made of sections, with a scope that applies to every section (`vault:*`, or none for
/// the current vault). The built-ins (`@today`, `@agenda`, `@inbox`, `@tasks`) ship in the binary;
/// editing one stores your version, and `reset` goes back to the shipped one.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Sectioned {
    pub name: String,
    pub scope: Option<String>,
    pub sections: Vec<Section>,
    /// One of the shipped views (edited or not).
    pub builtin: bool,
    /// Stored (your version), not the shipped one.
    pub edited: bool,
}

/// The shipped sectioned views: (name, scope, [(section, query)]). Until you edit one, the
/// built-in code runs it (it also shows fired alerts and alert times, which a query can't say).
pub const SECTIONED: [(&str, Option<&str>, &[(&str, &str)]); 4] = [
    (
        "today",
        Some("vault:*"),
        &[
            ("Overdue", "is:overdue status:open"),
            ("Today", "(sched<=today or due=today or is:alert) status:open -is:overdue"),
            ("Doing", "status:doing"),
            ("Next 7 days", "happens<=+7d happens>today status:open"),
            ("Done today", "done>=today"),
        ],
    ),
    ("agenda", Some("vault:*"), &[("Agenda", "happens>=today happens<=+7d status:open sort:date")]),
    ("inbox", None, &[("Inbox", "is:inbox sort:created")]),
    ("tasks", None, &[("Tasks", "status:open sort:due")]),
];

fn shipped(name: &str) -> Option<Sectioned> {
    SECTIONED.iter().find(|(n, ..)| *n == name).map(|(n, scope, secs)| Sectioned {
        name: n.to_string(),
        scope: scope.map(str::to_string),
        sections: secs.iter().map(|(t, q)| Section { title: t.to_string(), query: q.to_string() }).collect(),
        builtin: true,
        edited: false,
    })
}

/// A sectioned view by name: yours (stored) if you've edited or made it, else the shipped one.
pub fn sectioned(store: &Store, name: &str) -> Result<Option<Sectioned>> {
    let name = name.trim_start_matches('@');
    if let Some(v) = stored_sectioned(store, name)? {
        return Ok(Some(v));
    }
    Ok(shipped(name))
}

/// Every sectioned view: the shipped ones (or your versions), then yours.
pub fn all_sectioned(store: &Store) -> Result<Vec<Sectioned>> {
    let mut out: Vec<Sectioned> = SECTIONED.iter().filter_map(|(n, ..)| sectioned(store, n).ok().flatten()).collect();
    for v in list_raw(store)? {
        if v.1.is_some() && !out.iter().any(|o| o.name == v.0.name) {
            if let Some(s) = stored_sectioned(store, &v.0.name)? {
                out.push(s);
            }
        }
    }
    Ok(out)
}

/// Views with their `sections` prop (raw JSON), for sectioned lookups.
fn list_raw(store: &Store) -> Result<Vec<(View, Option<Value>)>> {
    let mut out = Vec::new();
    for v in list(store)? {
        let s = store.props_of(&v.id)?.get("sections").cloned();
        out.push((v, s));
    }
    Ok(out)
}

fn stored_sectioned(store: &Store, name: &str) -> Result<Option<Sectioned>> {
    let Some(v) = find(store, name)? else { return Ok(None) };
    let p = store.props_of(&v.id)?;
    let Some(raw) = p.get("sections") else { return Ok(None) };
    let sections: Vec<Section> = match raw {
        Value::String(s) => serde_json::from_str(s).unwrap_or_default(),
        other => serde_json::from_value(other.clone()).unwrap_or_default(),
    };
    Ok(Some(Sectioned {
        name: v.name.clone(),
        scope: p.get("scope").and_then(|s| s.as_str()).map(str::to_string).filter(|s| !s.is_empty()),
        sections,
        builtin: shipped(&v.name).is_some(),
        edited: true,
    }))
}

/// Whether a scope reads more than one vault (where the view lives: vaults.md §3.3).
pub fn spans_vaults(scope: Option<&str>) -> bool {
    scope.is_some_and(|s| s.contains("vault:*") || s.contains(" or ") || s.matches("vault:").count() > 1)
}

/// Store a sectioned view: replaces your version of `name` (or makes it). `scope`: None keeps
/// what it had (the shipped scope for a built-in).
pub fn set_sectioned(b: &mut TxBuilder, name: &str, scope: Option<Option<&str>>, sections: &[Section], today: chrono::NaiveDate) -> Result<Sectioned> {
    let name = normalize_name(name)?;
    if sections.is_empty() {
        return Err(invalid("a sectioned view needs at least one --section TITLE QUERY"));
    }
    for s in sections {
        if s.title.trim().is_empty() {
            return Err(invalid("a section needs a title"));
        }
        if s.query.contains("vault:") {
            return Err(invalid(format!("section {:?}: the scope goes in --scope, not the section's query", s.title)));
        }
        check_cycle(b.store, &name, &s.query)?;
        validate_query(b.store, &s.query, today)?;
    }
    let had = sectioned(b.store, &name)?;
    let scope: Option<String> = match scope {
        Some(s) => s.map(str::to_string).filter(|s| !s.trim().is_empty()),
        None => had.as_ref().and_then(|h| h.scope.clone()),
    };
    if let Some(sc) = &scope {
        if !sc.split_whitespace().all(|t| t.starts_with("vault:") || t.starts_with("-vault:") || t == "or" || t.ends_with(')')) {
            return Err(invalid(format!("--scope takes vault: terms (vault:*, vault:(a or b), -vault:x), got {sc:?}")));
        }
    }
    ensure_seeded(b)?;
    let mut props = Map::new();
    props.insert("sections".into(), json!(serde_json::to_string(sections)?));
    props.insert("scope".into(), scope.as_ref().map(|s| json!(s)).unwrap_or(json!("")));
    match find(b.store, &name)? {
        Some(v) => b.ops.push(Op::NodeSet { id: v.id.clone(), props }),
        None => {
            let mut all = view_props(&name, "", None, false, None);
            all.extend(props);
            // Built-ins keep one id on every device: two devices editing @today agree on the node.
            let id = if shipped(&name).is_some() { view_id(&name) } else { id::new_id() };
            let ord = key_between(last_ord(b)?.as_deref(), None);
            b.ops.push(Op::NodeCreate { id, parent: Some(page_id()), order: ord, text: format!("@{name}"), title: None, props: all });
        }
    }
    Ok(Sectioned { name: name.clone(), scope, sections: sections.to_vec(), builtin: shipped(&name).is_some(), edited: true })
}

/// Back to the shipped view (your version deleted; `thc undo` brings it back).
pub fn reset(b: &mut TxBuilder, name: &str) -> Result<()> {
    let name = normalize_name(name)?;
    if shipped(&name).is_none() {
        return Err(invalid(format!("@{name} isn't a built-in · thc view rm {name} removes it")));
    }
    match find(b.store, &name)? {
        Some(v) if stored_sectioned(b.store, &name)?.is_some() => {
            b.ops.push(Op::NodeDelete { id: v.id.clone() });
            Ok(())
        }
        _ => Err(invalid(format!("@{name} is already the shipped one"))),
    }
}

#[cfg(test)]
mod sectioned_tests {
    use super::*;

    #[test]
    fn the_shipped_sections_are_valid_queries() {
        let dir = std::env::temp_dir().join(format!("thc-sect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("s.db")).unwrap();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        for (name, _, secs) in SECTIONED {
            for (t, q) in secs.iter() {
                crate::query::compile(q, &store, today).unwrap_or_else(|e| panic!("@{name} {t}: {q}: {e}"));
            }
        }
        assert!(spans_vaults(Some("vault:*")) && spans_vaults(Some("vault:(a or b)")) && !spans_vaults(Some("vault:acme")) && !spans_vaults(None));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
