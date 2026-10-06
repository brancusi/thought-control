//! Safety tiers and read-only mode (docs/design/policy.md §2): who may run which verbs.
//!
//! A guard against mistakes, not a security boundary: anything that can run `thc` can set
//! `THC_ACTOR=human` or edit the config. Policy lives in **device config**
//! (`~/.config/thought/config.toml`, or `$THC_CONFIG_DIR/config.toml`), never in the vault,
//! so a synced folder can't change what another machine allows.
//!
//! Order of checks for every write (§1): read-only, the tier's deny, the tier's confirm,
//! then `pre-apply` hooks (crate::hooks), the write, and `post-apply` hooks.

use crate::error::ThcError;
use crate::event::{Actor, Op};
use anyhow::Result;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// What `write` allows (§2.1). `apply`/`import` up to 20 ops; `undo` of the agent's own txs.
pub const WRITE_VERBS: &[&str] = &[
    "msg", "add", "attach", "todo", "remind", "done", "reopen", "skip", "set", "text", "tag", "mv", "link", "unlink", "alert", "apply", "import", "view",
    "views", "undo", "page",
];

/// Verb classes (§2.1): what each one covers.
pub const CLASSES: &[(&str, &str)] = &[
    ("delete", "rm, apply lines with rm"),
    ("move-root", "mv --root"),
    ("undo-others", "undo of a transaction made by someone else"),
    ("undo-bulk", "undo --by, review revert with more than one tx"),
    ("apply-large", "apply / import with more than 20 ops"),
    ("conflict", "conflict resolve"),
    ("views", "view add/set/rm"),
];

/// A class in plain words: (after "can't", as the subject of "needs confirmation").
pub fn class_words(class: &str) -> Option<(&'static str, &'static str)> {
    Some(match class {
        "delete" => ("delete", "deleting"),
        "move-root" => ("move to the top level", "moving to the top level"),
        "undo-others" => ("undo someone else's change", "undoing someone else's change"),
        "undo-bulk" => ("undo in bulk", "undoing in bulk"),
        "apply-large" => ("apply more than 20 ops at once", "applying more than 20 ops at once"),
        "conflict" => ("resolve conflicts", "resolving conflicts"),
        "views" => ("change saved views", "changing saved views"),
        _ => return None,
    })
}

/// `apply` / `import` past this many ops is `apply-large`.
pub const LARGE: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Read,
    Write,
    Full,
}

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Tier::Read => "read",
            Tier::Write => "write",
            Tier::Full => "full",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    Confirm,
}

#[derive(Debug, Default, Deserialize)]
struct ActorSection {
    tier: Option<String>,
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
    #[serde(default)]
    confirm: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct DeviceConfig {
    #[serde(default)]
    actors: BTreeMap<String, ActorSection>,
}

/// The effective policy for one actor on this device.
#[derive(Clone, Debug)]
pub struct Policy {
    /// `claude`, `human`, or `default-agent` when the agent has no section of its own.
    pub section: String,
    /// The actor as written in refusals: `claude`, `human`.
    pub who: String,
    pub tier: Tier,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub confirm: Vec<String>,
    /// Where it came from: `~/.config/thought/config.toml [actors.claude]`, or `default`.
    pub source: String,
    /// `--readonly` or `THC_READONLY`, when on.
    pub readonly: Option<&'static str>,
}

/// `$THC_CONFIG_DIR`, else `~/.config/thought`: device config (policy, hooks).
pub fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("THC_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let d = home.join(".config/thought");
    crate::sandbox::check(&d);
    d
}

/// `~/.config/thought/config.toml` with the home directory folded to `~`, for messages.
pub fn config_path_display() -> String {
    let p = config_dir().join("config.toml");
    match (std::env::var_os("HOME"), p.to_str()) {
        (Some(h), Some(s)) if !h.is_empty() => match s.strip_prefix(h.to_str().unwrap_or("\u{0}")) {
            Some(rest) => format!("~{rest}"),
            None => s.to_string(),
        },
        _ => p.display().to_string(),
    }
}

/// `--readonly` or `THC_READONLY=1`.
pub fn readonly_source(flag: bool) -> Option<&'static str> {
    if flag {
        return Some("--readonly");
    }
    match std::env::var("THC_READONLY") {
        Ok(v) if !v.is_empty() && v != "0" && v != "false" => Some("THC_READONLY"),
        _ => None,
    }
}

