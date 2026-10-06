//! Placement and balancing for members added to an existing team.
use super::*;

pub type MemberPane = (String, Option<String>, Vec<String>, Option<String>);

pub fn create_member(
    roster: &crate::team_cmd::Roster,
    env: &BTreeMap<String, String>,
    argv: &[String],
) -> Result<MemberPane> {
    let host = &roster.host;
    let mut args: Vec<String>;
    let mut stack = Vec::new();
    let mut title = None;
    if *host == Host::Herdr {
        args = vec![
            "pane".into(),
            "split".into(),
            "--current".into(),
            "--direction".into(),
            "down".into(),
            "--no-focus".into(),
            "--cwd".into(),
            roster.project.display().to_string(),
        ];
        for (k, v) in env {
            args.extend(["--env".into(), format!("{k}={v}")]);
        }
        let value: Value = serde_json::from_str(&call(host, &roster.context, &args)?)?;
        let pane = value
            .pointer("/result/pane")
            .ok_or_else(|| invalid("herdr split returned no result.pane"))?;
        return Ok((
            pane_id(pane).ok_or_else(|| invalid("herdr split returned no pane id"))?,
            identity(pane),
            stack,
            title,
        ));
    }
    let live = list(host, &roster.context, false)?;
    let owned: Vec<_> = roster
        .members
        .iter()
        .filter_map(|m| {
            let p = m.pane.as_ref()?;
            live.iter()
                .find(|v| pane_id(v).as_ref() == Some(p))
                .map(|v| (m, v))
        })
        .collect();
    for (m, v) in &owned {
        if m.host_identity.is_some() && m.host_identity != identity(v) {
            return Err(invalid("team pane has been replaced · run thc team ls"));
        }
    }
    let (_, last) = owned
        .last()
        .ok_or_else(|| invalid("no live team pane · run thc team up again"))?;
    let last_id = pane_id(last).unwrap();
    let tab = last
        .get("tab_id")
        .ok_or_else(|| invalid("wezterm list returned no tab_id"))?;
    let column = last
        .get("left_col")
        .ok_or_else(|| invalid("wezterm list returned no left_col"))?;
    let lead = roster
        .members
        .first()
        .filter(|m| matches!(m.role.as_str(), "pm" | "lead"));
    stack = owned
        .iter()
        .filter(|(m, v)| {
            v.get("tab_id") == Some(tab)
                && v.get("left_col") == Some(column)
                && lead.is_none_or(|l| m.actor != l.actor)
        })
        .filter_map(|(_, v)| pane_id(v))
        .collect();
    let tabs: std::collections::BTreeSet<_> = owned
        .iter()
        .filter_map(|(_, v)| v.get("tab_id").map(Value::to_string))
        .collect();
    args = vec!["cli".into()];
    if stack.len() >= 5 {
        args.extend(["spawn".into(), "--pane-id".into(), last_id.clone()]);
        stack.clear();
        title = Some(format!("team {}", tabs.len() + 1));
    } else {
        args.extend(["split-pane".into(), "--pane-id".into(), last_id.clone()]);
        if stack.is_empty() {
            args.extend(["--right".into(), "--percent".into(), "60".into()]);
        } else {
            // Never resize manually added panes as a side effect of delegation.
            if live.iter().any(|v| {
                v.get("tab_id") == Some(tab)
                    && v.get("left_col") == Some(column)
                    && pane_id(v).is_some_and(|p| !stack.contains(&p))
            }) {
                return Err(invalid(
                    "team stack contains another pane · move it before adding a teammate",
                ));
            }
            args.extend(["--bottom".into(), "--percent".into(), "50".into()]);
        }
    }
    args.extend([
        "--cwd".into(),
        roster.project.display().to_string(),
        "--".into(),
        "env".into(),
    ]);
    args.extend(env.iter().map(|(k, v)| format!("{k}={v}")));
    args.extend(argv.iter().cloned());
    let pane = call(host, &roster.context, &args)?;
    if pane.is_empty() || !pane.chars().all(|c| c.is_ascii_digit()) {
        return Err(invalid("wezterm returned an invalid pane id"));
    }
    // Return successful creation even if a follow-up read fails: the caller must save
    // the exact pane before any naming or rebalancing can fail.
    let ident = list(host, &roster.context, false).ok().and_then(|rows| {
        rows.into_iter()
            .find(|v| pane_id(v).as_ref() == Some(&pane))
            .and_then(|v| identity(&v))
    });
    stack.push(pane.clone());
    Ok((pane, ident, stack, title))
}

/// Move the nested bottom-split boundaries from top to bottom. Parent adjustments
/// resize all lower panes, so geometry is re-read after each call.
pub fn balance(context: &BTreeMap<String, String>, stack: &[String]) -> Result<()> {
    if stack.len() < 2 {
        return Ok(());
    }
    let rows = list(&Host::Wezterm, context, false)?;
    let heights: Vec<_> = stack
        .iter()
        .map(|id| {
            rows.iter()
                .find(|v| pane_id(v).as_ref() == Some(id))
                .and_then(|v| v.pointer("/size/rows"))
                .and_then(Value::as_u64)
                .ok_or_else(|| invalid("wezterm returned no stack pane height"))
        })
        .collect::<Result<_>>()?;
    let total: u64 = heights.iter().sum();
    let target = total / stack.len() as u64;
    if target == 0 {
        return Err(invalid("team stack is too short to balance"));
    }
    for (i, id) in stack.iter().take(stack.len() - 1).enumerate() {
        let desired = target + u64::from((i as u64) < total % stack.len() as u64);
        let rows = list(&Host::Wezterm, context, false)?;
        let current = rows
            .iter()
            .find(|v| pane_id(v).as_ref() == Some(id))
            .and_then(|v| v.pointer("/size/rows"))
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid("team pane disappeared while balancing"))?;
        if current != desired {
            call(
                &Host::Wezterm,
                context,
                &[
                    "cli".into(),
                    "adjust-pane-size".into(),
                    "--pane-id".into(),
                    id.clone(),
                    "--amount".into(),
                    current.abs_diff(desired).to_string(),
                    if current < desired { "Down" } else { "Up" }.into(),
                ],
            )?;
        }
    }
    Ok(())
}
