//! The event log contract (see docs/FORMAT.md). One JSON object per line.

use crate::hlc::{Hlc, order_key};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The newest format this binary reads and writes. Each event carries the lowest version that
/// understands it (`Op::min_version`), so v1 readers keep reading everything that predates v2.
pub const FORMAT_VERSION: u32 = 2;

/// Every op this binary understands. A line with any other op is from a newer writer and is
/// skipped (FORMAT.md: readers ignore unknown ops).
pub const KNOWN_OPS: [&str; 17] = [
    "node.create", "node.text", "node.set", "node.move", "node.complete", "node.skip", "node.delete",
    "node.restore", "edge.add", "edge.remove", "alert.add", "alert.ack", "alert.snooze", "alert.remove",
    "prop.define", "tx.review", "tx.unreview",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Actor {
    pub kind: String, // "human" | "agent"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Actor {
    pub fn label(&self) -> String {
        match &self.name {
            Some(n) => format!("{}:{}", self.kind, n),
            None => self.kind.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub v: u32,
    pub eid: String,
    pub hlc: Hlc,
    pub dev: String,
    pub actor: Actor,
    pub via: String,
    pub tx: String,
    #[serde(flatten)]
    pub op: Op,
}

impl Event {
    pub fn order_key(&self) -> String {
        order_key(self.hlc, &self.dev, &self.eid)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trigger {
    /// Absolute local datetime `YYYY-MM-DDTHH:MM`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Relative offset such as `-1d`, `-15m`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<String>,
    /// `scheduled` | `due`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NextDates {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Op {
    #[serde(rename = "node.create")]
    NodeCreate {
        id: String,
        #[serde(default)]
        parent: Option<String>,
        order: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        props: Map<String, Value>,
    },
    #[serde(rename = "node.text")]
    NodeText {
        id: String,
        text: String,
        #[serde(default)]
        base: Option<String>,
    },
    /// Set (or with null, unset) properties. `title` is set here too.
    #[serde(rename = "node.set")]
    NodeSet { id: String, props: Map<String, Value> },
    #[serde(rename = "node.move")]
    NodeMove {
        id: String,
        #[serde(default)]
        parent: Option<String>,
        order: String,
    },
    /// Complete. For repeating nodes the writer computes `next` and the node stays open.
    #[serde(rename = "node.complete")]
    NodeComplete {
        id: String,
        /// Local completion time.
        at: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        occurrence: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next: Option<NextDates>,
    },
    #[serde(rename = "node.skip")]
    NodeSkip { id: String, occurrence: String, next: NextDates },
    #[serde(rename = "node.delete")]
    NodeDelete { id: String },
    #[serde(rename = "node.restore")]
    NodeRestore { id: String },
    #[serde(rename = "edge.add")]
    EdgeAdd { src: String, rel: String, dst: String },
    #[serde(rename = "edge.remove")]
    EdgeRemove { src: String, rel: String, dst: String },
    #[serde(rename = "alert.add")]
    AlertAdd { id: String, node: String, trigger: Trigger },
    #[serde(rename = "alert.ack")]
    AlertAck { id: String },
    #[serde(rename = "alert.snooze")]
    AlertSnooze { id: String, until: String },
    #[serde(rename = "alert.remove")]
    AlertRemove { id: String },
    #[serde(rename = "prop.define")]
    PropDefine {
        key: String,
        #[serde(rename = "type")]
        ty: String,
    },
    /// v2. A verdict on other transactions: `accepted` (human only) or `reverted`. Changes no node.
    #[serde(rename = "tx.review")]
    TxReview { txs: Vec<String>, verdict: String },
    /// v2. Withdraw a verdict, putting the transactions back in the review queue.
    #[serde(rename = "tx.unreview")]
    TxUnreview { txs: Vec<String> },
}

impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Op::NodeCreate { .. } => "node.create",
            Op::NodeText { .. } => "node.text",
            Op::NodeSet { .. } => "node.set",
            Op::NodeMove { .. } => "node.move",
            Op::NodeComplete { .. } => "node.complete",
            Op::NodeSkip { .. } => "node.skip",
            Op::NodeDelete { .. } => "node.delete",
            Op::NodeRestore { .. } => "node.restore",
            Op::EdgeAdd { .. } => "edge.add",
            Op::EdgeRemove { .. } => "edge.remove",
            Op::AlertAdd { .. } => "alert.add",
            Op::AlertAck { .. } => "alert.ack",
            Op::AlertSnooze { .. } => "alert.snooze",
            Op::AlertRemove { .. } => "alert.remove",
            Op::PropDefine { .. } => "prop.define",
            Op::TxReview { .. } => "tx.review",
            Op::TxUnreview { .. } => "tx.unreview",
        }
    }

    /// The lowest format version whose readers understand this op.
    pub fn min_version(&self) -> u32 {
        match self {
            Op::TxReview { .. } | Op::TxUnreview { .. } => 2,
            _ => 1,
        }
    }

    pub fn is_review(&self) -> bool {
        matches!(self, Op::TxReview { .. } | Op::TxUnreview { .. })
    }

    /// The primary entity this op touches (for history lookups).
    pub fn entity(&self) -> String {
        match self {
            Op::NodeCreate { id, .. }
            | Op::NodeText { id, .. }
            | Op::NodeSet { id, .. }
            | Op::NodeMove { id, .. }
            | Op::NodeComplete { id, .. }
            | Op::NodeSkip { id, .. }
            | Op::NodeDelete { id }
            | Op::NodeRestore { id } => id.clone(),
            Op::EdgeAdd { src, .. } | Op::EdgeRemove { src, .. } => src.clone(),
            Op::AlertAdd { node, .. } => node.clone(),
            Op::AlertAck { id } | Op::AlertSnooze { id, .. } | Op::AlertRemove { id } => id.clone(),
            Op::PropDefine { key, .. } => format!("prop:{key}"),
            Op::TxReview { .. } | Op::TxUnreview { .. } => "review".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_flat_json() {
        let e = Event {
            v: 1,
            eid: "01J".into(),
            hlc: Hlc(1759500000123, 0),
            dev: "mbp-7f3a".into(),
            actor: Actor { kind: "agent".into(), name: Some("claude".into()) },
            via: "cli".into(),
            tx: "01J".into(),
            op: Op::NodeSet { id: "k3f9a2mq7x1c".into(), props: serde_json::from_str(r#"{"due":"2026-10-06"}"#).unwrap() },
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains(r#""op":"node.set""#), "{s}");
        assert!(s.contains(r#""hlc":[1759500000123,0]"#), "{s}");
        let back: Event = serde_json::from_str(&s).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn known_ops_cover_every_variant() {
        let ops = [
            Op::TxReview { txs: vec!["T".into()], verdict: "accepted".into() },
            Op::TxUnreview { txs: vec!["T".into()] },
            Op::PropDefine { key: "k".into(), ty: "text".into() },
        ];
        for op in ops {
            let v = serde_json::to_value(&op).unwrap();
            assert!(KNOWN_OPS.contains(&v["op"].as_str().unwrap()));
            assert_eq!(v["op"], op.name());
        }
    }
}