impl Policy {
    /// The policy for `actor`. A config that doesn't parse is an error, not "allow".
    pub fn load(actor: &Actor, readonly_flag: bool) -> Result<Policy> {
        let path = config_dir().join("config.toml");
        let cfg: DeviceConfig = match std::fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).map_err(|e| crate::error::invalid(format!("{} doesn't parse: {e}", config_path_display())))?,
            Err(_) => DeviceConfig::default(),
        };
        let human = actor.kind != "agent";
        let who = if human { "human".to_string() } else { actor.name.clone().unwrap_or_else(|| "agent".into()) };
        let (section, sec) = if human {
            ("human".to_string(), cfg.actors.get("human"))
        } else {
            match cfg.actors.get(&who) {
                Some(s) => (who.clone(), Some(s)),
                None => ("default-agent".to_string(), cfg.actors.get("default-agent")),
            }
        };
        let default_tier = if human { Tier::Full } else { Tier::Write };
        let tier = match sec.and_then(|s| s.tier.as_deref()) {
            None => default_tier,
            Some("read") => Tier::Read,
            Some("write") => Tier::Write,
            Some("full") => Tier::Full,
            Some(t) => return Err(crate::error::invalid(format!("unknown tier {t:?} in [actors.{section}] · use read, write or full"))),
        };
        let source = match sec {
            Some(_) => format!("{} [actors.{section}]", config_path_display()),
            None => "default".to_string(),
        };
        let norm = |v: &Vec<String>| v.iter().map(|x| x.trim().to_lowercase()).collect::<Vec<_>>();
        Ok(Policy {
            section,
            who,
            tier,
            allow: sec.map(|s| norm(&s.allow)).unwrap_or_default(),
            deny: sec.map(|s| norm(&s.deny)).unwrap_or_default(),
            confirm: sec.map(|s| norm(&s.confirm)).unwrap_or_default(),
            source,
            readonly: readonly_source(readonly_flag),
        })
    }

    /// The section a person would edit: the agent's own, even when it runs on `default-agent`.
    fn hint_section(&self) -> &str {
        if self.section == "default-agent" { &self.who } else { &self.section }
    }

    /// One verb: explicit deny, then confirm, then allow, then the tier's preset.
    pub fn decide(&self, verb: &str) -> Decision {
        let v = verb.to_string();
        if self.deny.contains(&v) {
            return Decision::Deny;
        }
        if self.confirm.contains(&v) {
            return Decision::Confirm;
        }
        if self.allow.contains(&v) {
            return Decision::Allow;
        }
        match self.tier {
            Tier::Full => Decision::Allow,
            Tier::Read => Decision::Deny,
            Tier::Write => {
                if WRITE_VERBS.contains(&verb) {
                    Decision::Allow
                } else {
                    Decision::Deny
                }
            }
        }
    }

    /// Check a write made of `verbs` (the command, plus the classes its ops fall in).
    /// `yes`: the caller says a human agreed (§2.3: for agents it means "I asked").
    pub fn check(&self, verbs: &[String], yes: bool) -> Result<()> {
        self.check_at(verbs, yes, None)
    }

    /// Same, naming where in a batch the refused part is (`apply line 3`).
    pub fn check_at(&self, verbs: &[String], yes: bool, at: Option<&str>) -> Result<()> {
        if let Some(src) = self.readonly {
            return Err(ThcError::Refused {
                kind: "readonly",
                message: format!("read-only ({src}) · nothing written"),
                hint: String::new(),
                actor: self.who.clone(),
                tier: self.tier.name().into(),
                verb: verbs.first().cloned().unwrap_or_default(),
                config: None,
            }
            .into());
        }
        if let Some(v) = verbs.iter().find(|v| self.decide(v) == Decision::Deny) {
            // Classes in plain words; the token stays in JSON `verb` and `thc policy`.
            let what = class_words(v).map(|(can, _)| can.to_string()).unwrap_or_else(|| v.clone());
            let paren = match at {
                Some(a) => format!("({a}, tier: {})", self.tier.name()),
                None => format!("(tier: {})", self.tier.name()),
            };
            return Err(ThcError::Refused {
                kind: "denied",
                message: format!("{} can't {what} {paren} · nothing written", self.who),
                hint: format!("ask the human to do it, or to change [actors.{}] in {}", self.hint_section(), config_path_display()),
                actor: self.who.clone(),
                tier: self.tier.name().into(),
                verb: v.clone(),
                config: Some(format!("{} [actors.{}]", config_path_display(), self.hint_section())),
            }
            .into());
        }
        if !yes {
            if let Some(v) = verbs.iter().find(|v| self.decide(v) == Decision::Confirm) {
                let what = class_words(v).map(|(_, ing)| ing.to_string()).unwrap_or_else(|| v.clone());
                return Err(ThcError::Refused {
                    kind: "needs_confirmation",
                    message: format!("{what} needs confirmation for {} · nothing written", self.who),
                    hint: "ask the human first, then run it again with --yes".into(),
                    actor: self.who.clone(),
                    tier: self.tier.name().into(),
                    verb: v.clone(),
                    config: None,
                }
                .into());
            }
        }
        Ok(())
    }

    /// Verbs this policy confirms or denies among the ones worth naming (`thc policy`, prime).
    pub fn summary(&self) -> (Vec<String>, Vec<String>, Vec<String>) {
        let named: Vec<&str> = [
            "msg", "add", "attach", "todo", "remind", "done", "reopen", "skip", "set", "text", "tag", "mv", "link", "alert", "apply", "view", "undo", "rm", "restore",
            "rewind", "edit", "move-root", "undo-others", "undo-bulk", "apply-large", "conflict",
        ]
        .into();
        let mut can = Vec::new();
        let mut confirm = Vec::new();
        let mut deny = Vec::new();
        let extra: Vec<String> = self.confirm.iter().chain(&self.deny).chain(&self.allow).filter(|v| !named.contains(&v.as_str())).cloned().collect();
        for v in named.iter().map(|s| s.to_string()).chain(extra) {
            match self.decide(&v) {
                Decision::Allow => can.push(v),
                Decision::Confirm => confirm.push(v),
                Decision::Deny => deny.push(v),
            }
        }
        (can, confirm, deny)
    }
}

