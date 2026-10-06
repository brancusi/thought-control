//! Queries across vaults (vaults.md §3.1, vaults-architecture.md §3): `vault:acme`,
//! `vault:(acme or personal)`, `vault:*`, `-vault:side`.
//!
//! Federated, not ATTACH: each vault's own store runs the query (its ids, tags and statistics
//! are its own), one thread per vault, and the rows are merged by the query's own ORDER BY in an
//! in-memory table, so the order is exactly a single vault's.

use crate::error::invalid;
use crate::event::Actor;
use crate::model::Node;
use crate::registry::{Entry, Registry};
use crate::vault::{Paths, Vault};
use anyhow::Result;
use chrono::NaiveDate;

/// Which vaults a query reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    pub all: bool,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

/// Split a query's top-level `vault:` terms from the rest. None: the query names no vault.
pub fn split_scope(q: &str) -> Result<(Option<Scope>, String)> {
    let mut scope = Scope::default();
    let mut found = false;
    let mut rest: Vec<String> = Vec::new();
    for tok in top_tokens(q) {
        let (neg, body) = match tok.strip_prefix('-') {
            Some(b) => (true, b),
            None => (false, tok.as_str()),
        };
        let Some(v) = body.strip_prefix("vault:") else {
            if tok.contains("vault:") && (tok.starts_with('(') || tok.starts_with("-(")) {
                return Err(invalid("vault: goes at the top of a query, not inside ( … ) · e.g. vault:(acme or personal) status:open".to_string()));
            }
            rest.push(tok);
            continue;
        };
        found = true;
        let names: Vec<String> = match v.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
            Some(inner) => inner.split_whitespace().filter(|w| *w != "or").map(str::to_string).collect(),
            None => vec![v.to_string()],
        };
        for n in names {
            if n == "*" {
                if neg {
                    return Err(invalid("-vault:* would leave nothing · name the vaults to leave out".to_string()));
                }
                scope.all = true;
            } else if neg {
                scope.exclude.push(n);
            } else {
                scope.include.push(n);
            }
        }
    }
    Ok((found.then_some(scope), rest.join(" ")))
}

