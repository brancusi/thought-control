//! Human and JSON rendering.

use chrono::NaiveDate;
use serde_json::{Value, json};
use std::io::{IsTerminal, Write};
use thc_core::dates::{self, DateVal};
use thc_core::model::Node;
use thc_core::store::Store;

pub struct Out {
    pub json: bool,
    /// `--fields a,b,c`: node objects keep only these keys.
    pub fields: Option<Vec<String>>,
    pub color: bool,
    pub today: NaiveDate,
    buf: std::io::BufWriter<std::io::StdoutLock<'static>>,
}

impl Out {
    pub fn new(json: bool) -> Out {
        let stdout = std::io::stdout();
        let color = !json && stdout.is_terminal() && std::env::var_os("NO_COLOR").is_none();
        Out { json, fields: None, color, today: dates::today(), buf: std::io::BufWriter::new(stdout.lock()) }
    }

    pub fn line(&mut self, s: impl AsRef<str>) {
        let _ = writeln!(self.buf, "{}", s.as_ref());
    }

    pub fn json(&mut self, v: &Value) {
        // Listings say which vault they read (vaults.md §10.2): `"vault":{"name","source"}`.
        let with_vault;
        let v = match (VAULT_JSON.get(), v.as_object()) {
            (Some(vj), Some(o)) if o.contains_key("items") && !o.contains_key("vault") => {
                let mut o = o.clone();
                o.insert("vault".into(), vj.clone());
                with_vault = Value::Object(o);
                &with_vault
            }
            _ => v,
        };
        match &self.fields {
            Some(f) => {
                let mut v = v.clone();
                crate::schema::filter_fields(&mut v, f);
                let _ = writeln!(self.buf, "{}", serde_json::to_string(&v).unwrap_or_default());
            }
            None => {
                let _ = writeln!(self.buf, "{}", serde_json::to_string(v).unwrap_or_default());
            }
        }
    }

    pub fn flush(&mut self) {
        let _ = self.buf.flush();
    }

    fn paint(&self, code: &str, s: &str) -> String {
        if self.color { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
    }
    pub fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
    pub fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }
    pub fn red(&self, s: &str) -> String {
        self.paint("31", s)
    }
    pub fn yellow(&self, s: &str) -> String {
        self.paint("33", s)
    }
    pub fn green(&self, s: &str) -> String {
        self.paint("32", s)
    }
    pub fn cyan(&self, s: &str) -> String {
        self.paint("36", s)
    }
    pub fn magenta(&self, s: &str) -> String {
        self.paint("35", s)
    }
    pub fn agent(&self, s: &str) -> String {
        self.paint("94", s)
    }

    pub fn heading(&mut self, s: &str) {
        if !self.json {
            let h = self.bold(s);
            self.line(h);
        }
    }

    fn date_label(&self, raw: &str, is_due: bool, open: bool) -> String {
        let Some(dv) = DateVal::from_stored(raw) else { return raw.to_string() };
        let d = dv.date();
        let rel = dates::relative(d, self.today);
        let time = dv.time().map(|t| format!(" {}", t.format("%H:%M"))).unwrap_or_default();
        let s = if (0..7).contains(&(d - self.today).num_days()) {
            format!("{rel}{time}")
        } else {
            format!("{}{time} ({rel})", d.format("%b %-d"))
        };
        if open && d < self.today && is_due {
            self.red(&format!("due {s}"))
        } else if open && d == self.today {
            self.yellow(&if is_due { format!("due {s}") } else { s })
        } else if is_due {
            format!("due {s}")
        } else {
            s
        }
    }

    pub fn checkbox(&self, n: &Node) -> String {
        match n.status.as_deref() {
            Some("todo") => "[ ] ".into(),
            Some("doing") => self.cyan("[/] "),
            Some("waiting") => self.magenta("[w] "),
            Some("done") => self.green("[x] "),
            Some("cancelled") => self.dim("[-] "),
            _ => String::new(),
        }
    }

    /// One-line summary: `k3f9a  [ ] Call dentist #health   due fri · !high · ↻ every week`
    pub fn node_line(&self, store: &Store, n: &Node, indent: usize, context: bool) -> String {
        self.node_line_marked(store, n, indent, context, None)
    }

