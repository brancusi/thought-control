//! The `thc q` query language, compiled to one SQL WHERE clause over `nodes n`.
//!
//! ```text
//! expr  := term (("and"|"or") term)*        ; implicit "and"
//! term  := "-"? atom | "(" expr ")" | "-"? ("ancestor:(" | "has:child(") expr ")"
//! atom  := status:<s> | #tag | !high | group:<by> | happens<op><date> | has:child | under:<id> | is:<kind> | <field><op><value> | text:<word> | sort:<field>[-] | <word>
//! ```
//!
//! Alongside the SQL the parser builds an [`Ast`] that says each term in plain words, for
//! `thc q --explain` (views.md §3.1).

use crate::dates;
use crate::error::invalid;
use crate::store::Store;
use anyhow::Result;
use chrono::{Datelike, NaiveDate};
use rusqlite::types::Value as SqlValue;

#[derive(Debug, Default)]
pub struct Compiled {
    pub where_sql: String,
    pub params: Vec<SqlValue>,
    pub order_sql: String,
}

/// The query in plain words (views.md §3.1). Every term carries what it means, with relative
/// dates resolved, and a terse form for the TUI's one-line meaning.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Ast {
    And { terms: Vec<Ast> },
    Or { terms: Vec<Ast> },
    Not { term: Box<Ast> },
    Term { text: String, means: String, short: String },
}

impl Ast {
    fn term(text: &str, means: impl Into<String>, short: impl Into<String>) -> Ast {
        Ast::Term { text: text.to_string(), means: means.into(), short: short.into() }
    }

    fn negate(self) -> Ast {
        match self {
            Ast::Not { term } => *term,
            t => Ast::Not { term: Box::new(t) },
        }
    }

    /// `due on or before Tue Oct 6, or priority high` (nested groups in parentheses).
    pub fn means(&self) -> String {
        self.render(false, false)
    }

    /// `due by Tue Oct 6 or !high`.
    pub fn short(&self) -> String {
        self.render(true, false)
    }

    fn render(&self, short: bool, nested: bool) -> String {
        let wrap = |s: String| if nested { format!("({s})") } else { s };
        match self {
            Ast::Term { means, short: sh, .. } => if short { sh.clone() } else { means.clone() },
            Ast::Not { term } => {
                // `not tagged #someday`; a group keeps its parentheses: `not (a or b)`.
                format!("not {}", term.render(short, true))
            }
            Ast::And { terms } => wrap(terms.iter().map(|t| t.render(short, true)).collect::<Vec<_>>().join(" and ")),
            Ast::Or { terms } => wrap(terms.iter().map(|t| t.render(short, true)).collect::<Vec<_>>().join(if short { " or " } else { ", or " })),
        }
    }

    /// The `means` block: one line per top-level term, `and …` / `or …` after the first.
    pub fn lines(&self) -> Vec<String> {
        match self {
            Ast::And { terms } => terms.iter().enumerate().map(|(i, t)| if i == 0 { t.render(false, true) } else { format!("and {}", t.render(false, true)) }).collect(),
            Ast::Or { terms } => terms.iter().enumerate().map(|(i, t)| if i == 0 { t.render(false, true) } else { format!("or {}", t.render(false, true)) }).collect(),
            t => vec![t.means()],
        }
    }
}

/// What `--explain` shows: the meaning, the SQL, and the views it expanded.
#[derive(Debug)]
pub struct Explained {
    pub compiled: Compiled,
    pub ast: Option<Ast>,
    /// `sorted by due date, undated last`, one per `sort:` term.
    pub sorts: Vec<String>,
    pub views: Vec<crate::views::View>,
    /// `group:parent` etc. (views.md §3.2): presentation only, so it isn't in the SQL.
    pub group: Option<String>,
}

impl Explained {
    pub fn lines(&self) -> Vec<String> {
        let mut v = self.ast.as_ref().map(|a| a.lines()).unwrap_or_else(|| vec!["everything".into()]);
        v.extend(self.sorts.iter().cloned());
        if let Some(g) = &self.group {
            v.push(format!("grouped by {g}"));
        }
        v
    }

    /// `open tasks · due by Tue Oct 6 or !high · #work · not #someday · by due date` (TUI row 3).
    pub fn short(&self) -> String {
        let mut parts: Vec<String> = match &self.ast {
            Some(Ast::And { terms }) => terms.iter().map(|t| t.render(true, false)).collect(),
            Some(a) => vec![a.short()],
            None => vec!["everything".into()],
        };
        parts.extend(self.sorts.iter().map(|s| s.trim_start_matches("sorted ").split(',').next().unwrap_or("").to_string()));
        if let Some(g) = &self.group {
            parts.push(format!("grouped by {g}"));
        }
        parts.join(" · ")
    }

    /// The SQL with its parameters inlined, for reading (`--explain=sql`).
    pub fn sql(&self) -> String {
        let mut sql = format!("SELECT … FROM nodes n WHERE {} {}", self.compiled.where_sql, self.compiled.order_sql);
        // Highest index first so ?1 doesn't eat ?10.
        for (i, p) in self.compiled.params.iter().enumerate().rev() {
            let lit = match p {
                SqlValue::Null => "NULL".to_string(),
                SqlValue::Integer(n) => n.to_string(),
                SqlValue::Real(f) => f.to_string(),
                SqlValue::Text(t) => format!("'{}'", t.replace('\'', "''")),
                SqlValue::Blob(_) => "x''".to_string(),
            };
            sql = sql.replace(&format!("?{}", i + 1), &lit);
        }
        sql
    }
}

/// A query that didn't parse, with where in the query it went wrong (byte offsets), when known.
#[derive(Debug)]
pub struct QueryError {
    pub error: anyhow::Error,
    pub span: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    LParen,
    Not,
    RParen,
    And,
    Or,
    Atom(String),
    /// `ancestor:(` / `has:child(`: a structural sub-query; its body runs to the matching `)`.
    Sub(String),
}

/// Marks a tag test (`\u{1}?3\u{1}`), settled per scope by [`probe`].
const TAG_PROBE: char = '\u{1}';

/// Marks a date column (`\u{2}n.due`), settled per scope by [`probe`]: with a selective
/// open-status term it becomes `+n.due`, so SQLite stays on the small open-task index instead
/// of range-scanning every dated node (`status:open due<=+3d`: 12 ms → 5.6 at 50k).
const DATE_COL: char = '\u{2}';

