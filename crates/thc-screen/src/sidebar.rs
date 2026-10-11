//! The sidebar's stack (sidebar.md §2, §3, §10.1), as the screen's store entry `sidebar`: the
//! panels top to bottom (`{kind, ref, pinned, folded, opened_by}`), the ones closed for `⌥⇧T`,
//! whether it shows. Pure rules over that value: the host runs them for the person's keys, and
//! `thc ui aside` runs the same ones for an agent, writing the entry with `--if-rev`.

use serde::{Deserialize, Serialize};

/// At most this many panels (§2 rule 4); `closed` keeps as many.
pub const LIMIT: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A page: `ref` is its id.
    Page,
    /// A journal day: `ref` is `2026-10-06`, or `today` (turns over at midnight, §8.1).
    Day,
    /// A built-in view: `today`, `inbox`, `tasks`, `log`.
    View,
    /// Any query; a tag panel is `#health`.
    Query,
}

impl Kind {
    pub fn is_doc(self) -> bool {
        matches!(self, Kind::Page | Kind::Day)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    pub kind: Kind,
    #[serde(rename = "ref")]
    pub target: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub folded: bool,
    /// An agent's open (§10.4): its header says who until the person touches it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened_by: Option<String>,
}

impl Panel {
    pub fn new(kind: Kind, target: &str) -> Panel {
        Panel { kind, target: target.into(), pinned: false, folded: false, opened_by: None }
    }

    /// The key as an id: letters, digits and `-` only (`page-jxwp0`, `query--health`).
    pub fn id(&self) -> String {
        self.key().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
    }

    /// What in the panel takes the focus (the end of its node ids): its body, `panel-…` (a page
    /// or day's editor) or `rows-…` (a list's rows), or its header, `head-…`, when it's folded.
    pub fn target(&self) -> String {
        let what = if self.folded {
            "head"
        } else if self.kind.is_doc() {
            "panel"
        } else {
            "rows"
        };
        format!("{what}-{}", self.id())
    }