/// The query's top-level tokens: words, with parentheses and quotes kept whole.
fn top_tokens(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut depth, mut quote) = (0i32, false);
    for c in q.chars() {
        match c {
            '"' => {
                quote = !quote;
                cur.push(c);
            }
            '(' if !quote => {
                depth += 1;
                cur.push(c);
            }
            ')' if !quote => {
                depth -= 1;
                cur.push(c);
            }
            c if c.is_whitespace() && depth <= 0 && !quote => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The registered vaults a scope names, in registry order (home first), and the names of
/// those this device doesn't have (their folder is missing). A name that isn't registered
/// exits 6 with a suggestion. With no `vault:` term at all the caller reads only its own.
pub fn resolve(scope: &Scope, reg: &Registry, current: &str) -> Result<(Vec<Entry>, Vec<String>)> {
    for n in scope.include.iter().chain(&scope.exclude) {
        if reg.find(n).is_none() {
            let near = reg.vaults.iter().map(|e| e.name.as_str()).find(|e| strsim(e, n) <= 2).map(|e| format!(" · did you mean {e}?")).unwrap_or_default();
            return Err(invalid(format!("no vault \"{n}\"{near} · thc vault ls")));
        }
    }
    let mut picked: Vec<Entry> = reg
        .vaults
        .iter()
        .filter(|e| scope.all || scope.include.contains(&e.name) || (scope.include.is_empty() && e.name == current))
        .filter(|e| !scope.exclude.contains(&e.name))
        .cloned()
        .collect();
    // `-vault:side` alone: everything in scope (the current vault) minus side.
    if picked.is_empty() && !scope.all && scope.include.is_empty() {
        picked = reg.vaults.iter().filter(|e| e.name == current && !scope.exclude.contains(&e.name)).cloned().collect();
    }
    let (here, missing): (Vec<Entry>, Vec<Entry>) = picked.into_iter().partition(|e| e.path.join(crate::vault::VAULT_MARKER).exists());
    Ok((here, missing.into_iter().map(|e| e.name).collect()))
}

/// One vault's part of a federated query: its store (kept open, to render its rows) and rows.
pub struct Part {
    pub name: String,
    pub vault: Vault,
    pub nodes: Vec<Node>,
}

/// Run `q` (with no `vault:` terms left) in each vault, in parallel. `paths_for` gives a
/// vault's folder and cache (the current one's may differ from the default, e.g. in tests).
pub fn run(entries: &[Entry], paths_for: &(dyn Fn(&Entry) -> Paths + Sync), actor: &Actor, q: &str, today: NaiveDate, limit: usize) -> Result<Vec<Part>> {
    let results: Vec<Result<Part>> = std::thread::scope(|s| {
        let handles: Vec<_> = entries
            .iter()
            .map(|e| {
                let paths = paths_for(e);
                let actor = actor.clone();
                let name = e.name.clone();
                s.spawn(move || -> Result<Part> {
                    let vault = Vault::open(paths, actor, "cli")?;
                    let recipient = crate::messages::Recipient::current(Some(vault.actor.name.as_deref().unwrap_or("human")));
                    let nodes = crate::messages::query(&vault.store, q, today, limit, &recipient)?;
                    Ok(Part { name, vault, nodes })
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("a vault's query panicked")))).collect()
    });
    results.into_iter().collect()
}

/// The rows of every part in the query's own order, as (part, row) indices, at most `limit`.
pub fn merge(parts: &[Part], order_sql: &str, limit: usize) -> Result<Vec<(usize, usize)>> {
    let lists: Vec<&[Node]> = parts.iter().map(|p| p.nodes.as_slice()).collect();
    let stores: Vec<&crate::store::Store> = parts.iter().map(|p| &p.vault.store).collect();
    merge_lists(&lists, &stores, order_sql, limit)
}

/// `merge` for node lists from stores the caller already holds (a sectioned view's sections).
/// Each list comes from the corresponding store, queried in `order_sql` order.
pub fn merge_lists(lists: &[&[Node]], stores: &[&crate::store::Store], order_sql: &str, limit: usize) -> Result<Vec<(usize, usize)>> {
    anyhow::ensure!(lists.len() == stores.len(), "each merge list needs its source store");
    // Outline order depends on ancestors that may not match the query. Evaluate its key in
    // the source store, then sort those keys in memory. IDs can overlap between vaults.
    let outline = order_sql.contains(crate::query::ORDER_KEY);
    let order_sql = order_sql.replace(crate::query::ORDER_KEY, "n.outline_order");
    let mem = rusqlite::Connection::open_in_memory()?;
    mem.execute_batch(
        "CREATE TABLE n(part INTEGER, row INTEGER, id TEXT, title TEXT, status TEXT, scheduled TEXT, due TEXT, priority TEXT, done_at TEXT, created_ms INTEGER, updated_ms INTEGER, outline_order TEXT)",
    )?;
    {
        let mut st = mem.prepare("INSERT INTO n VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)")?;
        let key_sql = format!("SELECT {} FROM nodes n WHERE n.id=?1", crate::query::ORDER_KEY);
        for (pi, p) in lists.iter().enumerate() {
            let mut key = if outline { Some(stores[pi].conn.prepare(&key_sql)?) } else { None };
            for (ri, x) in p.iter().enumerate() {
                let outline_order: Option<String> = match &mut key {
                    Some(key) => key.query_row([&x.id], |r| r.get(0))?,
                    None => None,
                };
                st.execute(rusqlite::params![pi as i64, ri as i64, x.id, x.title, x.status, x.scheduled, x.due, x.priority, x.done_at, x.created_ms, x.updated_ms, outline_order])?;
            }
        }
    }
    // Ties keep each vault's own order, vaults in registry order.
    let sql = format!("SELECT part, row FROM n {order_sql}, part, row LIMIT {limit}");
    let mut st = mem.prepare(&sql)?;
    let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn strsim(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_terms_split_off_the_top_level() {
        let (s, rest) = split_scope("vault:(acme or personal) status:open -vault:side (due<=+3d or !high)").unwrap();
        let s = s.unwrap();
        assert_eq!(s.include, ["acme", "personal"]);
        assert_eq!(s.exclude, ["side"]);
        assert_eq!(rest, "status:open (due<=+3d or !high)");
        let (s, rest) = split_scope("vault:* #work").unwrap();
        assert!(s.unwrap().all && rest == "#work");
        assert!(split_scope("status:open").unwrap().0.is_none());
        assert!(split_scope("(vault:acme or #x)").is_err());
    }
}

/// A sectioned view's rows: each section's query in each store, merged in that query's own
/// order, as (store index, node). `stores` are the vaults in the view's scope (index 0 first).
pub fn run_sections(sections: &[crate::views::Section], stores: &[&crate::store::Store], today: NaiveDate, limit: usize) -> Result<Vec<Vec<(usize, Node)>>> {
    let mut out = Vec::new();
    for sec in sections {
        let mut lists: Vec<Vec<Node>> = Vec::new();
        for s in stores {
            lists.push(s.query(&sec.query, today, limit)?);
        }
        let Some(first) = stores.first() else {
            out.push(vec![]);
            continue;
        };
        let order = crate::query::compile(&sec.query, first, today)?.order_sql;
        let refs: Vec<&[Node]> = lists.iter().map(|l| l.as_slice()).collect();
        let picked = merge_lists(&refs, stores, &order, limit)?;
        out.push(picked.into_iter().map(|(si, ri)| (si, lists[si][ri].clone())).collect());
    }
    Ok(out)
}