/// `sort:order`: the outline path to a note, as one string that sorts in outline order.
pub(crate) const ORDER_KEY: &str = "(WITH RECURSIVE up(id, parent, k) AS (SELECT n.id, n.parent, n.ord || char(1) || n.id \
    UNION ALL SELECT x.id, x.parent, x.ord || char(1) || x.id || char(2) || up.k FROM nodes x JOIN up ON x.id = up.parent) \
    SELECT k FROM up WHERE parent IS NULL)";

/// Settle one scope's tag tests and date columns. With a selective open-status term in the same scope, a
/// correlated EXISTS lets SQLite scan the small open-task index and check each row's tags;
/// without one, an uncorrelated IN builds the tagged set once and drives from it. Measured at
/// 50k nodes: `status:open #work sort:due` 5.6 ms (EXISTS) vs 9.8 (IN); `#work` 11 ms (IN) vs
/// 32 (EXISTS); `#nosuchtag` 4.6 (IN) vs 31 (EXISTS).
fn probe(sql: &str, selective: bool) -> String {
    let sql = sql.replace(DATE_COL, if selective { "+" } else { "" });
    let sql = sql.as_str();
    let mut out = String::with_capacity(sql.len() + 64);
    let mut parts = sql.split(TAG_PROBE);
    out.push_str(parts.next().unwrap_or(""));
    while let (Some(p), Some(rest)) = (parts.next(), parts.next()) {
        out.push_str(&if selective {
            format!("EXISTS (SELECT 1 FROM edges e JOIN nodes t ON t.id=e.dst WHERE e.src=n.id AND e.rel='tag' AND t.title={p} COLLATE NOCASE)")
        } else {
            format!("n.id IN (SELECT e.src FROM edges e JOIN nodes t ON t.id=e.dst WHERE e.rel='tag' AND t.title={p} COLLATE NOCASE)")
        });
        out.push_str(rest);
    }
    out
}

/// Tokens with their byte spans in `s`.
fn lex(s: &str) -> Vec<(Tok, (usize, usize))> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut at = 0;
    let mut quote = false;
    let flush = |cur: &mut String, out: &mut Vec<(Tok, (usize, usize))>, at: usize| {
        if cur.is_empty() {
            return;
        }
        let w = std::mem::take(cur);
        let span = (at, at + w.len());
        out.push((
            match w.to_lowercase().as_str() {
                "and" | "&&" => Tok::And,
                "or" | "||" => Tok::Or,
                _ => Tok::Atom(w),
            },
            span,
        ));
    };
    for (i, c) in s.char_indices() {
        if cur.is_empty() {
            at = i;
        }
        match c {
            '"' => {
                quote = !quote;
                cur.push(c)
            }
            '(' if !quote && cur.is_empty() => out.push((Tok::LParen, (i, i + 1))),
            // `ancestor:( … )`, `has:child( … )`, and their negations.
            '(' if !quote && matches!(cur.trim_start_matches('-').to_lowercase().as_str(), "ancestor:" | "has:child") => {
                let w = std::mem::take(&mut cur);
                if w.starts_with('-') {
                    out.push((Tok::Not, (at, at + 1)));
                }
                let name = w.trim_start_matches('-').trim_end_matches(':').to_lowercase();
                out.push((Tok::Sub(name), (at, i + 1)));
            }
            // `-( … )` negates a group (and `-@view`, which expands to one).
            '(' if !quote && cur == "-" => {
                cur.clear();
                out.push((Tok::Not, (i - 1, i)));
                out.push((Tok::LParen, (i, i + 1)));
            }
            ')' if !quote => {
                flush(&mut cur, &mut out, at);
                out.push((Tok::RParen, (i, i + 1)))
            }
            c if c.is_whitespace() && !quote => flush(&mut cur, &mut out, at),
            c => cur.push(c),
        }
    }
    flush(&mut cur, &mut out, at);
    out
}

struct Parser<'a> {
    toks: Vec<Tok>,
    spans: Vec<(usize, usize)>,
    pos: usize,
    params: Vec<SqlValue>,
    order: Vec<String>,
    sorts: Vec<String>,
    group: Option<String>,
    /// This scope has a selective open-status term (status:open/todo/doing/waiting, is:ready,
    /// is:overdue): see [`probe`].
    selective: bool,
    /// The query asks for attachment nodes (is:image, is:file, is:attachment): they're system
    /// nodes, hidden otherwise.
    attachments: bool,
    today: NaiveDate,
    store: &'a Store,
    recipient: &'a crate::messages::Recipient,
}

pub fn compile(query: &str, store: &Store, today: NaiveDate) -> Result<Compiled> {
    explain(query, store, today).map(|e| e.compiled).map_err(|e| e.error)
}

/// Compile and explain. On error, `span` points into `query` (when no view was expanded).
pub fn explain(query: &str, store: &Store, today: NaiveDate) -> std::result::Result<Explained, QueryError> {
    explain_for(query, store, today, &crate::messages::Recipient::current(None))
}

/// Compile with an explicit message recipient (`--actor`, prime and team listings).
pub fn explain_for(query: &str, store: &Store, today: NaiveDate, recipient: &crate::messages::Recipient) -> std::result::Result<Explained, QueryError> {
    // Saved views (`@work`) expand to their queries first (views.md §1.2).
    let (expanded, views) = crate::views::expand(store, query).map_err(|error| QueryError { error, span: None })?;
    let lexed = lex(&expanded);
    let (toks, spans) = lexed.into_iter().unzip();
    let mut p = Parser { toks, spans, pos: 0, params: vec![], order: vec![], sorts: vec![], group: None, selective: false, attachments: false, today, store, recipient };
    let fail = |p: &Parser, error: anyhow::Error| {
        // Spans are only meaningful against the text the person typed.
        let span = (expanded == query).then(|| p.spans.get(p.pos.saturating_sub(1)).or(p.spans.last()).copied()).flatten();
        QueryError { error, span }
    };
    let expr = if p.toks.is_empty() {
        None
    } else {
        match p.expr() {
            Ok(e) => e,
            Err(e) => return Err(fail(&p, e)),
        }
    };
    if p.pos < p.toks.len() {
        p.pos += 1;
        let e = invalid(format!("unexpected {} in query", describe(&p.toks[p.pos - 1])));
        return Err(fail(&p, e));
    }
    // System pages (¶ Views) and their children never show up in queries; attachment nodes do
    // when asked for (FORMAT.md "Attachments").
    let hidden = if p.attachments {
        "n.id NOT IN (SELECT node FROM props WHERE key = 'system' AND value != '\"attachment\"') \
         AND (n.parent IS NULL OR n.parent NOT IN (SELECT node FROM props WHERE key = 'system'))"
            .to_string()
    } else {
        crate::views::HIDDEN_SQL.to_string()
    };
    let base = format!("n.deleted=0 AND n.is_tag=0 AND {hidden}");
    let (where_sql, ast) = match expr {
        Some((e, a)) => (format!("{base} AND ({})", probe(&e, p.selective)), Some(a)),
        None => (base.clone(), None),
    };
    let order_sql = if p.order.is_empty() {
        "ORDER BY coalesce(n.scheduled, n.due) IS NULL, coalesce(n.scheduled, n.due), n.updated_ms DESC".to_string()
    } else {
        format!("ORDER BY {}", p.order.join(", "))
    };
    Ok(Explained { compiled: Compiled { where_sql, params: p.params, order_sql }, ast, sorts: p.sorts, views, group: p.group })
}

