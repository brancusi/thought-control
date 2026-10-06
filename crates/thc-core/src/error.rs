//! Typed errors that map to stable CLI exit codes.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ThcError {
    #[error("{0}")]
    Usage(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("ambiguous id {prefix:?} matches: {}", candidates.join(", "))]
    Ambiguous { prefix: String, candidates: Vec<String> },
    #[error("invalid: {0}")]
    Validation(String),
    /// A write precondition (`--if-match`, `--expect`) failed: the node changed since it was read.
    #[error("{message}")]
    Stale { message: String, hint: String, node: String, rev: Option<String>, changed: Vec<serde_json::Value> },
    /// A safety tier or read-only mode refused a write (policy.md §2.3): `kind` is `denied`,
    /// `needs_confirmation` or `readonly`.
    #[error("{message}")]
    Refused { kind: &'static str, message: String, hint: String, actor: String, tier: String, verb: String, config: Option<String> },
    /// A `pre-apply` hook vetoed a write (policy.md §3.4).
    #[error("vetoed by hook {hook}: {message} · nothing written")]
    Vetoed { hook: String, message: String },
}

impl ThcError {
    pub fn exit_code(&self) -> i32 {
        match self {
            ThcError::Usage(_) => 2,
            ThcError::NotFound(_) => 3,
            ThcError::Conflict(_) | ThcError::Stale { .. } => 4,
            ThcError::Ambiguous { .. } => 5,
            ThcError::Validation(_) | ThcError::Refused { .. } | ThcError::Vetoed { .. } => 6,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            ThcError::Usage(_) => "usage",
            ThcError::NotFound(_) => "not_found",
            ThcError::Conflict(_) => "conflict",
            ThcError::Ambiguous { .. } => "ambiguous",
            ThcError::Validation(_) => "validation",
            ThcError::Stale { .. } => "stale",
            ThcError::Refused { kind, .. } => kind,
            ThcError::Vetoed { .. } => "vetoed",
        }
    }
}

pub fn invalid(msg: impl Into<String>) -> anyhow::Error {
    ThcError::Validation(msg.into()).into()
}

pub fn usage(msg: impl Into<String>) -> anyhow::Error {
    ThcError::Usage(msg.into()).into()
}

pub fn not_found(msg: impl Into<String>) -> anyhow::Error {
    ThcError::NotFound(msg.into()).into()
}