/// Classes a transaction's ops fall in, beyond the command's own verb: `delete` for any
/// node.delete, `move-root` for a move to the root, `apply-large` past [`LARGE`] ops.
pub fn op_classes(ops: &[Op], batch: bool) -> Vec<String> {
    let mut v = Vec::new();
    if ops.iter().any(|o| matches!(o, Op::NodeDelete { .. })) {
        v.push("delete".to_string());
    }
    if ops.iter().any(|o| matches!(o, Op::NodeMove { parent: None, .. })) {
        v.push("move-root".to_string());
    }
    if batch && ops.len() > LARGE {
        v.push("apply-large".to_string());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pol(tier: Tier) -> Policy {
        Policy { section: "claude".into(), who: "claude".into(), tier, allow: vec![], deny: vec![], confirm: vec![], source: "default".into(), readonly: None }
    }

    #[test]
    fn presets_and_overrides() {
        let w = pol(Tier::Write);
        assert_eq!(w.decide("add"), Decision::Allow);
        assert_eq!(w.decide("rm"), Decision::Deny);
        assert_eq!(w.decide("delete"), Decision::Deny);
        assert_eq!(w.decide("rewind"), Decision::Deny);
        assert_eq!(pol(Tier::Read).decide("add"), Decision::Deny);
        assert_eq!(pol(Tier::Full).decide("rm"), Decision::Allow);
        let mut c = pol(Tier::Write);
        c.confirm = vec!["rewind".into()];
        c.deny = vec!["tag".into()];
        assert_eq!(c.decide("rewind"), Decision::Confirm);
        assert_eq!(c.decide("tag"), Decision::Deny);
        let e = c.check(&["rewind".into()], false).unwrap_err().to_string();
        assert!(e.contains("rewind needs confirmation for claude · nothing written"), "{e}");
        assert!(c.check(&["rewind".into()], true).is_ok());
        let e = w.check(&["rm".into(), "delete".into()], true).unwrap_err().to_string();
        assert!(e.contains("claude can't rm (tier: write) · nothing written"), "{e}");
        assert!(w.check(&["undo".into(), "undo-others".into()], true).unwrap_err().to_string().contains("claude can't undo someone else's change (tier: write)"));
        let e = w.check_at(&["apply".into(), "delete".into()], false, Some("apply line 3")).unwrap_err().to_string();
        assert!(e.contains("claude can't delete (apply line 3, tier: write)"), "{e}");
        let mut b = pol(Tier::Write);
        b.confirm = vec!["undo-bulk".into()];
        assert!(b.check(&["undo".into(), "undo-bulk".into()], false).unwrap_err().to_string().contains("undoing in bulk needs confirmation for claude"));
        let mut r = pol(Tier::Full);
        r.readonly = Some("THC_READONLY");
        assert!(r.check(&["add".into()], true).unwrap_err().to_string().contains("read-only (THC_READONLY) · nothing written"));
    }
}
