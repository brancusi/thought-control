//! Project-owned team composition. Device policy and trust stay in the user's config.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use thc_core::error::invalid;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub role: String,
    pub agent: String,
    pub model: Option<String>,
    #[serde(default = "one")]
    pub count: usize,
    pub permission_mode: Option<String>,
}
fn one() -> usize {
    1
}

#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub roster: Option<Vec<Entry>>,
    pub layout: Option<Layout>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    #[serde(default = "columns")]
    pub columns: usize,
    #[serde(default = "tab")]
    pub tab: String,
    pub pm: Option<String>,
}
fn tab() -> String {
    "team".into()
}
fn columns() -> usize {
    3
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            columns: 3,
            tab: tab(),
            pm: Some("bottom-right".into()),
        }
    }
}

/// Creation order: column roots first, then each column's rows. References are member indices.
pub struct Cell {
    pub member: usize,
    pub parent: Option<usize>,
    pub direction: &'static str,
    pub remaining: usize,
}

impl Layout {
    pub fn cells(&self, members: &[super::Member]) -> Vec<Cell> {
        let mut order: Vec<_> = (0..members.len()).collect();
        if self.pm.as_deref() == Some("bottom-right") {
            if let Some(i) = order
                .iter()
                .position(|&i| matches!(members[i].role.as_str(), "pm" | "lead"))
            {
                let lead = order.remove(i);
                order.push(lead);
            }
        }
        let columns = self.columns.min(order.len());
        let mut groups = Vec::new();
        let mut start = 0;
        for col in 0..columns {
            let count = order.len() / columns + usize::from(col < order.len() % columns);
            groups.push(&order[start..start + count]);
            start += count;
        }
        let mut cells = Vec::new();
        for (col, group) in groups.iter().enumerate() {
            cells.push(Cell {
                member: group[0],
                parent: col.checked_sub(1).map(|previous| groups[previous][0]),
                direction: "right",
                remaining: columns + 1 - col,
            });
        }
        for group in &groups {
            for row in 1..group.len() {
                cells.push(Cell {
                    member: group[row],
                    parent: Some(group[row - 1]),
                    direction: "down",
                    remaining: group.len() + 1 - row,
                });
            }
        }
        cells
    }
}

pub fn load(root: &Path) -> Result<Profile> {
    let file = root.join(".thc.toml");
    thc_core::sandbox::check(&file);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Profile::default()),
        Err(e) => return Err(e.into()),
    };
    let config: toml::Table =
        toml::from_str(&text).map_err(|e| invalid(format!(".thc.toml: {e}")))?;
    let Some(team) = config.get("team") else {
        return Ok(Profile::default());
    };
    let team = team
        .as_table()
        .ok_or_else(|| invalid(".thc.toml team must be a table"))?;
    let roster: Option<Vec<Entry>> = team
        .get("roster")
        .map(|v| v.clone().try_into())
        .transpose()
        .map_err(|e| invalid(format!("team.roster: {e}")))?;
    let layout: Option<Layout> = team
        .get("layout")
        .map(|v| v.clone().try_into())
        .transpose()
        .map_err(|e| invalid(format!("team.layout: {e}")))?;
    if let Some(entries) = &roster {
        if entries.is_empty() {
            return Err(invalid("team.roster must contain at least one entry"));
        }
        for entry in entries {
            crate::instructions::role_name(&entry.role)?;
            if entry.count == 0 {
                return Err(invalid("team.roster count must be positive"));
            }
            if entry.model.as_ref().is_some_and(|m| m.trim().is_empty()) {
                return Err(invalid("team.roster model must not be empty"));
            }
            if let Some(mode) = &entry.permission_mode {
                if entry.agent != "claude" || !matches!(mode.as_str(), "auto" | "manual") {
                    return Err(invalid(
                        "team.roster permission_mode auto|manual is only for claude",
                    ));
                }
            }
        }
    }
    if let Some(layout) = &layout {
        if layout.columns == 0 || layout.columns > max_agents() {
            return Err(invalid(
                "team.layout columns must be positive and within max_agents",
            ));
        }
        if layout.tab.trim().is_empty() || layout.tab.chars().any(char::is_control) {
            return Err(invalid(
                "team.layout tab must be a nonempty single-line name",
            ));
        }
        if layout.pm.as_deref().is_some_and(|p| p != "bottom-right") {
            return Err(invalid("team.layout pm must be bottom-right"));
        }
    }
    Ok(Profile { roster, layout })
}

pub fn max_agents() -> usize {
    thc_core::settings::load(None)
        .get("team.max_agents")
        .and_then(|v| v.as_integer())
        .filter(|n| *n > 0)
        .unwrap_or(6) as usize
}

/// CLI overrides apply to a configured roster without changing its order or counts.
pub fn entries(profile: &Profile, choices: &[String], model: Option<&str>) -> Result<Vec<Entry>> {
    let mut entries = profile.roster.clone().ok_or_else(|| {
        invalid("no team roster · list roles, or define [[team.roster]] in .thc.toml")
    })?;
    for choice in choices {
        if let Some((role, agent)) = choice.split_once('=') {
            let mut found = false;
            for entry in entries.iter_mut().filter(|e| e.role == role) {
                entry.agent = agent.into();
                found = true;
            }
            if !found {
                return Err(invalid(format!("agent override names missing role {role}")));
            }
        } else {
            for entry in &mut entries {
                entry.agent = choice.clone();
            }
        }
    }
    if let Some(model) = model {
        for entry in &mut entries {
            entry.model = Some(model.into());
        }
    }
    let count = entries
        .iter()
        .try_fold(0usize, |n, e| n.checked_add(e.count))
        .ok_or_else(|| invalid("team roster count is too large"))?;
    if count > max_agents() {
        return Err(invalid(format!(
            "team exceeds max_agents ({})",
            max_agents()
        )));
    }
    Ok(entries)
}
