//! `group:` in queries (views.md §3.2): presentation only. It changes how matches are laid
//! out, never which nodes match. Items keep the query's order. Groups are ordered by their
//! first item under the query's `sort:`; with no sort, pages A–Z, then journal days newest
//! first, then Inbox (tags A–Z). Due buckets, statuses and actors keep their fixed orders.

use crate::model::Node;
use crate::store::Store;
use anyhow::Result;
use chrono::NaiveDate;

pub const BY: &[&str] = &["parent", "tag", "status", "due", "actor", "vault"];

#[derive(Debug, Clone)]
pub struct Group {
    /// Stable key: a node id for `parent`, else the bucket name (`overdue`, `work`, `todo`).
    pub key: String,
    /// `¶ Q4 Planning`, `§ today`, `Overdue`, `#work`, `◆ claude`.
    pub label: String,
    /// True for the Overdue bucket, which is drawn in the overdue colour as everywhere else.
    pub overdue: bool,
    pub items: Vec<Node>,
}

#[derive(Debug, Default)]
pub struct Grouped {
    pub groups: Vec<Group>,
    /// `group:tag` put a node under more than one tag.
    pub repeated: bool,
}

/// The page or journal day a node lives in: its nearest titled or journal ancestor.
/// None for inbox trees; `Some(self)` is never returned.
pub fn container(store: &Store, n: &Node) -> Option<Node> {
    let mut cur = n.parent.clone();
    let mut hops = 0;
    while let Some(id) = cur {
        let p = store.node(&id).ok().flatten()?;
        if p.title.is_some() || p.journal.is_some() {
            return Some(p);
        }
        cur = p.parent.clone();
        hops += 1;
        if hops > 64 {
            break;
        }
    }
    None
}

/// `¶ Q4 Planning`, `§ today`, `§ Sat Oct 3`.
pub fn container_label(c: &Node, today: NaiveDate) -> String {
    match c.journal.as_deref().and_then(|j| NaiveDate::parse_from_str(j, "%Y-%m-%d").ok()) {
        Some(d) if (d - today).num_days().abs() <= 1 => format!("§ {}", crate::dates::relative(d, today)),
        Some(d) => format!("§ {}", d.format("%a %b %-d")),
        None => format!("¶ {}", c.label()),
    }
}

fn push(groups: &mut Vec<Group>, key: &str, label: impl FnOnce() -> String, n: Node) {
    match groups.iter_mut().find(|g| g.key == key) {
        Some(g) => g.items.push(n),
        None => groups.push(Group { key: key.to_string(), label: label(), overdue: key == "overdue", items: vec![n] }),
    }
}

fn order_by(groups: &mut [Group], order: &[&str]) {
    groups.sort_by_key(|g| order.iter().position(|k| *k == g.key).unwrap_or(order.len()));
}

/// `sorted`: the query had a `sort:` term (groups then follow their first item).
pub fn group(store: &Store, nodes: Vec<Node>, by: &str, today: NaiveDate, sorted: bool) -> Result<Grouped> {
    let mut out = Grouped::default();
    let g = &mut out.groups;
    match by {
        "parent" => {
            for n in nodes {
                match container(store, &n) {
                    Some(c) => {
                        let label = container_label(&c, today);
                        push(g, &c.id, || label, n);
                    }
                    // Top-level pages and inbox items have no container.
                    None if n.parent.is_none() && n.title.is_some() => push(g, "top", || "top level".into(), n),
                    None => push(g, "inbox", || "Inbox".into(), n),
                }
            }
            if !sorted {
                // Pages A–Z, journal days newest first, then top level, then Inbox.
                let rank = |x: &Group| -> (u8, String) {
                    match x.key.as_str() {
                        "top" => (2, String::new()),
                        "inbox" => (3, String::new()),
                        id => match store.node(id).ok().flatten() {
                            Some(c) if c.journal.is_some() => {
                                // Newest first: invert the date's digits so an ascending sort works.
                                let j: String = c.journal.unwrap_or_default().chars().map(|ch| ch.to_digit(10).map(|d| char::from(b'9' - d as u8)).unwrap_or(ch)).collect();
                                (1, j)
                            }
                            _ => (0, x.label.to_lowercase()),
                        },
                    }
                };
                g.sort_by_cached_key(rank);
            }
        }
        "tag" => {
            for n in nodes {
                let tags = store.tags_of(&n.id).unwrap_or_default();
                if tags.len() > 1 {
                    out.repeated = true;
                }
                if tags.is_empty() {
                    push(g, "", || "no tag".into(), n);
                    continue;
                }
                for t in tags {
                    let t = t.to_lowercase();
                    push(g, &t, || format!("#{t}"), n.clone());
                }
            }
            if !sorted {
                g.sort_by(|a, b| a.key.cmp(&b.key));
            }
            // Untagged last.
            let none = g.iter().position(|x| x.key.is_empty());
            if let Some(i) = none {
                let x = g.remove(i);
                g.push(x);
            }
        }
        "status" => {
            for n in nodes {
                let s = n.status.clone().unwrap_or_default();
                let label = if s.is_empty() { "no status".to_string() } else { s.clone() };
                push(g, &s, || label, n);
            }
            order_by(g, &["doing", "todo", "waiting", "done", "cancelled", ""]);
        }
        "due" => {
            let week_end = today + chrono::Duration::days(6);
            for n in nodes {
                let d = n.due.as_deref().and_then(|d| NaiveDate::parse_from_str(&d[..d.len().min(10)], "%Y-%m-%d").ok());
                let (key, label) = match d {
                    // Literal: a scheduled date with no deadline is still "no due date" (red is for deadlines).
                    None => ("none", "No due date"),
                    Some(d) if d < today && n.is_open() => ("overdue", "Overdue"),
                    // A closed task that was due in the past isn't overdue; it's just earlier.
                    Some(d) if d < today => ("earlier", "Earlier"),
                    Some(d) if d == today => ("today", "Today"),
                    Some(d) if d == today.succ_opt().unwrap_or(today) => ("tomorrow", "Tomorrow"),
                    Some(d) if d <= week_end => ("week", "This week"),
                    Some(_) => ("later", "Later"),
                };
                push(g, key, || label.into(), n);
            }
            order_by(g, &["overdue", "earlier", "today", "tomorrow", "week", "later", "none"]);
        }
        "actor" => {
            for n in nodes {
                let (key, label) = match n.created_by.strip_prefix("agent:") {
                    Some(a) => (n.created_by.clone(), format!("◆ {a}")),
                    None => ("human".to_string(), "human".to_string()),
                };
                push(g, &key, || label, n);
            }
            // Human first, then agents in the order they appear.
            if let Some(i) = g.iter().position(|x| x.key == "human") {
                let x = g.remove(i);
                g.insert(0, x);
            }
        }
        // One vault's rows are one group (the caller names it); across vaults, the federated
        // query groups by the vault each row came from (vaults.md §3.1).
        "vault" => {
            for n in nodes {
                push(g, "vault", || "this vault".into(), n);
            }
        }
        other => return Err(crate::query::unknown_value("group", other, BY)),
    }
    Ok(out)
}
