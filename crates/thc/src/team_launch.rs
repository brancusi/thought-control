//! Launchers are argument arrays, never shell templates. Built-ins use normal permissions.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thc_core::error::invalid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Launcher {
    pub command: Vec<String>,
    #[serde(default)]
    pub model: Vec<String>,
    #[serde(default)]
    pub prompt: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

pub fn load(name: &str, herdr: bool) -> Result<Launcher> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(invalid("agent names must be lowercase names"));
    }
    let mut l = Launcher {
        command: vec![name.into()],
        model: vec![
            if name == "codex" {
                "-m".into()
            } else {
                "--model".into()
            },
            "{model}".into(),
        ],
        prompt: vec!["{prompt}".into()],
        env: BTreeMap::new(),
    };
    let config = thc_core::settings::load(None);
    if let Some(t) = config
        .get(&format!("team.agents.{name}"))
        .and_then(|v| v.as_table())
    {
        for (key, target) in [
            ("command", &mut l.command),
            ("model", &mut l.model),
            ("prompt", &mut l.prompt),
        ] {
            if let Some(v) = t.get(key) {
                *target = v
                    .as_array()
                    .and_then(|a| a.iter().map(|v| v.as_str().map(str::to_string)).collect())
                    .ok_or_else(|| {
                        invalid(format!("launcher {name}.{key} must be an array of strings"))
                    })?;
            }
        }
        if let Some(v) = t.get("env") {
            l.env = v
                .as_table()
                .and_then(|t| {
                    t.iter()
                        .map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .ok_or_else(|| invalid(format!("launcher {name}.env must contain strings")))?;
        }
    } else if !herdr && !matches!(name, "claude" | "codex") {
        return Err(invalid(format!(
            "no launcher {name} · define [team.agents.{name}] in your config"
        )));
    }
    if l.command.is_empty() || l.command[0].is_empty() {
        return Err(invalid(format!("launcher {name}.command is empty")));
    }
    for arg in l.command.iter().chain(&l.model).chain(&l.prompt) {
        safe(name, arg)?;
    }
    for key in l.env.keys() {
        if matches!(
            key.as_str(),
            "THC_ACTOR" | "THC_ROLE" | "THC_BOARD" | "THC_SESSION_ID"
        ) {
            return Err(invalid(format!("launcher {name} cannot override {key}")));
        }
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(invalid("invalid launcher environment name"));
        }
    }
    Ok(l)
}

pub fn safe(name: &str, arg: &str) -> Result<()> {
    let forbidden = [
        "--dangerously-skip-permissions",
        "--dangerously-bypass-approvals-and-sandbox",
        "--yolo",
        "--allow-dangerously-skip-permissions",
        "--permission-mode=bypassPermissions",
        "--approval-mode=yolo",
    ];
    let tokens: Vec<_> = arg
        .split(|c: char| c.is_whitespace() || "'\";|&()".contains(c))
        .collect();
    if let Some(flag) = tokens
        .iter()
        .find(|s| s.starts_with("--dangerously-") || s.starts_with("--allow-dangerously-"))
    {
        return Err(invalid(format!(
            "launcher {name} contains {flag} · thc won't skip permission prompts"
        )));
    }
    if let Some(flag) = forbidden.iter().find(|f| {
        tokens
            .iter()
            .copied()
            .any(|part| part == **f || part.starts_with(&format!("{f}=")))
    }) {
        return Err(invalid(format!(
            "launcher {name} contains {flag} · thc won't start agents that skip your permission prompts"
        )));
    }
    if tokens
        .iter()
        .copied()
        .any(|s| s == "bypassPermissions" || s.starts_with("--permission-mode=bypassPermissions"))
    {
        return Err(invalid(format!(
            "launcher {name} contains bypassPermissions · thc won't skip permission prompts"
        )));
    }
    Ok(())
}

pub fn argv(name: &str, l: &Launcher, model: Option<&str>, prompt: &str) -> Result<Vec<String>> {
    let mut a = l.command.clone();
    if let Some(model) = model {
        safe(name, model)?;
        a.extend(l.model.iter().map(|s| s.replace("{model}", model)));
    }
    a.extend(l.prompt.iter().map(|s| s.replace("{prompt}", prompt)));
    Ok(a)
}

pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Native options, without the opening prompt (herdr sends that separately).
pub fn options(
    name: &str,
    l: &Launcher,
    model: Option<&str>,
    mode: Option<&str>,
) -> Result<Vec<String>> {
    let mut args = Vec::new();
    if let Some(model) = model {
        safe(name, model)?;
        args.extend(l.model.iter().map(|s| s.replace("{model}", model)));
    }
    if let Some(mode) = mode {
        if name != "claude" || !matches!(mode, "auto" | "manual") {
            return Err(invalid("--permission-mode auto|manual is only for claude"));
        }
        args.extend(["--permission-mode".into(), mode.into()]);
    }
    for arg in &args {
        safe(name, arg)?;
    }
    Ok(args)
}

pub const HERDR_KINDS: &[&str] = &[
    "pi",
    "claude",
    "codex",
    "gemini",
    "cursor",
    "devin",
    "agy",
    "cline",
    "omp",
    "mastracode",
    "opencode",
    "copilot",
    "kimi",
    "kiro",
    "droid",
    "amp",
    "grok",
    "hermes",
    "kilo",
    "qodercli",
    "qwen",
    "letta",
    "maki",
    "muse",
];