/// The typed terms of an AST, space-separated (for a sub-query's `text`).
fn ast_text(a: &Ast) -> String {
    match a {
        Ast::Term { text, .. } => text.clone(),
        Ast::Not { term } => format!("-{}", ast_text(term)),
        Ast::And { terms } => terms.iter().map(ast_text).collect::<Vec<_>>().join(" "),
        Ast::Or { terms } => format!("({})", terms.iter().map(ast_text).collect::<Vec<_>>().join(" or ")),
    }
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::LParen => "\"(\"".into(),
        Tok::RParen => "\")\"".into(),
        Tok::Not => "\"-\"".into(),
        Tok::And => "\"and\"".into(),
        Tok::Or => "\"or\"".into(),
        Tok::Atom(a) => format!("{a:?}"),
        Tok::Sub(n) => format!("\"{n}(\""),
    }
}

/// `Tue Oct 6`, with the year when it isn't this year's.
/// Whether [`date_range`] refers to the day after for this operator.
fn needs_next(op: &str) -> bool {
    !matches!(op, "<" | ">=")
}

/// The day after a `YYYY-MM-DD` string.
fn next_day(d: &str) -> String {
    NaiveDate::parse_from_str(d, "%Y-%m-%d").ok().and_then(|x| x.succ_opt()).map(|x| x.format("%Y-%m-%d").to_string()).unwrap_or_else(|| d.to_string())
}

fn day(d: &str, today: NaiveDate) -> String {
    match NaiveDate::parse_from_str(d, "%Y-%m-%d") {
        Ok(x) if x.year() == today.year() => x.format("%a %b %-d").to_string(),
        Ok(x) => x.format("%a %b %-d %Y").to_string(),
        Err(_) => d.to_string(),
    }
}

/// A date comparison as an ISO range, so SQLite can use the by_due/by_sched/by_done
/// indexes (`substr(col,1,10) <= ?` can't). `day` and `next` are bound parameters for the
/// day and the day after; stored values are `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM`.
fn date_range(col: &str, op: &str, day: &str, next: &str) -> String {
    match op {
        "<=" => format!("{col} < {next}"),
        "<" => format!("{col} < {day}"),
        ">=" => format!("{col} >= {day}"),
        ">" => format!("{col} >= {next}"),
        "!=" => format!("NOT ({col} >= {day} AND {col} < {next})"),
        _ => format!("({col} >= {day} AND {col} < {next})"),
    }
}

/// `on or before`, for a comparison against a date.
fn date_words(op: &str) -> &'static str {
    match op {
        "<=" => "on or before",
        ">=" => "on or after",
        "<" => "before",
        ">" => "after",
        "!=" => "not on",
        _ => "on",
    }
}

