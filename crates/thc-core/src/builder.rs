//! Turns user/agent intents into validated ops for one transaction. All IDs, order keys,
//! recurrence math and link resolution happen here, so replay stays a pure function of events.

use crate::capture::{self, Capture, STATUSES, normalize_priority};
use crate::dates::{self, DateVal};
use crate::error::{invalid, not_found};
use crate::event::{NextDates, Op, Trigger};
use crate::id::{self, ID_LEN};
use crate::ord::key_between;
use crate::recur::{Mode, Repeat};
use crate::store::Store;
use anyhow::Result;
use chrono::NaiveDate;
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub struct TxBuilder<'a> {
    pub store: &'a Store,
    pub ops: Vec<Op>,
    pub today: NaiveDate,
    /// Nodes created in this transaction, in order.
    pub created: Vec<String>,
    /// Stub pages created to satisfy `[[Title]]` links.
    pub stubs: Vec<String>,
    /// `--plain`: notes created read no `#tags` from their text (it's kept exactly as written).
    pub plain: bool,
    last_ord: HashMap<Option<String>, String>,
    tags: HashMap<String, String>,
    pages: HashMap<String, String>,
    journals: HashMap<String, String>,
    defined: HashMap<String, String>,
    pending: HashMap<String, Map<String, Value>>,
}

