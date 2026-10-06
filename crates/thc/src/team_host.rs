//! Host adapters. No control call runs without an explicit pane from a creation response/cache.
#[path = "team_placement.rs"]
mod placement;
use anyhow::Result;
pub use placement::{balance, create_member};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};
use thc_core::error::invalid;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Host {
    Herdr,
    Wezterm,
    Printed,
}

pub fn available(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

pub fn detect() -> Host {
    if std::env::var("HERDR_ENV").is_ok_and(|v| v == "1") && available("herdr") {
        return Host::Herdr;
    }
    if std::env::var_os("WEZTERM_PANE").is_some() && available("wezterm") {
        if call(
            &Host::Wezterm,
            &BTreeMap::new(),
            &[
                "cli".into(),
                "list".into(),
                "--format".into(),
                "json".into(),
            ],
        )
        .is_ok()
        {
            return Host::Wezterm;
        }
    }
    Host::Printed
}

pub fn context(host: &Host) -> BTreeMap<String, String> {
    let keys: &[&str] = match host {
        Host::Herdr => &["HERDR_SOCKET_PATH", "HERDR_WORKSPACE_ID"],
        Host::Wezterm => &["WEZTERM_UNIX_SOCKET"],
        Host::Printed => &[],
    };
    keys.iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect()
}

pub fn call(host: &Host, context: &BTreeMap<String, String>, args: &[String]) -> Result<String> {
    let name = match host {
        Host::Herdr => "herdr",
        Host::Wezterm => "wezterm",
        Host::Printed => return Err(invalid("printed teams have no terminal host")),
    };
    let o = thc_core::sandbox::tool(name)
        .envs(context)
        .args(args)
        .output()
        .map_err(|e| invalid(format!("can't run {name}: {e}")))?;
    if !o.status.success() {
        return Err(invalid(format!(
            "{name}: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )));
    }
    Ok(String::from_utf8(o.stdout)?.trim().to_string())
}

pub fn rows(value: &Value, field: &str) -> Vec<Value> {
    value
        .as_array()
        .or_else(|| {
            value
                .get("result")
                .and_then(|v| v.get(field))
                .and_then(Value::as_array)
        })
        .or_else(|| value.get(field).and_then(Value::as_array))
        .cloned()
        .unwrap_or_default()
}

pub fn pane_id(value: &Value) -> Option<String> {
    value.get("pane_id").map(|p| {
        p.as_str()
            .map(str::to_string)
            .unwrap_or_else(|| p.to_string())
    })
}
pub fn identity(value: &Value) -> Option<String> {
    value
        .get("terminal_id")
        .or_else(|| value.get("tty_name"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub fn list(host: &Host, context: &BTreeMap<String, String>, agents: bool) -> Result<Vec<Value>> {
    if *host == Host::Printed {
        return Ok(Vec::new());
    }
    let args: Vec<String> = match host {
        Host::Herdr => vec![if agents { "agent" } else { "pane" }.into(), "list".into()],
        _ => vec![
            "cli".into(),
            "list".into(),
            "--format".into(),
            "json".into(),
        ],
    };
    let value: Value = serde_json::from_str(&call(host, context, &args)?)?;
    Ok(rows(&value, if agents { "agents" } else { "panes" }))
}

pub fn create(
    host: &Host,
    root: &Path,
    env: &BTreeMap<String, String>,
    argv: &[String],
    prior: &[String],
    total: usize,
) -> Result<(String, Option<String>)> {
    let mut a = Vec::new();
    match host {
        Host::Herdr => {
            a.extend(["pane", "split"].map(str::to_string));
            if let Some(p) = prior.last() {
                a.push(p.clone());
            } else {
                a.push("--current".into());
            }
            a.extend([
                "--direction".into(),
                if prior.len() == 1 { "right" } else { "down" }.into(),
                "--no-focus".into(),
                "--cwd".into(),
                root.display().to_string(),
            ]);
            for (k, v) in env {
                a.extend(["--env".into(), format!("{k}={v}")]);
            }
            let v: Value = serde_json::from_str(&call(host, &BTreeMap::new(), &a)?)?;
            let value = v
                .pointer("/result/pane")
                .ok_or_else(|| invalid("herdr split returned no result.pane"))?;
            let pane = pane_id(value)
                .ok_or_else(|| invalid("herdr split returned no result.pane.pane_id"))?;
            Ok((pane, identity(value)))
        }
        Host::Wezterm => {
            a.push("cli".into());
            if prior.is_empty() {
                a.push("spawn".into());
            } else {
                a.extend([
                    "split-pane".into(),
                    "--pane-id".into(),
                    prior.last().unwrap().clone(),
                ]);
                if prior.len() == 1 {
                    a.extend(["--right".into(), "--percent".into(), "60".into()]);
                } else {
                    a.extend([
                        "--bottom".into(),
                        "--percent".into(),
                        ((100 * (total - prior.len())) / (total + 1 - prior.len())).to_string(),
                    ]);
                }
            }
            a.extend([
                "--cwd".into(),
                root.display().to_string(),
                "--".into(),
                "env".into(),
            ]);
            a.extend(env.iter().map(|(k, v)| format!("{k}={v}")));
            a.extend(argv.iter().cloned());
            let pane = call(host, &BTreeMap::new(), &a)?;
            if pane.is_empty() || !pane.chars().all(|c| c.is_ascii_digit()) {
                return Err(invalid("wezterm returned an invalid pane id"));
            }
            let identity = list(host, &context(host), false).ok().and_then(|rows| {
                rows.into_iter()
                    .find(|r| pane_id(r) == Some(pane.clone()))
                    .and_then(|r| identity(&r))
            });
            Ok((pane, identity))
        }
        Host::Printed => Err(invalid("printed teams do not create panes")),
    }
}

pub fn close(host: &Host, context: &BTreeMap<String, String>, pane: &str) -> Result<()> {
    let args = if *host == Host::Herdr {
        vec!["pane".into(), "close".into(), pane.into()]
    } else {
        vec![
            "cli".into(),
            "kill-pane".into(),
            "--pane-id".into(),
            pane.into(),
        ]
    };
    call(host, context, &args)?;
    Ok(())
}