fn op_words(op: &str) -> &'static str {
    match op {
        "<=" => "at most",
        ">=" => "at least",
        "<" => "below",
        ">" => "above",
        "!=" => "not",
        _ => "",
    }
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn expr(&mut self) -> Result<Option<(String, Ast)>> {
        let mut parts: Vec<(String, Ast)> = Vec::new();
        let mut ops: Vec<&str> = Vec::new();
        loop {
            match self.peek() {
                None | Some(Tok::RParen) => break,
                Some(Tok::And) => {
                    self.pos += 1;
                    if !parts.is_empty() {
                        ops.push("AND");
                    }
                    continue;
                }
                Some(Tok::Or) => {
                    self.pos += 1;
                    if !parts.is_empty() {
                        ops.push("OR");
                    }
                    continue;
                }
                _ => {}
            }
            let term = self.term()?;
            if let Some(t) = term {
                if parts.len() > ops.len() {
                    ops.push("AND");
                }
                parts.push(t);
            }
        }
        if parts.is_empty() {
            return Ok(None);
        }
        // AND binds tighter than OR.
        let mut or_groups: Vec<Vec<(String, Ast)>> = vec![vec![parts[0].clone()]];
        for (i, op) in ops.iter().enumerate().take(parts.len() - 1) {
            if *op == "OR" {
                or_groups.push(vec![]);
            }
            or_groups.last_mut().unwrap().push(parts[i + 1].clone());
        }
        let s = or_groups.iter().map(|g| format!("({})", g.iter().map(|x| x.0.as_str()).collect::<Vec<_>>().join(" AND "))).collect::<Vec<_>>().join(" OR ");
        let and = |g: Vec<(String, Ast)>| {
            // `(a and b) and c` reads as three terms (a view expands to a group).
            let mut terms: Vec<Ast> = g
                .into_iter()
                .flat_map(|x| match x.1 {
                    Ast::And { terms } => terms,
                    t => vec![t],
                })
                .collect();
            if terms.len() == 1 { terms.pop().unwrap() } else { Ast::And { terms } }
        };
        let mut groups: Vec<Ast> = or_groups.into_iter().map(and).collect();
        let ast = if groups.len() == 1 {
            groups.pop().unwrap()
        } else {
            Ast::Or { terms: groups.into_iter().flat_map(|t| match t {
                Ast::Or { terms } => terms,
                t => vec![t],
            }).collect() }
        };
        Ok(Some((s, ast)))
    }

    fn term(&mut self) -> Result<Option<(String, Ast)>> {
        match self.toks.get(self.pos).cloned() {
            Some(Tok::Not) => {
                self.pos += 1;
                Ok(self.term()?.map(|(t, a)| (format!("NOT {t}"), a.negate())))
            }
            Some(Tok::LParen) => {
                self.pos += 1;
                let inner = self.expr()?;
                if self.peek() != Some(&Tok::RParen) {
                    return Err(invalid("missing ) in query"));
                }
                self.pos += 1;
                Ok(inner.map(|(i, a)| (format!("({i})"), a)))
            }
            Some(Tok::Sub(kind)) => {
                self.pos += 1;
                // The body is a query over the same columns, evaluated in a subquery whose own
                // `nodes n` shadows the outer one, and built once (uncorrelated). It's its own
                // scope for tag probing.
                let outer = std::mem::replace(&mut self.selective, false);
                let inner = self.expr();
                let inner_sel = std::mem::replace(&mut self.selective, outer);
                let inner = inner?.map(|(b, a)| (probe(&b, inner_sel), a));
                if self.peek() != Some(&Tok::RParen) {
                    return Err(invalid(format!("missing ) after {kind}(")));
                }
                self.pos += 1;
                let Some((body, ast)) = inner else { return Err(invalid(format!("{kind}( ) needs a query inside"))) };
                let set = format!("SELECT n.id FROM nodes n WHERE n.deleted=0 AND ({body})");
                let (means, short) = (ast.means(), ast.short());
                Ok(Some(if kind == "ancestor" {
                    (
                        // Walk up from each candidate (a few hops) and test against the matching set,
                        // which is built once. Walking down from the matches visits every descendant:
                        // 87 ms vs 2 ms for `ancestor:(#project) status:open` at 50k nodes.
                        format!("EXISTS (WITH RECURSIVE up(id) AS (SELECT n.parent UNION ALL SELECT x.parent FROM nodes x JOIN up ON x.id=up.id WHERE x.parent IS NOT NULL) SELECT 1 FROM up WHERE up.id IN ({set}))"),
                        Ast::term(&format!("ancestor:({})", ast_text(&ast)), format!("under something that is: {means}"), format!("under ({short})")),
                    )
                } else {
                    (
                        // `+n.parent` keeps SQLite off the by_parent index (a 50k-row walk) so it filters
                        // by the body first; the NULL guard keeps `-has:child(…)` correct.
                        format!("n.id IN (SELECT n.parent FROM nodes n WHERE n.deleted=0 AND +n.parent IS NOT NULL AND ({body}))"),
                        Ast::term(&format!("has:child({})", ast_text(&ast)), format!("with a child that is: {means}"), format!("has child ({short})")),
                    )
                }))
            }
            Some(Tok::Atom(a)) => {
                self.pos += 1;
                if let Some(rest) = a.strip_prefix('-') {
                    if !rest.is_empty() && !rest.chars().next().unwrap().is_ascii_digit() {
                        return Ok(self.atom(rest)?.map(|(s, x)| (format!("NOT ({s})"), x.negate())));
                    }
                }
                self.atom(&a)
            }
            Some(other) => {
                self.pos += 1;
                Err(invalid(format!("unexpected {} in query", describe(&other))))
            }
            None => Err(invalid("query ends early")),
        }
    }

    fn param(&mut self, v: impl Into<SqlValue>) -> String {
        self.params.push(v.into());
        format!("?{}", self.params.len())
    }

    fn date(&self, raw: &str) -> Result<String> {
        Ok(dates::parse(raw.trim_matches('"'), self.today)?.date().format("%Y-%m-%d").to_string())
    }

    /// `¶ Q4 Planning`, `§ 2026-10-04`, or the first words of a block.
    /// A node named in a query: an id (or prefix), or a page's title, quoted when it has spaces
    /// (`under:"¶ Issues"`, `under:Plans`).
    fn node_ref(&self, value: &str) -> Result<String, anyhow::Error> {
        let quoted = value.starts_with('"');
        let title = value.trim_matches('"').trim_start_matches('¶').trim();
        if quoted || value.contains('¶') {
            if let Some(id) = self.store.find_root_by_title(title, false)? {
                return Ok(id);
            }
        }
        match self.store.resolve(title) {
            Ok(id) => Ok(id),
            Err(e) => match self.store.find_root_by_title(title, false)? {
                Some(id) => Ok(id),
                None => Err(e),
            },
        }
    }

    fn label(&self, id: &str) -> String {
        match self.store.node(id) {
            Ok(Some(n)) if n.journal.is_some() => format!("§ {}", n.label()),
            Ok(Some(n)) if n.title.is_some() => format!("¶ {}", n.label()),
            Ok(Some(n)) => {
                let l = n.label();
                let cut: String = l.chars().take(30).collect();
                format!("\"{cut}{}\"", if cut.len() < l.len() { "…" } else { "" })
            }
            _ => id.to_string(),
        }
    }

    fn atom(&mut self, a: &str) -> Result<Option<(String, Ast)>> {
        let a = a.trim();
        if let Some(tag) = a.strip_prefix('#') {
            let p = self.param(tag.to_lowercase());
            return Ok(Some((
                // Settled per scope by `probe`: a correlated EXISTS or an uncorrelated IN.
                format!("{TAG_PROBE}{p}{TAG_PROBE}"),
                Ast::term(a, format!("tagged #{tag}"), format!("#{tag}")),
            )));
        }
        if let Some(by) = a.strip_prefix("group:") {
            let by = by.to_lowercase();
            if !crate::group::BY.contains(&by.as_str()) {
                return Err(unknown("group", &by, crate::group::BY));
            }
            self.group = Some(by);
            return Ok(None);
        }
        if let Some(rest) = a.strip_prefix("sort:") {
            let (field, desc) = match rest.strip_suffix('-') {
                Some(f) => (f, true),
                None => (rest, false),
            };
            let col = match field {
                "due" | "scheduled" | "done_at" | "title" | "priority" | "status" => format!("n.{field}"),
                "created" => "n.created_ms".into(),
                "updated" => "n.updated_ms".into(),
                "date" => "coalesce(n.scheduled, n.due)".into(),
                // Outline order (a queue): each note's (ord, id) from its root down, so a
                // parent comes before its children and siblings keep the order they're shown in.
                // \x02 parts the levels, \x01 a level's ord from its id: both sort below any ord.
                "order" => ORDER_KEY.into(),
                _ => {
                    let opts = ["due", "scheduled", "done_at", "title", "priority", "status", "created", "updated", "date", "order"];
                    return Err(unknown("sort", field, &opts));
                }
            };
            let col = if field == "priority" {
                "CASE n.priority WHEN 'high' THEN 0 WHEN 'med' THEN 1 WHEN 'low' THEN 2 ELSE 3 END".to_string()
            } else {
                col
            };
            self.order.push(format!("{col} IS NULL, {col} {}", if desc { "DESC" } else { "ASC" }));
            if field == "due" {
                self.order.push("n.scheduled IS NULL, n.scheduled ASC".into());
            }
            let (what, empty) = match field {
                "due" => ("due date", "undated last"),
                "scheduled" => ("scheduled date", "unscheduled last"),
                "done_at" => ("done date", "open last"),
                "date" => ("date (scheduled or due)", "undated last"),
                "priority" => ("priority", "none last"),
                "status" => ("status", "non-tasks last"),
                "created" => ("creation time", ""),
                "updated" => ("last change", ""),
                "order" => ("outline order", ""),
                _ => ("title", "untitled last"),
            };
            let dir = match (field, desc) {
                ("priority", false) => "high first",
                ("priority", true) => "low first",
                ("title" | "status" | "order", false) => "",
                ("title" | "status", true) => "Z to A",
                ("order", true) => "last first",
                (_, false) => "oldest first",
                (_, true) => "newest first",
            };
            let dir = if matches!(field, "due" | "scheduled" | "date") && !desc { "" } else { dir };
            let tail: Vec<&str> = [dir, empty].into_iter().filter(|s| !s.is_empty()).collect();
            self.sorts.push(format!("sorted by {what}{}", if tail.is_empty() { String::new() } else { format!(", {}", tail.join(", ")) }));
            return Ok(None);
        }
        // `!high`, as in capture syntax.
        if let Some(pr) = a.strip_prefix('!').and_then(crate::capture::normalize_priority) {
            let p = self.param(pr.to_string());
            return Ok(Some((format!("n.priority = {p}"), Ast::term(a, format!("priority {pr}"), format!("!{pr}")))));
        }
        // field<op>value
        for op in ["<=", ">=", "!=", "<", ">", "=", ":"] {
            if let Some(i) = a.find(op) {
                let (field, value) = (&a[..i], &a[i + op.len()..]);
                if field.is_empty() || value.is_empty() {
                    continue;
                }
                return self.field(&field.to_lowercase(), op, value).map(|(s, m, sh)| Some((s, Ast::term(a, m, sh))));
            }
        }
        // bare word: substring match in title/text
        let w = a.trim_matches('"');
        let p = self.param(format!("%{w}%"));
        Ok(Some((crate::ocr::text_sql(&p), Ast::term(a, format!("text contains \"{w}\""), format!("\"{w}\"")))))
    }

    /// (sql, means, short)
    fn field(&mut self, field: &str, op: &str, value: &str) -> Result<(String, String, String)> {
        let sql_op = match op {
            ":" | "=" => "=",
            o => o,
        };
        let value_l = value.to_lowercase();
        match field {
            "status" => {
                let (sql, m) = match value_l.as_str() {
                    "open" => {
                        self.selective = true;
                        ("n.status IN ('todo','doing','waiting')".to_string(), "open tasks".to_string())
                    }
                    "closed" => ("n.status IN ('done','cancelled')".into(), "closed tasks (done or cancelled)".into()),
                    "any" => ("n.status IS NOT NULL".into(), "tasks".into()),
                    "none" => ("n.status IS NULL".into(), "not tasks".into()),
                    s if crate::capture::STATUSES.contains(&s) => {
                        let open = sql_op == "=" && matches!(s, "todo" | "doing" | "waiting");
                        self.selective |= open;
                        let p = self.param(s.to_string());
                        // An open status also names the open-task partial index's condition.
                        let guard = if open { "n.status IN ('todo','doing','waiting') AND " } else { "" };
                        (format!("({guard}n.status {sql_op} {p})"), format!("status {}{s}", if sql_op == "!=" { "not " } else { "" }))
                    }
                    s => {
                        let opts = ["open", "closed", "any", "none", "todo", "doing", "waiting", "done", "cancelled"];
                        return Err(unknown("status", s, &opts));
                    }
                };
                let sh = m.trim_end_matches(" (done or cancelled)").to_string();
                Ok((sql, m, sh))
            }
            "is" => {
                let today = self.today.format("%Y-%m-%d").to_string();
                let (sql, m): (String, String) = match value_l.as_str() {
                    "unread" => {
                        let recipient = self.recipient.clone();
                        (recipient.sql(true, &mut |s| self.param(s))?, "unread messages addressed to you or your role".into())
                    }
                    "task" => ("n.status IS NOT NULL".into(), "tasks".into()),
                    "page" => ("(n.parent IS NULL AND n.title IS NOT NULL AND n.journal IS NULL)".into(), "pages".into()),
                    "journal" => ("n.journal IS NOT NULL".into(), "journal days".into()),
                    "inbox" => ("(n.parent IS NULL AND n.title IS NULL AND n.journal IS NULL)".into(), "in the inbox".into()),
                    "event" => ("(n.status IS NULL AND n.scheduled LIKE '%T%')".into(), "events (a time, no task status)".into()),
                    "repeating" => ("n.repeat IS NOT NULL".into(), "repeating".into()),
                    // Attachment nodes (FORMAT.md "Attachments"): images, other files, either.
                    "image" | "file" | "attachment" => {
                        self.attachments = true;
                        let kind = match value_l.as_str() {
                            "image" => " AND id IN (SELECT node FROM props WHERE key='kind' AND value='\"image\"')",
                            "file" => " AND id IN (SELECT node FROM props WHERE key='kind' AND value='\"file\"')",
                            _ => "",
                        };
                        (
                            format!("n.id IN (SELECT node FROM props WHERE key='system' AND value='\"attachment\"'{})", kind.replace("AND id IN", "AND node IN")),
                            match value_l.as_str() {
                                "image" => "attached images".into(),
                                "file" => "attached files (not images)".into(),
                                _ => "attachments".into(),
                            },
                        )
                    }
                    "overdue" => {
                        self.selective = true;
                        let p = self.param(today.clone());
                        (format!("(n.status IN ('todo','doing','waiting') AND substr(n.due,1,10) < {p})"), format!("overdue (open, due before {})", day(&today, self.today)))
                    }
                    "conflict" => ("n.id IN (SELECT node FROM conflicts WHERE resolved=0)".into(), "with an open conflict".into()),
                    "alert" => ("n.id IN (SELECT node FROM alerts WHERE deleted=0)".into(), "with an alert".into()),
                    // To review (issues.md §5): done by an agent, and that change not yet accepted
                    // in the review lane. Derived, not a status.
                    "to-review" | "to_review" | "toreview" => (
                        "(n.status='done' AND EXISTS (SELECT 1 FROM clocks c JOIN events ev ON ev.okey=c.okey \
                         WHERE c.entity=n.id AND c.field='status' AND ev.actor NOT LIKE 'human%' \
                         AND NOT EXISTS (SELECT 1 FROM reviews r WHERE r.tx=ev.tx AND r.verdict='accepted')))"
                            .into(),
                        "done by an agent, not yet reviewed".into(),
                    ),
                    // Probe this node's edges before looking up a blocker by ID. Stale
                    // statistics can otherwise put every open task inside the correlated loop.
                    // CROSS JOIN fixes the loop order; the primary-key index fixes its lookup.
                    "blocked" => (
                        "EXISTS (SELECT 1 FROM edges e CROSS JOIN nodes b INDEXED BY sqlite_autoindex_nodes_1 ON b.id=e.src WHERE e.dst=n.id AND e.rel='blocks' \
                         AND b.deleted=0 AND b.status IN ('todo','doing','waiting'))"
                            .into(),
                        "blocked by an open task".into(),
                    ),
                    // Ready: an open task you can start now (not waiting, scheduled reached, unblocked).
                    "ready" => {
                        self.selective = true;
                        let p = self.param(today.clone());
                        (
                            // The redundant open-status term lets SQLite use the open_by_due partial
                            // index (it can't infer it from IN ('todo','doing')): 13 ms → 0.5 at 50k.
                            format!(
                                "(n.status IN ('todo','doing','waiting') AND n.status IN ('todo','doing') AND (n.scheduled IS NULL OR substr(n.scheduled,1,10) <= {p}) \
                                 AND NOT EXISTS (SELECT 1 FROM edges e CROSS JOIN nodes b INDEXED BY sqlite_autoindex_nodes_1 ON b.id=e.src WHERE e.dst=n.id AND e.rel='blocks' \
                                 AND b.deleted=0 AND b.status IN ('todo','doing','waiting')))"
                            ),
                            format!("ready (todo or doing, scheduled by {} or unscheduled, nothing open blocking)", day(&today, self.today)),
                        )
                    }
                    "blocking" => (
                        "EXISTS (SELECT 1 FROM edges e CROSS JOIN nodes b INDEXED BY sqlite_autoindex_nodes_1 ON b.id=e.dst WHERE e.src=n.id AND e.rel='blocks' \
                         AND b.deleted=0 AND b.status IN ('todo','doing','waiting'))"
                            .into(),
                        "blocking an open task".into(),
                    ),
                    k => {
                        let opts = ["task", "page", "journal", "inbox", "event", "repeating", "overdue", "conflict", "alert", "blocked", "blocking", "ready", "to-review", "unread", "image", "file", "attachment"];
                        return Err(unknown("is", k, &opts));
                    }
                };
                let sh = m.split(" (").next().unwrap_or(&m).to_string();
                Ok((sql, m, sh))
            }
            "has" => match value_l.as_str() {
                "child" | "children" => Ok((
                    "n.id IN (SELECT parent FROM nodes WHERE deleted=0 AND parent IS NOT NULL)".into(),
                    "with children".into(),
                    "has children".into(),
                )),
                v => Err(unknown("has", v, &["child"])),
            },
            // Any date: scheduled, due, or an alert's fire time (Obsidian Tasks' `happens`).
            "happens" => {
                if value_l == "none" || value_l == "any" {
                    let any = "(n.scheduled IS NOT NULL OR n.due IS NOT NULL OR n.id IN (SELECT node FROM alerts WHERE deleted=0))";
                    return Ok(if value_l == "any" {
                        (any.into(), "has a date (scheduled, due or alert)".into(), "has a date".into())
                    } else {
                        (format!("NOT {any}"), "no date at all".into(), "no date".into())
                    });
                }
                let d = self.date(value)?;
                let shown = day(&d, self.today);
                let next = next_day(&d);
                let p = self.param(d);
                // Only bind the day after when the range uses it (SQLite rejects unused params).
                let q = if needs_next(sql_op) { self.param(next) } else { String::new() };
                let c = |col: &str| format!("({col} IS NOT NULL AND {})", date_range(&format!("{DATE_COL}{col}"), sql_op, &p, &q));
                Ok((
                    format!("({} OR {} OR n.id IN (SELECT node FROM alerts WHERE deleted=0 AND {}))", c("n.scheduled"), c("n.due"), c("fire_at")),
                    format!("happens {} {shown} (scheduled, due or an alert)", date_words(sql_op)),
                    match sql_op {
                        "<=" => format!("happens by {shown}"),
                        o => format!("happens {} {shown}", date_words(o)),
                    },
                ))
            }
            "under" | "in" => {
                let id = self.node_ref(value)?;
                let label = self.label(&id);
                let p = self.param(id);
                Ok((
                    format!("n.id IN (WITH RECURSIVE d(id) AS (SELECT id FROM nodes WHERE parent={p} UNION SELECT x.id FROM d CROSS JOIN nodes x INDEXED BY by_parent ON x.parent=d.id) SELECT id FROM d)"),
                    format!("anywhere under {label}"),
                    format!("in {label}"),
                ))
            }
            "parent" => {
                let id = self.node_ref(value)?;
                let label = self.label(&id);
                let p = self.param(id);
                Ok((format!("n.parent = {p}"), format!("directly under {label}"), format!("in {label}")))
            }
            // The notes that show an attachment (FORMAT.md "Attachments").
            "embeds" => {
                let id = self.store.resolve(value)?;
                let label = self.label(&id);
                let p = self.param(id);
                Ok((format!("EXISTS (SELECT 1 FROM edges e WHERE e.src=n.id AND e.rel='embed' AND e.dst={p})"), format!("showing {label}"), format!("embeds {label}")))
            }
            "blocks" | "blocked-by" | "blocked_by" | "rel" => {
                if field == "rel" {
                    let p = self.param(value_l.clone());
                    return Ok((format!("EXISTS (SELECT 1 FROM edges e WHERE e.src=n.id AND e.rel={p})"), format!("with a \"{value_l}\" link"), format!("rel:{value_l}")));
                }
                let id = self.store.resolve(value)?;
                let label = self.label(&id);
                let p = self.param(id);
                Ok(if field == "blocks" {
                    (format!("EXISTS (SELECT 1 FROM edges e WHERE e.src=n.id AND e.rel='blocks' AND e.dst={p})"), format!("blocking {label}"), format!("blocks {label}"))
                } else {
                    (format!("EXISTS (SELECT 1 FROM edges e WHERE e.dst=n.id AND e.rel='blocks' AND e.src={p})"), format!("blocked by {label}"), format!("blocked by {label}"))
                })
            }
            "to" => {
                let recipient = crate::messages::Recipient::target(value.trim_matches('"'))?;
                let sql = recipient.sql(false, &mut |s| self.param(s))?;
                Ok((sql, format!("messages addressed to {value}"), format!("to:{value}")))
            }
            "by" | "actor" => {
                let v = value.trim_matches('"').to_lowercase();
                let human = v == "human" || v == "me";
                let p = self.param(if human { "human".to_string() } else { format!("%{v}%") });
                let who = if human { "a person".to_string() } else { v.clone() };
                Ok((format!("n.created_by LIKE {p}"), format!("created by {who}"), format!("by {}", if human { "human" } else { &v })))
            }
            "text" => {
                let w = value.trim_matches('"');
                let p = self.param(format!("%{w}%"));
                Ok((crate::ocr::text_sql(&p), format!("text contains \"{w}\""), format!("\"{w}\"")))
            }
            "title" => {
                let w = value.trim_matches('"');
                let p = self.param(w.to_string());
                let m = format!("titled {}\"{w}\"", op_words(sql_op).to_string() + if sql_op == "=" { "" } else { " " });
                Ok((format!("n.title {sql_op} {p} COLLATE NOCASE"), m.clone(), m))
            }
            "priority" | "prio" => {
                let pr = crate::capture::normalize_priority(value).unwrap_or("none").to_string();
                let p = self.param(pr.clone());
                let m = match sql_op {
                    "=" => format!("priority {pr}"),
                    "!=" => format!("priority not {pr}"),
                    // Stored as text, so < and > compare the words; say what was typed.
                    o => format!("priority {o} {pr}"),
                };
                let sh = if sql_op == "=" { format!("!{pr}") } else { m.clone() };
                Ok((format!("n.priority {sql_op} {p}"), m, sh))
            }
            "due" | "scheduled" | "sched" | "done_at" | "done" => {
                let col = match field {
                    "sched" => "scheduled",
                    "done" => "done_at",
                    f => f,
                };
                let noun = match col {
                    "due" => "due",
                    "scheduled" => "scheduled",
                    _ => "done",
                };
                if value_l == "none" {
                    let m = match col {
                        "due" => "no due date".to_string(),
                        "scheduled" => "not scheduled".into(),
                        _ => "not done".into(),
                    };
                    return Ok((format!("n.{col} IS NULL"), m.clone(), m));
                }
                if value_l == "any" {
                    let m = match col {
                        "due" => "has a due date".to_string(),
                        "scheduled" => "scheduled".into(),
                        _ => "done".into(),
                    };
                    return Ok((format!("n.{col} IS NOT NULL"), m.clone(), m));
                }
                let d = self.date(value)?;
                let shown = day(&d, self.today);
                let next = next_day(&d);
                let p = self.param(d);
                let q = if needs_next(sql_op) { self.param(next) } else { String::new() };
                let sh = match sql_op {
                    "<=" => format!("{noun} by {shown}"),
                    "=" => format!("{noun} {shown}"),
                    o => format!("{noun} {} {shown}", date_words(o)),
                };
                // No `IS NOT NULL`: a range is never true for NULL, and the extra term would make
                // the all-dates partial index eligible where the open-task one is better.
                Ok((format!("({})", date_range(&format!("{DATE_COL}n.{col}"), sql_op, &p, &q)), format!("{noun} {} {shown}", date_words(sql_op)), sh))
            }
            "created" | "updated" => {
                let d = self.date(value)?;
                let shown = day(&d, self.today);
                let p = self.param(d);
                let col = if field == "created" { "created_ms" } else { "updated_ms" };
                let noun = if field == "created" { "created" } else { "changed" };
                Ok((format!("date(n.{col}/1000, 'unixepoch', 'localtime') {sql_op} {p}"), format!("{noun} {} {shown}", date_words(sql_op)), format!("{noun} {} {shown}", date_words(sql_op))))
            }
            "journal" => {
                let d = self.date(value)?;
                let shown = day(&d, self.today);
                let p = self.param(d);
                let m = if sql_op == "=" { format!("in the journal for {shown}") } else { format!("in a journal day {} {shown}", date_words(sql_op)) };
                Ok((format!("n.parent IN (SELECT id FROM nodes WHERE journal {sql_op} {p})"), m.clone(), m))
            }
            key => {
                if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                    return Err(invalid(format!("bad field {key:?}")));
                }
                let kp = self.param(key.to_string());
                let ty = self.store.prop_type(key)?.unwrap_or_else(|| "text".into());
                let raw = value.trim_matches('"').to_string();
                let (expr, v, shown): (&str, SqlValue, String) = match ty.as_str() {
                    "number" => ("CAST(p.value AS REAL)", SqlValue::Real(value.parse().map_err(|_| invalid(format!("{key} is numeric")))?), raw.clone()),
                    "date" => {
                        let d = self.date(value)?;
                        let shown = day(&d, self.today);
                        ("substr(json_extract(p.value,'$'),1,10)", SqlValue::Text(d), shown)
                    }
                    "bool" => ("json_extract(p.value,'$')", SqlValue::Integer((value_l == "true") as i64), value_l.clone()),
                    _ => ("json_extract(p.value,'$')", SqlValue::Text(raw.clone()), format!("\"{raw}\"")),
                };
                let vp = self.param(v);
                let cmp = if ty == "text" && sql_op == "=" { format!("{expr} = {vp} COLLATE NOCASE") } else { format!("{expr} {sql_op} {vp}") };
                let words = if ty == "date" { date_words(sql_op) } else if sql_op == "=" { "is" } else { op_words(sql_op) };
                let m = format!("{key} {words} {shown}");
                Ok((format!("n.id IN (SELECT p.node FROM props p WHERE p.key={kp} AND {cmp})"), m, format!("{key}{op}{raw}")))
            }
        }
    }
}

