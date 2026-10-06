//! Team messages are ordinary notes with `to`, `from` and per-recipient read properties.
//! No message-specific event types or replay behaviour are needed.

use crate::{builder::TxBuilder, error::invalid, model::Node, store::Store};
use anyhow::Result;
use chrono::NaiveDate;
use rusqlite::types::Value as SqlValue;
use serde_json::{Map, json};

#[derive(Clone, Debug, Default)]
pub struct Recipient {
    pub role: Option<String>,
    pub actor: Option<String>,
    /// Explicit role listings include messages addressed to every actor in that role.
    pub include_role_actors: bool,
}

pub fn validate(name: &str) -> Result<()> {
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_lowercase()) || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(invalid("recipient must be a lowercase role or actor name (letters, digits and hyphens)"));
    }
    Ok(())
}

/// `<agent>-<role>[-<number>]`, including custom roles with hyphens.
pub fn actor_role(actor: &str) -> Option<String> {
    let (_, role) = actor.split_once('-')?;
    let role = match role.rsplit_once('-') {
        Some((r, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => r,
        _ => role,
    };
    (!role.is_empty()).then(|| role.to_string())
}

impl Recipient {
    pub fn new(role: Option<&str>, actor: Option<&str>) -> Self {
        let actor = actor.map(|a| a.strip_prefix("agent:").unwrap_or(a).to_string());
        let role = role.map(str::to_string).or_else(|| actor.as_deref().and_then(actor_role));
        Self { role, actor, include_role_actors: false }
    }

    pub fn current(actor: Option<&str>) -> Self {
        let env_actor = std::env::var("THC_ACTOR").ok();
        let role = std::env::var("THC_ROLE").ok().filter(|s| !s.is_empty());
        Self::new(role.as_deref(), actor.or(env_actor.as_deref()).or(Some("human")))
    }

    pub fn target(target: &str) -> Result<Self> {
        validate(target)?;
        let mut recipient = if target.contains('-') || target == "human" { Self::new(None, Some(target)) } else { Self::new(Some(target), None) };
        recipient.include_role_actors = recipient.actor.is_none();
        Ok(recipient)
    }

    fn roles(&self) -> Vec<String> {
        match self.role.as_deref() {
            Some("pm" | "lead") => vec!["pm".into(), "lead".into()],
            Some(r) => vec![r.into()],
            None => vec![],
        }
    }

    pub fn matches(&self, to: &str) -> bool {
        if to == "all" || self.role.as_deref() == Some("all") {
            return true;
        }
        if self.actor.as_deref() == Some(to) {
            return true;
        }
        let roles = self.roles();
        roles.iter().any(|r| r == to) || (self.include_role_actors && actor_role(to).is_some_and(|r| roles.contains(&r)))
    }

    pub fn read_key(&self) -> String {
        format!("read_{}", self.actor.as_deref().unwrap_or("human"))
    }

    /// Bound SQL over `nodes n`; reusable by the query compiler and message listings.
    pub fn sql(&self, unread: bool, param: &mut impl FnMut(String) -> String) -> Result<String> {
        for name in self.role.iter().chain(self.actor.iter()) {
            validate(name)?;
        }
        let dest = "json_extract(m.value,'$')";
        let mut matches = vec![format!("{dest}='all'")];
        if self.role.as_deref() == Some("all") {
            matches = vec!["1".into()];
        } else {
            for role in self.roles() {
                let p = param(role.clone());
                matches.push(format!("{dest}={p}"));
                if self.include_role_actors {
                    // Exact role component and optional numeric teammate suffix, no substring match.
                    let p = param(format!("*-{role}"));
                    let numbered = param(format!("*-{role}-[0-9]*"));
                    matches.push(format!(
                        "({dest} GLOB {p} OR ({dest} GLOB {numbered} AND \
                        substr({dest}, instr({dest}, '-{role}-') + {}) NOT GLOB '*[^0-9]*'))",
                        role.len() + 2
                    ));
                }
            }
            if let Some(actor) = &self.actor {
                let p = param(actor.clone());
                matches.push(format!("{dest}={p}"));
            }
        }
        let receipt = if unread {
            let key = param(self.read_key());
            format!(" AND NOT EXISTS (SELECT 1 FROM props r WHERE r.node=m.node AND r.key={key} AND r.value='true')")
        } else {
            String::new()
        };
        Ok(format!(
            "n.id IN (SELECT m.node FROM props m WHERE m.key='to' \
            AND json_type(m.value)='text' AND ({}){receipt})",
            matches.join(" OR ")
        ))
    }
}

pub fn list(store: &Store, recipient: &Recipient, unread: bool, limit: usize) -> Result<Vec<Node>> {
    let limit = limit.min(i64::MAX as usize);
    let mut params = Vec::new();
    let clause = recipient.sql(unread, &mut |s| {
        params.push(SqlValue::Text(s));
        format!("?{}", params.len())
    })?;
    let params: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
    store.nodes_where(&format!("n.deleted=0 AND n.is_tag=0 AND {clause} ORDER BY n.created_ms DESC, n.id LIMIT {limit}"), &params)
}

pub fn send(b: &mut TxBuilder, to: &str, from: &str, text: &str, parent: Option<String>, id: Option<String>) -> Result<String> {
    validate(to)?;
    if text.trim().is_empty() {
        return Err(invalid("message text is empty"));
    }
    if let Some(id) = &id {
        if b.store.node_exists(id)? {
            return Ok(id.clone());
        }
    }
    let parent = match parent {
        Some(p) => p,
        None => b.page("Messages", true)?.unwrap(),
    };
    let mut props = Map::new();
    props.insert("to".into(), json!(to));
    props.insert("from".into(), json!(from));
    let plain = b.plain;
    b.plain = true;
    let result = b.create(Some(parent), &format!("to: {to} (from {from}): {text}"), None, props, &[], id);
    b.plain = plain;
    result
}

pub fn mark_read(b: &mut TxBuilder, ids: &[String], recipient: &Recipient) -> Result<()> {
    for id in ids {
        b.store.must_node(id)?;
        let props = b.store.props_of(id)?;
        let to = props.get("to").and_then(|v| v.as_str()).ok_or_else(|| invalid(format!("{id} is not an addressed message")))?;
        if !recipient.matches(to) {
            return Err(invalid(format!("{id} is addressed to {to}, not this recipient")));
        }
        let key = recipient.read_key();
        if props.get(&key) != Some(&json!(true)) {
            b.set_props(id, &[(key, "true".into())])?;
        }
    }
    Ok(())
}

/// Query with an explicit identity; the clock remains the caller's normal query clock.
pub fn query(store: &Store, q: &str, today: NaiveDate, limit: usize, recipient: &Recipient) -> Result<Vec<Node>> {
    let c = crate::query::explain_for(q, store, today, recipient).map_err(|e| e.error)?.compiled;
    let params: Vec<&dyn rusqlite::ToSql> = c.params.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
    store.nodes_where(&format!("{} {} LIMIT {limit}", c.where_sql, c.order_sql), &params)
}