impl<'a> TxBuilder<'a> {
    pub fn new(store: &'a Store, today: NaiveDate) -> TxBuilder<'a> {
        TxBuilder {
            store,
            ops: vec![],
            today,
            created: vec![],
            stubs: vec![],
            plain: false,
            last_ord: HashMap::new(),
            tags: HashMap::new(),
            pages: HashMap::new(),
            journals: HashMap::new(),
            defined: HashMap::new(),
            pending: HashMap::new(),
        }
    }

    pub fn finish(self) -> Vec<Op> {
        self.ops
    }

    fn exists(&self, id: &str) -> Result<bool> {
        Ok(self.created.iter().any(|c| c == id) || self.store.node_exists(id)?)
    }

    pub fn next_ord(&mut self, parent: Option<&str>) -> Result<String> {
        let key = parent.map(str::to_string);
        let last = match self.last_ord.get(&key) {
            Some(k) => Some(k.clone()),
            None => self.store.last_child_ord(parent)?,
        };
        let ord = key_between(last.as_deref(), None);
        self.last_ord.insert(key, ord.clone());
        Ok(ord)
    }

    // ---- well-known roots -------------------------------------------------------------------

    /// The id for a node every device would create the same way (a journal day, a tag, a page
    /// by title): `id::from_key`, so two devices that both create it offline make one node
    /// (replay keeps the first create of an id). If that id is already taken (a page renamed
    /// away from this title keeps it, or the node was deleted), a fresh random id instead.
    fn keyed_id(&self, key: &str) -> Result<String> {
        let id = id::from_key(key);
        Ok(if self.store.node(&id)?.is_some() || self.created.contains(&id) { id::new_id() } else { id })
    }

    pub fn tag(&mut self, name: &str) -> Result<String> {
        let name = name.trim_start_matches('#').to_lowercase();
        if let Some(id) = self.tags.get(&name) {
            return Ok(id.clone());
        }
        let id = match self.store.find_root_by_title(&name, true)? {
            Some(id) => id,
            None => {
                let id = self.keyed_id(&format!("tag:{name}"))?;
                let ord = self.next_ord(None)?;
                let mut props = Map::new();
                props.insert("tag".into(), json!(true));
                self.ops.push(Op::NodeCreate {
                    id: id.clone(),
                    parent: None,
                    order: ord,
                    text: String::new(),
                    title: Some(name.clone()),
                    props,
                });
                self.created.push(id.clone());
                id
            }
        };
        self.tags.insert(name, id.clone());
        Ok(id)
    }

    /// Find a page by title, creating it if `create`.
    pub fn page(&mut self, title: &str, create: bool) -> Result<Option<String>> {
        let key = title.to_lowercase();
        if let Some(id) = self.pages.get(&key) {
            return Ok(Some(id.clone()));
        }
        if let Some(id) = self.store.find_root_by_title(title, false)? {
            self.pages.insert(key, id.clone());
            return Ok(Some(id));
        }
        if !create {
            return Ok(None);
        }
        let id = self.create_page(title, &[])?;
        self.stubs.push(id.clone());
        Ok(Some(id))
    }

    pub fn create_page(&mut self, title: &str, tags: &[String]) -> Result<String> {
        if title.trim().is_empty() {
            return Err(invalid("page title is empty"));
        }
        if self.store.find_root_by_title(title, false)?.is_some() || self.pages.contains_key(&title.to_lowercase()) {
            return Err(invalid(format!("a page titled {title:?} already exists")));
        }
        let id = self.keyed_id(&format!("page:{}", title.trim().to_lowercase()))?;
        let ord = self.next_ord(None)?;
        self.ops.push(Op::NodeCreate {
            id: id.clone(),
            parent: None,
            order: ord,
            text: String::new(),
            title: Some(title.trim().to_string()),
            props: Map::new(),
        });
        self.created.push(id.clone());
        self.pages.insert(title.to_lowercase(), id.clone());
        for t in tags {
            let tid = self.tag(t)?;
            self.ops.push(Op::EdgeAdd { src: id.clone(), rel: "tag".into(), dst: tid });
        }
        Ok(id)
    }

    pub fn journal(&mut self, date: NaiveDate) -> Result<String> {
        let key = date.format("%Y-%m-%d").to_string();
        if let Some(id) = self.journals.get(&key) {
            return Ok(id.clone());
        }
        let id = match self.store.journal_node(&key)? {
            Some(id) => id,
            None => {
                let id = self.keyed_id(&format!("journal:{key}"))?;
                let ord = self.next_ord(None)?;
                let mut props = Map::new();
                props.insert("journal".into(), json!(key));
                self.ops.push(Op::NodeCreate { id: id.clone(), parent: None, order: ord, text: String::new(), title: None, props });
                self.created.push(id.clone());
                id
            }
        };
        self.journals.insert(key, id.clone());
        Ok(id)
    }

    // ---- text ---------------------------------------------------------------------------------

    /// Rewrite `[[Title]]` to `[[id]]` (creating stub pages) and return the linked ids.
    pub fn canonicalize(&mut self, text: &str) -> Result<(String, Vec<String>)> {
        // `--plain`: the text as written, so `[[…]]` is text too (no link, no stub page).
        if self.plain {
            return Ok((text.to_string(), vec![]));
        }
        let mut out = String::with_capacity(text.len());
        let mut links = Vec::new();
        let mut rest = text;
        // Quoted text is never parsed (writing.md §1): a quoted `[[…]]` is text, no link, no stub.
        let quoted = capture::quoted_ranges(text);
        while let Some(start) = rest.find("[[") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find("]]") else {
                out.push_str(&rest[start..]);
                rest = "";
                break;
            };
            let at = text.len() - rest.len() + start;
            if quoted.iter().any(|(a, b)| at > *a && at < *b) {
                out.push_str(&rest[start..start + 2 + end + 2]);
                rest = &after[end + 2..];
                continue;
            }
            let inner = after[..end].trim();
            let (target, label) = match inner.split_once('|') {
                Some((t, l)) => (t.trim(), Some(l.trim())),
                None => (inner, None),
            };
            // `[[vault:…]]` is reserved for links between vaults (FORMAT.md): kept as written.
            if target.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("vault:")) {
                out.push_str(&rest[start..start + 2 + end + 2]);
                rest = &after[end + 2..];
                continue;
            }
            let norm = id::normalize(target);
            let resolved = if norm.len() == ID_LEN && id::looks_like_id(&norm) && self.exists(&norm)? {
                norm
            } else {
                self.page(target, true)?.expect("created")
            };
            if !links.contains(&resolved) {
                links.push(resolved.clone());
            }
            out.push_str("[[");
            out.push_str(&resolved);
            if let Some(l) = label {
                out.push('|');
                out.push_str(l);
            }
            out.push_str("]]");
            rest = &after[end + 2..];
        }
        out.push_str(rest);
        Ok((out, links))
    }

    /// Emit edge diffs so `tag`/`mention` edges match `text` (for an existing node).
    /// Bring a node's tag and mention edges in line with its new text. Mentions come only from
    /// text. Tags can also be added on their own (`thc tag`, `todo -t`): a text edit removes only
    /// the tags the old text carried and the new one doesn't, never one added outside the text.
    /// The attachment node for a file in the vault (FORMAT.md "Attachments"): a
    /// root node keyed from the path, so the same file is the same node on every device, hidden
    /// from Pages by `system = "attachment"`. Its title is the first caption it's given (else the
    /// file name); `meta`, when the caller has the file, adds its size, dimensions and type.
    /// Made once: later calls return it (a deleted one comes back).
    pub fn attachment(&mut self, path: &str, caption: &str, meta: Option<&crate::attach::Stored>) -> Result<String> {
        let id = id::from_key(&format!("file:{path}"));
        if self.created.contains(&id) {
            return Ok(id);
        }
        if let Some(n) = self.store.node(&id)? {
            if n.deleted {
                self.ops.push(Op::NodeRestore { id: id.clone() });
            }
            return Ok(id);
        }
        let name = path.rsplit('/').next().unwrap_or(path);
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
        let mut props = Map::new();
        props.insert("system".into(), json!("attachment"));
        props.insert("kind".into(), json!(if crate::attach::is_image(path) { "image" } else { "file" }));
        props.insert("path".into(), json!(path));
        props.insert("mime".into(), json!(meta.map_or(crate::attach::mime(&ext), |m| m.mime)));
        if let Some(m) = meta {
            props.insert("bytes".into(), json!(m.bytes));
            if let (Some(w), Some(h)) = (m.w, m.h) {
                props.insert("w".into(), json!(w));
                props.insert("h".into(), json!(h));
            }
        }
        let title = if caption.trim().is_empty() { name.to_string() } else { caption.trim().to_string() };
        let order = self.next_ord(None)?;
        self.ops.push(Op::NodeCreate { id: id.clone(), parent: None, order, text: String::new(), title: Some(title), props });
        self.created.push(id.clone());
        Ok(id)
    }

    /// The attachments a note's text embeds: its `![caption](files/…)` references, as nodes.
    pub fn embeds_of(&mut self, text: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for (caption, path) in crate::attach::refs(text) {
            if path.starts_with("files/") {
                let id = self.attachment(&path, &caption, None)?;
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        Ok(out)
    }

    pub fn sync_edges(&mut self, id: &str, old_text_tags: &[String], tags: &[String], links: &[String]) -> Result<()> {
        let current = self.store.edges_from(id)?;
        let mut want: Vec<(String, String)> = Vec::new();
        // `embed`: the files its text shows, derived like mentions (FORMAT.md). --plain reads
        // nothing from the text, so it embeds nothing either.
        if !self.plain {
            let text = self.text_now(id)?;
            for e in self.embeds_of(&text)? {
                want.push(("embed".into(), e));
            }
        }
        for t in tags {
            want.push(("tag".into(), self.tag(t)?));
        }
        for l in links {
            if l != id {
                want.push(("mention".into(), l.clone()));
            }
        }
        let mut dropped_tags: Vec<String> = Vec::new();
        for t in old_text_tags.iter().filter(|t| !tags.contains(t)) {
            if let Some(tid) = self.store.find_root_by_title(t, true)? {
                dropped_tags.push(tid);
            }
        }
        for (rel, dst) in &current {
            let gone = match rel.as_str() {
                "mention" | "embed" => !want.contains(&(rel.clone(), dst.clone())),
                "tag" => dropped_tags.contains(dst),
                _ => false,
            };
            if gone {
                self.ops.push(Op::EdgeRemove { src: id.into(), rel: rel.clone(), dst: dst.clone() });
            }
        }
        for (rel, dst) in want {
            if !current.contains(&(rel.clone(), dst.clone())) {
                self.ops.push(Op::EdgeAdd { src: id.into(), rel, dst });
            }
        }
        Ok(())
    }

    // ---- create -------------------------------------------------------------------------------

    pub fn create_from_capture(&mut self, parent: Option<String>, cap: &Capture, id: Option<String>) -> Result<String> {
        let mut props = Map::new();
        if let Some(s) = &cap.status {
            props.insert("status".into(), json!(s));
            if s == "done" {
                props.insert("done_at".into(), json!(dates::now_stamp()));
            }
        }
        if let Some(d) = cap.scheduled {
            props.insert("scheduled".into(), json!(d.fmt()));
        }
        if let Some(d) = cap.due {
            props.insert("due".into(), json!(d.fmt()));
        }
        if let Some(p) = &cap.priority {
            props.insert("priority".into(), json!(p));
        }
        if let Some(r) = &cap.repeat {
            if cap.scheduled.is_none() && cap.due.is_none() {
                props.insert("scheduled".into(), json!(r.first_on_or_after(self.today)?.format("%Y-%m-%d").to_string()));
            }
            props.insert("repeat".into(), serde_json::to_value(r)?);
        }
        self.create(parent, &cap.text, None, props, &cap.tags, id)
    }

    pub fn create(
        &mut self,
        parent: Option<String>,
        text: &str,
        title: Option<String>,
        props: Map<String, Value>,
        tags: &[String],
        id: Option<String>,
    ) -> Result<String> {
        if let Some(p) = &parent {
            if !self.exists(p)? {
                return Err(not_found(format!("parent {p}")));
            }
        }
        let id = id.unwrap_or_else(id::new_id);
        if id.len() != ID_LEN || !id::looks_like_id(&id) {
            return Err(invalid(format!("--id must be a {ID_LEN}-char base32 id")));
        }
        let (text, links) = self.canonicalize(text)?;
        let ord = self.next_ord(parent.as_deref())?;
        self.ops.push(Op::NodeCreate { id: id.clone(), parent, order: ord, text, title, props: props.clone() });
        self.created.push(id.clone());
        self.pending.insert(id.clone(), props);
        let mut all_tags: Vec<String> = tags.to_vec();
        let from_text = if self.plain { vec![] } else { capture::extract_tags(&self.store_text_of_pending(&id)) };
        for t in from_text {
            if !all_tags.contains(&t) {
                all_tags.push(t);
            }
        }
        for t in all_tags {
            let tid = self.tag(&t)?;
            self.ops.push(Op::EdgeAdd { src: id.clone(), rel: "tag".into(), dst: tid });
        }
        for l in links {
            self.ops.push(Op::EdgeAdd { src: id.clone(), rel: "mention".into(), dst: l });
        }
        // The files its text shows (FORMAT.md "Attachments").
        if !self.plain {
            let text = self.store_text_of_pending(&id);
            for e in self.embeds_of(&text)? {
                self.ops.push(Op::EdgeAdd { src: id.clone(), rel: "embed".into(), dst: e });
            }
        }
        Ok(id)
    }

    /// Override the order key of a node created earlier in this transaction.
    pub fn set_created_order(&mut self, id: &str, ord: String) {
        for op in self.ops.iter_mut() {
            if let Op::NodeCreate { id: i, order, .. } = op {
                if i == id {
                    *order = ord;
                    return;
                }
            }
        }
    }

    /// A node's text as this transaction leaves it: its last text op here, else the store's.
    fn text_now(&self, id: &str) -> Result<String> {
        let pending = self.ops.iter().rev().find_map(|op| match op {
            Op::NodeCreate { id: i, text, .. } if i == id => Some(text.clone()),
            Op::NodeText { id: i, text, .. } if i == id => Some(text.clone()),
            _ => None,
        });
        Ok(match pending {
            Some(t) => t,
            None => self.store.node(id)?.map(|n| n.text).unwrap_or_default(),
        })
    }

    fn store_text_of_pending(&self, id: &str) -> String {
        self.ops
            .iter()
            .rev()
            .find_map(|op| match op {
                Op::NodeCreate { id: i, text, .. } if i == id => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    // ---- edits --------------------------------------------------------------------------------

    pub fn set_text(&mut self, id: &str, text: &str) -> Result<()> {
        let node = self.store.must_node(id)?;
        let (canon, links) = self.canonicalize(text)?;
        if canon != node.text {
            let base: Option<String> =
                self.store.conn.query_row("SELECT text_eid FROM nodes WHERE id=?1", [id], |r| r.get(0)).ok().flatten();
            self.ops.push(Op::NodeText { id: id.into(), text: canon.clone(), base });
        }
        let old_tags = capture::extract_tags(&self.store.render_text(&node.text));
        let tags = capture::extract_tags(&canon);
        self.sync_edges(id, &old_tags, &tags, &links)
    }

    /// Validate and normalize a property value given as a string. Empty string unsets.
    pub fn prop_value(&mut self, key: &str, raw: &str) -> Result<Value> {
        let raw = raw.trim();
        if raw.is_empty() || raw == "null" || raw == "none" {
            return Ok(Value::Null);
        }
        Ok(match key {
            "status" => {
                let s = raw.to_lowercase();
                let s = if s == "open" { "todo".to_string() } else { s };
                if !STATUSES.contains(&s.as_str()) {
                    return Err(invalid(format!("status must be one of {}", STATUSES.join(", "))));
                }
                json!(s)
            }
            "scheduled" | "due" | "done_at" | "end" => json!(dates::parse(raw, self.today)?.fmt()),
            "journal" => json!(dates::parse(raw, self.today)?.date().format("%Y-%m-%d").to_string()),
            "priority" => json!(normalize_priority(raw).ok_or_else(|| invalid("priority must be high, med or low"))?),
            "repeat" => serde_json::to_value(Repeat::parse(raw, None)?)?,
            "title" => json!(raw),
            // A blank line before the note (writing.md §1): always the string "1" or "0", never
            // a typed custom prop.
            "gap" => match raw.to_lowercase().as_str() {
                "1" | "true" | "yes" => json!("1"),
                "0" | "false" | "no" => json!("0"),
                _ => return Err(invalid("gap is 1 (a blank line before the note) or 0")),
            },
            "tag" => return Err(invalid("use `thc tag` to tag nodes")),
            _ => self.custom_value(key, raw)?,
        })
    }

    fn custom_value(&mut self, key: &str, raw: &str) -> Result<Value> {
        if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(invalid(format!("property name {key:?} may only contain letters, digits, _ and -")));
        }
        let known = match self.defined.get(key) {
            Some(t) => Some(t.clone()),
            None => self.store.prop_type(key)?,
        };
        let ty = match known {
            Some(t) => t,
            None => {
                let inferred = if raw.parse::<f64>().is_ok() {
                    "number"
                } else if raw == "true" || raw == "false" {
                    "bool"
                } else if DateVal::from_stored(raw).is_some() {
                    "date"
                } else {
                    "text"
                };
                self.ops.push(Op::PropDefine { key: key.into(), ty: inferred.into() });
                self.defined.insert(key.into(), inferred.into());
                inferred.to_string()
            }
        };
        Ok(match ty.as_str() {
            "number" => json!(raw.parse::<f64>().map_err(|_| invalid(format!("{key} is a number property")))?),
            "bool" => json!(match raw {
                "true" | "yes" | "1" => true,
                "false" | "no" | "0" => false,
                _ => return Err(invalid(format!("{key} is a bool property"))),
            }),
            "date" => json!(dates::parse(raw, self.today)?.fmt()),
            _ => json!(raw),
        })
    }

    /// Edit an existing node from capture-syntax text (in-place editing, tui-handoff §10.3):
    /// the text with tokens taken out, plus the fields the tokens set. Fields not mentioned
    /// are left alone.
    pub fn edit_from_capture(&mut self, id: &str, cap: &crate::capture::Capture) -> Result<()> {
        self.set_text(id, &cap.text)?;
        let mut pairs: Vec<(String, String)> = Vec::new();
        if let Some(s) = &cap.status {
            pairs.push(("status".into(), s.clone()));
        }
        if let Some(d) = &cap.scheduled {
            pairs.push(("scheduled".into(), d.fmt()));
        }
        if let Some(d) = &cap.due {
            pairs.push(("due".into(), d.fmt()));
        }
        if let Some(p) = &cap.priority {
            pairs.push(("priority".into(), p.clone()));
        }
        if let Some(r) = &cap.repeat {
            pairs.push(("repeat".into(), r.text.clone()));
        }
        if !pairs.is_empty() {
            self.set_props(id, &pairs)?;
        }
        Ok(())
    }

    pub fn set_props(&mut self, id: &str, pairs: &[(String, String)]) -> Result<()> {
        let node = self.store.must_node(id)?;
        let mut props = Map::new();
        for (k, raw) in pairs {
            let v = self.prop_value(k, raw)?;
            if k == "status" && v == json!("done") && node.done_at.is_none() {
                props.insert("done_at".into(), json!(dates::now_stamp()));
            }
            if k == "repeat" && !v.is_null() && node.scheduled.is_none() && node.due.is_none() {
                let r: Repeat = serde_json::from_value(v.clone())?;
                props.insert("scheduled".into(), json!(r.first_on_or_after(self.today)?.format("%Y-%m-%d").to_string()));
            }
            props.insert(k.clone(), v);
        }
        if !props.is_empty() {
            self.ops.push(Op::NodeSet { id: id.into(), props });
        }
        Ok(())
    }

    pub fn add_tags(&mut self, id: &str, add: &[String], remove: &[String]) -> Result<()> {
        let current = self.store.edges_from(id)?;
        for t in add {
            let tid = self.tag(t)?;
            if !current.contains(&("tag".into(), tid.clone())) {
                self.ops.push(Op::EdgeAdd { src: id.into(), rel: "tag".into(), dst: tid });
            }
        }
        for t in remove {
            if let Some(tid) = self.store.find_root_by_title(&t.trim_start_matches('#').to_lowercase(), true)? {
                if current.contains(&("tag".into(), tid.clone())) {
                    self.ops.push(Op::EdgeRemove { src: id.into(), rel: "tag".into(), dst: tid });
                }
            }
        }
        Ok(())
    }

    /// Complete a node. Repeating nodes advance their dates and stay open.
    pub fn complete(&mut self, id: &str) -> Result<Option<NextDates>> {
        let node = self.store.must_node(id)?;
        let at = dates::now_stamp();
        let Some(rep_v) = node.repeat.clone() else {
            self.ops.push(Op::NodeComplete { id: id.into(), at, occurrence: None, next: None });
            return Ok(None);
        };
        let rep: Repeat = serde_json::from_value(rep_v)?;
        let (occurrence, next) = self.next_dates(&node, &rep)?;
        self.ops.push(Op::NodeComplete { id: id.into(), at, occurrence: Some(occurrence), next: Some(next.clone()) });
        // Subtasks reset for the next occurrence.
        for child in self.store.children(id)? {
            if child.status.as_deref() == Some("done") {
                let mut p = Map::new();
                p.insert("status".into(), json!("todo"));
                p.insert("done_at".into(), Value::Null);
                self.ops.push(Op::NodeSet { id: child.id, props: p });
            }
        }
        Ok(Some(next))
    }

    pub fn skip(&mut self, id: &str) -> Result<NextDates> {
        let node = self.store.must_node(id)?;
        let Some(rep_v) = node.repeat.clone() else {
            return Err(invalid("only repeating nodes can be skipped"));
        };
        let mut rep: Repeat = serde_json::from_value(rep_v)?;
        rep.mode = Mode::Fixed;
        let (occurrence, next) = self.next_dates(&node, &rep)?;
        self.ops.push(Op::NodeSkip { id: id.into(), occurrence, next: next.clone() });
        Ok(next)
    }

    fn next_dates(&self, node: &crate::model::Node, rep: &Repeat) -> Result<(String, NextDates)> {
        let sched = node.scheduled.as_deref().and_then(DateVal::from_stored);
        let due = node.due.as_deref().and_then(DateVal::from_stored);
        let primary = sched.or(due).unwrap_or(DateVal::Date(self.today));
        let next_primary = rep.next_date(primary.date(), self.today, self.today)?;
        let shift = |d: Option<DateVal>| {
            d.map(|d| d.with_date(crate::recur::shift(d.date(), primary.date(), next_primary)).fmt())
        };
        Ok((primary.fmt(), NextDates { scheduled: shift(sched), due: shift(due) }))
    }

    pub fn delete(&mut self, id: &str) -> Result<usize> {
        self.store.must_node(id)?;
        let mut ids = self.store.descendants(id)?;
        ids.insert(0, id.to_string());
        for i in &ids {
            self.ops.push(Op::NodeDelete { id: i.clone() });
        }
        Ok(ids.len())
    }

    pub fn restore(&mut self, id: &str) -> Result<usize> {
        self.store.must_node(id)?;
        let mut ids = self.store.descendants(id)?;
        ids.insert(0, id.to_string());
        for i in &ids {
            self.ops.push(Op::NodeRestore { id: i.clone() });
        }
        Ok(ids.len())
    }

    /// Move under `parent` (None = root), after/before a sibling, else to the end.
    pub fn move_to(&mut self, id: &str, parent: Option<String>, after: Option<&str>, before: Option<&str>) -> Result<()> {
        self.store.must_node(id)?;
        if let Some(p) = &parent {
            if p == id || self.store.descendants(id)?.contains(p) {
                return Err(invalid("cannot move a node under itself"));
            }
        }
        use crate::model::Place;
        let at = match (after, before) {
            (Some(a), _) => Place::After(a),
            (None, Some(b)) => Place::Before(b),
            (None, None) => Place::Last,
        };
        let (lo, hi) = self.store.neighbour_ords(parent.as_deref(), id, at).map_err(|_| match at {
            Place::After(_) => invalid("--after must be a sibling under the new parent"),
            _ => invalid("--before must be a sibling under the new parent"),
        })?;
        let order = key_between(lo.as_deref(), hi.as_deref());
        self.ops.push(Op::NodeMove { id: id.into(), parent, order });
        Ok(())
    }

    /// Close a node's open text conflict (daemon.md §4.2): keep `current`, `other`, or `both`.
    /// With both, `top` (`current` or `other`) stays on the node and the other version becomes a
    /// new sibling right below it (the editor keeps the writer's own text on the line). Returns
    /// the new node for both.
    pub fn resolve_text_conflict(&mut self, id: &str, keep: &str, top: &str) -> Result<Option<String>> {
        let d = self
            .store
            .conflict_details(Some(id))?
            .into_iter()
            .find(|d| d.kind == "text")
            .ok_or_else(|| crate::error::not_found(format!("{} has no open text conflict", self.store.short(id))))?;
        let current = d.current.as_ref().map(|c| c.text.clone()).unwrap_or_default();
        let other = d.other.as_ref().map(|c| c.text.clone()).unwrap_or_default();
        let (stay, below) = match (keep, top) {
            ("other", _) => (other, None),
            ("both", "other") => (other, Some(current)),
            ("both", _) => (current, Some(other)),
            ("current", _) => (current, None),
            _ => return Err(invalid("keep must be current, other or both")),
        };
        let base: Option<String> = self.store.conn.query_row("SELECT text_eid FROM nodes WHERE id=?1", [id], |r| r.get(0))?;
        self.ops.push(Op::NodeText { id: id.into(), text: stay, base });
        let Some(below) = below else { return Ok(None) };
        let n = self.store.must_node(id)?;
        let text = self.store.render_text(&below);
        // Same form: a task stays a task (open again, since it's a new line).
        let mut props = Map::new();
        if let Some(st) = &n.status {
            props.insert("status".into(), json!(if st == "done" { "todo" } else { st.as_str() }));
        }
        let nid = self.create(n.parent.clone(), &text, None, props, &[], None)?;
        let siblings = match &n.parent {
            Some(p) => self.store.children(p)?,
            None => vec![],
        };
        if let Some(i) = siblings.iter().position(|s| s.id == id) {
            self.set_created_order(&nid, key_between(Some(&siblings[i].ord), siblings.get(i + 1).map(|s| s.ord.as_str())));
        }
        if crate::edit::has_para_style(self.store, id) {
            crate::edit::mark_para(self, &nid)?;
        }
        Ok(Some(nid))
    }

    /// Typed relation `a --rel--> b` (`blocks`, `relates`, or a custom lowercase name).
    pub fn link(&mut self, a: &str, b: &str, rel: &str) -> Result<bool> {
        let rel = rel.trim().to_lowercase();
        if rel == "tag" || rel == "mention" || rel == "mirror_of" {
            return Err(invalid(format!("\"{rel}\" links are managed by thc (use #tags and [[links]] in text)")));
        }
        if rel.is_empty() || !rel.chars().all(|c| c.is_ascii_lowercase() || c == '-' || c == '_') {
            return Err(invalid("relation names are lowercase letters, - or _ (e.g. blocks, relates)"));
        }
        if a == b {
            return Err(invalid("a node can't link to itself"));
        }
        self.store.must_node(a)?;
        self.store.must_node(b)?;
        if self.store.edges_from(a)?.contains(&(rel.clone(), b.to_string())) {
            return Ok(false);
        }
        self.ops.push(Op::EdgeAdd { src: a.into(), rel, dst: b.into() });
        Ok(true)
    }

    pub fn unlink(&mut self, a: &str, b: &str, rel: Option<&str>) -> Result<usize> {
        let mut n = 0;
        for (r, dst) in self.store.edges_from(a)? {
            if dst == b && r != "tag" && r != "mention" && rel.is_none_or(|x| x == r) {
                self.ops.push(Op::EdgeRemove { src: a.into(), rel: r, dst });
                n += 1;
            }
        }
        Ok(n)
    }

    pub fn add_alert(&mut self, node: &str, trigger: Trigger) -> Result<String> {
        if !self.exists(node)? {
            return Err(not_found(format!("node {node}")));
        }
        if trigger.at.is_none() {
            let anchor = trigger.anchor.as_deref().unwrap_or("due");
            let pending_has = self.pending.get(node).is_some_and(|p| p.contains_key(anchor));
            let stored_has = self.store.node(node)?.is_some_and(|n| {
                if anchor == "scheduled" { n.scheduled.is_some() } else { n.due.is_some() }
            });
            if !pending_has && !stored_has {
                return Err(invalid(format!("node has no {anchor} date to anchor a relative alert")));
            }
        }
        let id = id::new_id();
        self.ops.push(Op::AlertAdd { id: id.clone(), node: node.into(), trigger });
        Ok(id)
    }
}