/// `unknown status "opn" · did you mean open?` (validation error, exit code 6).
pub fn unknown_value(what: &str, got: &str, options: &[&str]) -> anyhow::Error {
    unknown(what, got, options)
}

fn unknown(what: &str, got: &str, options: &[&str]) -> anyhow::Error {
    match closest(got, options) {
        Some(s) => invalid(format!("unknown {what} \"{got}\" · did you mean {s}?")),
        None => invalid(format!("unknown {what} \"{got}\" · use one of {}", options.join(", "))),
    }
}

/// Levenshtein distance.
pub fn distance(a: &str, b: &str) -> usize {
    dist(a, b)
}

/// Closest option by edit distance (at most 2 edits, or a shared prefix).
pub fn closest<'a>(got: &str, options: &[&'a str]) -> Option<&'a str> {
    options
        .iter()
        .map(|o| (dist(got, o), *o))
        .filter(|(d, o)| *d <= 2 || (got.len() >= 2 && o.starts_with(got)))
        .min_by_key(|(d, _)| *d)
        .map(|(_, o)| o)
}

fn dist(a: &str, b: &str) -> usize {
    {
        let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for i in 1..=a.len() {
            let mut cur = vec![i; b.len() + 1];
            for j in 1..=b.len() {
                let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
                cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            }
            prev = cur;
        }
        prev[b.len()]
    }
}