    /// The panel's key, unique in the stack: `page:<id>`, `day:today`, `query:#health`.
    pub fn key(&self) -> String {
        format!("{}:{}", serde_json::to_value(self.kind).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default(), self.target)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
///
/// Whether it shows (the column at ≥ 120, the drawer or replace below) is the screen's own entry
/// `sidebar_shown`, so `⌥S` can show it and focus it in one key.
pub struct Stack {
    /// Top to bottom; the pinned panels first.
    #[serde(default)]
    pub open: Vec<Panel>,
    /// For `⌥⇧T`, newest first.
    #[serde(default)]
    pub closed: Vec<Panel>,
}

/// What an open did (§2): the bar says the last two.
#[derive(Clone, Debug, PartialEq)]
pub enum Opened {
    New,
    /// Already there: moved to the top of its group (a pinned one stays, §2 rule 2).
    Moved,
    /// The ninth closed the bottom unpinned panel to make room.
    Evicted(Panel),
    /// Eight pinned: nothing opens.
    Refused,
}

impl Stack {
    pub fn from_value(v: &serde_json::Value) -> Stack {
        serde_json::from_value(v.clone()).unwrap_or_default()
    }

    /// The stack as the store keeps it. Each panel carries a stable `id` (its key, as cells an id
    /// can hold), so a node made per panel can be named by it.
    pub fn to_value(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        for list in ["open", "closed"] {
            for (p, panel) in v[list].as_array_mut().into_iter().flatten().zip(if list == "open" { &self.open } else { &self.closed }) {
                p["id"] = panel.id().into();
            }
        }
        v
    }

    /// A day panel to another day (sidebar.md §8.1: `[` `]` `T`): `by` days from the one it
    /// shows, or None for the relative `today`. If that day is already a panel, nothing moves.
    /// The key it ends on.
    pub fn retarget_day(&mut self, key: &str, by: Option<i64>, today: chrono::NaiveDate) -> Option<String> {
        let i = self.position(key)?;
        let p = &self.open[i];
        if p.kind != Kind::Day {
            return Some(key.into());
        }
        let shown = if p.target == "today" { today } else { chrono::NaiveDate::parse_from_str(&p.target, "%Y-%m-%d").ok()? };
        let to = match by {
            Some(n) => (shown + chrono::Duration::days(n)).format("%Y-%m-%d").to_string(),
            None => "today".into(),
        };
        let next = Panel::new(Kind::Day, &to);
        if let Some(j) = self.position(&next.key()) {
            return Some(self.open[j].key());
        }
        self.open[i].target = to;
        Some(next.key())
    }

    pub fn position(&self, key: &str) -> Option<usize> {
        self.open.iter().position(|p| p.key() == key)
    }

    fn pinned(&self) -> usize {
        self.open.iter().filter(|p| p.pinned).count()
    }

    /// Open a panel (§2): on top of the unpinned group, expanded; already there, it moves to the
    /// top of its group and expands; a ninth closes the bottom unpinned one; eight pinned refuse.
    pub fn open(&mut self, p: Panel) -> Opened {
        if let Some(i) = self.position(&p.key()) {
            let mut cur = self.open.remove(i);
            cur.folded = false;
            if cur.opened_by.is_some() && p.opened_by.is_none() {
                cur.opened_by = None;
            }
            let at = if cur.pinned { i } else { self.pinned() };
            self.open.insert(at, cur);
            return Opened::Moved;
        }
        let mut evicted = None;
        if self.open.len() >= LIMIT {
            match self.open.iter().rposition(|q| !q.pinned) {
                Some(i) => {
                    let gone = self.open.remove(i);
                    self.remember(gone.clone());
                    evicted = Some(gone);
                }
                None => return Opened::Refused,
            }
        }
        let at = self.pinned();
        self.open.insert(at, Panel { folded: false, pinned: false, ..p });
        match evicted {
            Some(e) => Opened::Evicted(e),
            None => Opened::New,
        }
    }

    fn remember(&mut self, p: Panel) {
        self.closed.retain(|c| c.key() != p.key());
        self.closed.insert(0, p);
        self.closed.truncate(LIMIT);
    }

    /// Close a panel (§3.4); it can be reopened.
    pub fn close(&mut self, key: &str) -> Option<Panel> {
        let i = self.position(key)?;
        let p = self.open.remove(i);
        self.remember(p.clone());
        Some(p)
    }

    /// Close every unpinned panel (close all).
    pub fn close_unpinned(&mut self) {
        let gone: Vec<Panel> = self.open.iter().filter(|p| !p.pinned).cloned().collect();
        self.open.retain(|p| p.pinned);
        for p in gone.into_iter().rev() {
            self.remember(p);
        }
    }

    /// `⌥⇧T`: the last closed panel, back on top.
    pub fn reopen(&mut self) -> Option<Opened> {
        if self.closed.is_empty() {
            return None;
        }
        let p = self.closed.remove(0);
        Some(self.open(p))
    }

    /// `⌥P` (§3.2): pinned, it goes to the bottom of the pinned group; unpinned, to the top of the
    /// unpinned group.
    pub fn toggle_pin(&mut self, key: &str) -> Option<bool> {
        let i = self.position(key)?;
        let mut p = self.open.remove(i);
        p.pinned = !p.pinned;
        p.opened_by = None;
        let at = self.pinned();
        let pinned = p.pinned;
        self.open.insert(at, p);
        Some(pinned)
    }

    /// `⌥C` (§3.3).
    pub fn toggle_fold(&mut self, key: &str) -> Option<bool> {
        let i = self.position(key)?;
        let p = &mut self.open[i];
        p.folded = !p.folded;
        p.opened_by = None;
        Some(p.folded)
    }

    /// `⌥⇧K` / `⌥⇧J` (§3.5): one place up or down, within its group. The new index.
    pub fn move_by(&mut self, key: &str, up: bool) -> Option<usize> {
        let i = self.position(key)?;
        let j = if up { i.checked_sub(1)? } else { i + 1 };
        if j >= self.open.len() || self.open[j].pinned != self.open[i].pinned {
            return Some(i);
        }
        self.open.swap(i, j);
        Some(j)
    }
}

/// The places the sidebar shows in (§6.1): the column (120 and up), the drawer (90–119), and
/// replace (below 90). Which one shows is the screen's `when` on its width; each has its own
/// instance of every panel (`<place>_<tab>_<target>`).
pub const PLACES: &[&str] = &["sidebar", "drawer", "replace"];

/// The places over the view, narrow: an open there takes the focus (§2 rule 1, §6.4); beside,
/// in the column, it doesn't.
pub const OVER: &[&str] = &["drawer", "replace"];

/// What `thc ui aside <target>` opens (sidebar.md §10.3): `today` or a date is a day, `@today`
/// (`inbox`, `tasks`, `log`) a view, `#tag` or any other query a list, a page id or title a page.
pub fn panel_for_target(store: &thc_core::store::Store, target: &str, today: chrono::NaiveDate) -> anyhow::Result<Panel> {
    let t = target.trim();
    if t == "today" {
        return Ok(Panel::new(Kind::Day, "today"));
    }
    if let Some(v) = t.strip_prefix('@').filter(|v| ["today", "inbox", "tasks", "log"].contains(v)) {
        return Ok(Panel::new(Kind::View, v));
    }
    if t.starts_with('#') || t.starts_with('@') || t.contains(':') {
        return Ok(Panel::new(Kind::Query, t));
    }
    if let Ok(id) = store.resolve(t) {
        return Ok(Panel::new(Kind::Page, &id));
    }
    if let Some(id) = store.find_root_by_title(t, false)? {
        return Ok(Panel::new(Kind::Page, &id));
    }
    if let Ok(d) = thc_core::dates::parse(t, today) {
        return Ok(Panel::new(Kind::Day, &d.date().format("%Y-%m-%d").to_string()));
    }
    Err(thc_core::error::not_found(format!("no page or day {t:?}")))
}

/// One `thc ui aside` against a running screen, as the requests on its socket (sidebar.md
/// §10.3): from the stack as `read` returned it (`{rev, value}`), the write of the next stack
/// guarded by that revision (refused if the person changed it since: read again and retry),
/// then whether it shows, the active panel (never the focus, §10.4) and, for an agent, the
/// toast. An agent's panel says who opened it; `close` never closes a pinned one. The panel's
/// key and the requests, in order.
#[allow(clippy::too_many_arguments)]
pub fn aside_requests(store: &thc_core::store::Store, read: &serde_json::Value, target: &str, agent: Option<&str>, pin: bool, fold: bool, close: bool, today: chrono::NaiveDate) -> anyhow::Result<(String, Stack, Vec<serde_json::Value>)> {
    use serde_json::json;
    let mut stack = Stack::from_value(&read["value"]);
    let mut p = panel_for_target(store, target, today)?;
    let key = p.key();
    let by = agent.map(|a| format!("agent:{a}")).unwrap_or_else(|| "person".into());
    if close {
        if stack.open.iter().any(|q| q.key() == key && q.pinned) {
            return Err(thc_core::error::invalid(format!("{target} is pinned: only the person can close it")));
        }
        stack.close(&key);
    } else {
        p.opened_by = agent.map(|_| by.clone());
        stack.open(p);
        if let Some(q) = stack.position(&key) {
            if pin && !stack.open[q].pinned {
                stack.toggle_pin(&key);
            }
            if fold && !stack.open[stack.position(&key).unwrap_or(q)].folded {
                stack.toggle_fold(&key);
            }
        }
    }
    let put = |k: &str, value: serde_json::Value, if_rev: Option<u64>| json!({"op": "write", "key": k, "write": "put", "value": value, "if_rev": if_rev, "by": by});
    let mut reqs = vec![put("sidebar", stack.to_value(), read["rev"].as_u64()), put("sidebar_shown", true.into(), None), put("sidebar_beside", (!stack.open.is_empty()).into(), None)];
    if let Some(a) = stack.open.iter().find(|q| q.key() == key) {
        reqs.push(put("sidebar_active", a.id().into(), None));
        reqs.push(put("sidebar_target", a.target().into(), None));
    }
    if let (Some(a), false) = (agent, close) {
        reqs.push(json!({"op": "toast", "text": format!("◆ {a} opened {target} beside you"), "tone": "agent", "ms": 4000}));
    }
    Ok((key, stack, reqs))
}

/// Where a vault's stack is kept on this device (sidebar.md §11): its cache. Not the TUI's
/// `sidebar.json`, whose panels carry their editors' views.
pub fn file(cache: &std::path::Path) -> std::path::PathBuf {
    cache.join("screen-sidebar.json")
}

/// Keep the stack (atomically: a temp file, then a rename). The screen does on every change;
/// `thc ui aside` does for an agent's open.
pub fn save(cache: &std::path::Path, stack: &Stack) {
    let f = file(cache);
    let _ = std::fs::create_dir_all(cache);
    let tmp = f.with_extension("json.tmp");
    if serde_json::to_vec_pretty(&stack.to_value()).is_ok_and(|b| std::fs::write(&tmp, b).is_ok()) {
        let _ = std::fs::rename(&tmp, &f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(id: &str) -> Panel {
        Panel::new(Kind::Page, id)
    }

    fn keys(s: &Stack) -> Vec<String> {
        s.open.iter().map(Panel::key).collect()
    }

    /// S2: newest on top; opening again moves it up, never twice.
    #[test]
    fn s2_new_panels_go_on_top_and_reopening_moves_without_duplicating() {
        let mut s = Stack::default();
        assert_eq!(s.open(page("health")), Opened::New);
        assert_eq!(s.open(page("reading")), Opened::New);
        assert_eq!(keys(&s), vec!["page:reading", "page:health"]);
        s.toggle_fold("page:health");
        assert_eq!(s.open(page("health")), Opened::Moved);
        assert_eq!(keys(&s), vec!["page:health", "page:reading"]);
        assert!(!s.open[0].folded, "it expands");
    }

    /// S7: close and reopen; nine opens close the bottom unpinned; eight pinned refuse.
    #[test]
    fn s7_close_reopen_and_the_limit() {
        let mut s = Stack::default();
        s.open(page("a"));
        s.open(page("b"));
        s.close("page:b");
        assert_eq!(keys(&s), vec!["page:a"]);
        assert_eq!(s.reopen(), Some(Opened::New));
        assert_eq!(keys(&s), vec!["page:b", "page:a"]);
        for i in 0..6 {
            s.open(page(&format!("p{i}")));
        }
        assert_eq!(s.open.len(), 8);
        assert_eq!(s.open(page("ninth")), Opened::Evicted(page("a")));
        assert_eq!(s.closed[0].key(), "page:a");
        let mut all = Stack::default();
        for i in 0..8 {
            all.open(page(&format!("p{i}")));
            all.toggle_pin(&format!("page:p{i}"));
        }
        assert_eq!(all.open(page("more")), Opened::Refused);
        assert_eq!(all.open.len(), 8);
    }

    /// S14: a day panel moves to the day before or after, or to the relative today; a day that's
    /// already a panel isn't opened twice. Every panel has a stable id.
    #[test]
    fn s14_day_panels_change_day() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
        let mut s = Stack::default();
        s.open(Panel::new(Kind::Day, "2026-10-06"));
        assert_eq!(s.retarget_day("day:2026-10-06", Some(1), today).as_deref(), Some("day:2026-10-07"));
        assert_eq!(keys(&s), vec!["day:2026-10-07"]);
        assert_eq!(s.retarget_day("day:2026-10-07", None, today).as_deref(), Some("day:today"));
        assert_eq!(s.retarget_day("day:today", Some(-1), today).as_deref(), Some("day:2026-10-10"));
        s.open(Panel::new(Kind::Day, "2026-10-09"));
        assert_eq!(s.retarget_day("day:2026-10-10", Some(-1), today).as_deref(), Some("day:2026-10-09"), "that day is a panel already");
        assert_eq!(keys(&s), vec!["day:2026-10-09", "day:2026-10-10"]);
        assert_eq!(s.to_value()["open"][0]["id"], "day-2026-10-09");
    }

    /// S8, S9: a pinned panel stays at the top, survives close-all; reordering stays in a group.
    #[test]
    fn s8_s9_pins_and_reordering() {
        let mut s = Stack::default();
        s.open(Panel::new(Kind::View, "today"));
        s.open(page("a"));
        s.open(page("b"));
        assert_eq!(s.toggle_pin("view:today"), Some(true));
        assert_eq!(keys(&s), vec!["view:today", "page:b", "page:a"]);
        // A pinned panel doesn't move when opened again.
        s.open(Panel::new(Kind::View, "today"));
        assert_eq!(keys(&s)[0], "view:today");
        // Reordering within the unpinned group, never across.
        assert_eq!(s.move_by("page:a", true), Some(1));
        assert_eq!(keys(&s), vec!["view:today", "page:a", "page:b"]);
        assert_eq!(s.move_by("page:a", true), Some(1), "not into the pinned group");
        s.close_unpinned();
        assert_eq!(keys(&s), vec!["view:today"]);
        assert_eq!(s.toggle_pin("view:today"), Some(false));
    }
}