    /// Same, with a marker right after the text (e.g. `◎ fired 15:43`).
    pub fn node_line_marked(&self, store: &Store, n: &Node, indent: usize, context: bool, marker: Option<String>) -> String {
        let short = store.short(&n.id);
        let text = if n.title.is_some() {
            self.bold(&n.label())
        } else if n.journal.is_some() {
            // `§ today`, `§ yesterday`, `§ Oct 3` (`§ Oct 3 2025` in another year), as elsewhere.
            let day = DateVal::from_stored(&n.label()).map(|d| d.date());
            let label = match day {
                Some(d) if (d - self.today).num_days().abs() <= 1 => dates::relative(d, self.today),
                Some(d) if chrono::Datelike::year(&d) == chrono::Datelike::year(&self.today) => d.format("%b %-d").to_string(),
                Some(d) => d.format("%b %-d %Y").to_string(),
                None => n.label(),
            };
            self.bold(&format!("§ {label}"))
        } else {
            let t = store.render_text(&n.text);
            let first = t.lines().next().unwrap_or("").to_string();
            let more = if t.lines().count() > 1 { self.dim(" …") } else { String::new() };
            if matches!(n.status.as_deref(), Some("done" | "cancelled")) { self.dim(&first) + &more } else { first + &more }
        };
        let mut meta: Vec<String> = Vec::new();
        if let Some(m) = marker {
            meta.push(m);
        }
        let open = n.is_open();
        if let Some(s) = &n.scheduled {
            meta.push(self.date_label(s, false, open));
        }
        if let Some(d) = &n.due {
            meta.push(self.date_label(d, true, open));
        }
        if let Some(p) = &n.priority {
            meta.push(match p.as_str() {
                "high" => self.red("!high"),
                "med" => self.yellow("!med"),
                _ => "!low".into(),
            });
        }
        if let Some(r) = n.repeat.as_ref().and_then(|r| r.get("text")).and_then(|t| t.as_str()) {
            meta.push(format!("↻ {r}"));
        }
        if let Some(a) = n.created_by.strip_prefix("agent:") {
            meta.push(self.agent(&format!("◆ {a}")));
        }
        if context {
            if let Some(p) = n.parent.as_deref().and_then(|p| store.node(p).ok().flatten()) {
                let label = match &p.journal {
                    Some(j) if *j == self.today.format("%Y-%m-%d").to_string() => "in today's journal".to_string(),
                    Some(j) => match DateVal::from_stored(j) {
                        Some(dv) => format!("in journal {}", dv.date().format("%b %-d")),
                        None => format!("in journal {j}"),
                    },
                    None => format!("in {}", p.label()),
                };
                meta.push(self.dim(&label));
            }
        }
        let meta = if meta.is_empty() { String::new() } else { format!("  {}", meta.join(&self.dim(" · "))) };
        let in_conflict = store
            .conn
            .query_row("SELECT 1 FROM conflicts WHERE node=?1 AND resolved=0", [&n.id], |_| Ok(()))
            .is_ok();
        let id_col = if in_conflict { format!("{} {}", self.dim(&short), self.magenta("≠")) } else { self.dim(&format!("{short:<6}")) };
        format!("{}{}  {}{}{}", "  ".repeat(indent), id_col, self.checkbox(n), text, meta)
    }
}

/// The vault this command reads, by name: on every node (`vault`), and `{name, source}` on
/// listings. Set once, after the vault resolves.
pub static VAULT_NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
pub static VAULT_JSON: std::sync::OnceLock<Value> = std::sync::OnceLock::new();

/// JSON object for a node, with rendered text, tags and custom props.
pub fn node_json(store: &Store, n: &Node) -> Value {
    let mut v = json!({
        "id": n.id,
        "short": store.short(&n.id),
        "rev": store.rev(&n.id),
        "kind": n.kind(),
        "parent": n.parent,
        "title": n.title,
        "text": store.render_text(&n.text),
        "status": n.status,
        "scheduled": n.scheduled,
        "due": n.due,
        "priority": n.priority,
        "repeat": n.repeat,
        "done_at": n.done_at,
        "journal": n.journal,
        "tags": store.tags_of(&n.id).unwrap_or_default(),
        "created_by": n.created_by,
        "created": ms_to_local(n.created_ms),
        "updated": ms_to_local(n.updated_ms),
    });
    if let Ok(p) = store.props_of(&n.id) {
        if !p.is_empty() {
            v["props"] = Value::Object(p);
        }
    }
    if let Some(name) = VAULT_NAME.get() {
        v["vault"] = json!(name);
    }
    if n.deleted {
        v["deleted"] = json!(true);
    }
    // Drop nulls to keep agent output compact.
    if let Value::Object(m) = &mut v {
        m.retain(|_, val| !val.is_null());
    }
    v
}

pub fn ms_to_local(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%dT%H:%M").to_string())
        .unwrap_or_default()
}