pub(crate) fn fts_terms(terms: &str) -> String {
    terms.split_whitespace().map(|w| format!("\"{}\"*", w.replace('"', ""))).collect::<Vec<_>>().join(" ")
}

impl Store {
    pub fn query(&self, q: &str, today: NaiveDate, limit: usize) -> Result<Vec<crate::model::Node>> {
        let c = compile(q, self, today)?;
        let sql = format!("{} {} LIMIT {}", c.where_sql, c.order_sql, limit);
        let params: Vec<&dyn rusqlite::ToSql> = c.params.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
        self.nodes_where(&sql, &params)
    }

    /// Full-text search (FTS5), ranked.
    pub fn search(&self, terms: &str, limit: usize) -> Result<Vec<crate::model::Node>> {
        let fts = fts_terms(terms);
        if fts.is_empty() {
            return Ok(vec![]);
        }
        let sql = format!(
            "SELECT {} FROM nodes_fts f JOIN nodes n ON n.id = f.id WHERE nodes_fts MATCH ?1 AND n.deleted = 0 AND {} ORDER BY f.rank LIMIT {limit}",
            crate::model::NODE_COLS,
            crate::views::HIDDEN_SQL
        );
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map([&fts], crate::model::Node::from_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_parens_and_ops() {
        let t: Vec<Tok> = lex("status:open (due<=+3d or priority:high) -#someday").into_iter().map(|t| t.0).collect();
        assert_eq!(t.len(), 7);
        assert_eq!(t[1], Tok::LParen);
        assert_eq!(t[3], Tok::Or);
    }

    #[test]
    fn compiles_against_empty_store() {
        let s = Store::open_memory().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
        let c = compile("status:open (due<=+3d or priority:high) #work -#someday sort:due", &s, today).unwrap();
        assert!(c.where_sql.contains(" OR "), "{}", c.where_sql);
        assert!(c.where_sql.contains("NOT ("), "{}", c.where_sql);
        assert!(c.order_sql.contains("n.due"), "{}", c.order_sql);
        // open, high, work, someday, plus the due range's day and next day.
        assert_eq!(c.params.len(), 5);
        // Runs.
        s.query("status:open (due<=+3d or priority:high) #work -#someday sort:due", today, 10).unwrap();
        s.query("client=acme is:task", today, 10).unwrap();
        s.query("by:claude", today, 10).unwrap();
        // Every comparison binds exactly the parameters its SQL uses (the export's `due<today`).
        for op in ["<", "<=", ">", ">=", "=", "!=", ":"] {
            for f in ["due", "sched", "done", "happens", "created"] {
                s.query(&format!("{f}{op}today"), today, 10).unwrap_or_else(|e| panic!("{f}{op}today: {e}"));
            }
        }
        s.query("status:open (due<today or sched<today) sort:date", today, 10).unwrap();
    }

    #[test]
    fn unknown_values_suggest() {
        let s = Store::open_memory().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
        let e = s.query("status:opn", today, 10).unwrap_err().to_string();
        assert!(e.contains("unknown status \"opn\" · did you mean open?"), "{e}");
        assert!(s.query("is:tsk", today, 10).unwrap_err().to_string().contains("did you mean task?"));
        assert!(s.query("sort:dew", today, 10).unwrap_err().to_string().contains("did you mean due?"));
    }

    #[test]
    fn explains_in_plain_words() {
        let s = Store::open_memory().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
        let e = explain("status:open (due<=+3d or !high) #work -#someday sort:due", &s, today).unwrap();
        assert_eq!(
            e.lines(),
            vec![
                "open tasks",
                "and (due on or before Tue Oct 6, or priority high)",
                "and tagged #work",
                "and not tagged #someday",
                "sorted by due date, undated last",
            ]
        );
        assert_eq!(e.short(), "open tasks · due by Tue Oct 6 or !high · #work · not #someday · by due date");
        assert!(e.sql().contains("n.due < '2026-10-07'"), "on or before the 6th is before the 7th: {}", e.sql());
        // Top-level OR.
        let e = explain("#a or #b", &s, today).unwrap();
        assert_eq!(e.lines(), vec!["tagged #a", "or tagged #b"]);
        let e = explain("(#a #b) #c", &s, today).unwrap();
        assert_eq!(e.lines(), vec!["tagged #a", "and tagged #b", "and tagged #c"]);
        // Carets point at the bad token.
        let q = "status:opn #work";
        let err = explain(q, &s, today).unwrap_err();
        assert_eq!(err.span.map(|(a, b)| &q[a..b]), Some("status:opn"));
        let e = explain("status:open group:parent", &s, today).unwrap();
        assert_eq!(e.group.as_deref(), Some("parent"));
        assert_eq!(e.lines().last().unwrap(), "grouped by parent");
        assert!(explain("group:tags", &s, today).unwrap_err().error.to_string().contains("did you mean tag?"));
        let q = "(#a or #b";
        assert!(explain(q, &s, today).unwrap_err().error.to_string().contains("missing )"));
    }
}
