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

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
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
