//! Single-member lifecycle, including delegation rules and board audit notes.
use super::{Member, Roster, load, members, save, status};
use crate::{
    cli::Cli,
    out::Out,
    team_host::{self, Host},
    team_launch,
};
use anyhow::Result;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::{IsTerminal, Write},
    path::Path,
};
use thc_core::{
    builder::TxBuilder,
    capture::Capture,
    error::invalid,
    vault::{Paths, Vault},
};

pub fn authorize(cli: &Cli, permission_mode: Option<&str>) -> Result<()> {
    let actor = crate::parse_actor(cli.actor.as_deref());
    let mut policy = thc_core::policy::Policy::load(&actor, cli.readonly)?;
    if actor.kind == "agent" {
        let role = std::env::var("THC_ROLE")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                actor
                    .name
                    .as_deref()
                    .and_then(thc_core::messages::actor_role)
            });
        if !matches!(role.as_deref(), Some("pm" | "lead")) {
            return Err(invalid("only the human or the lead adds teammates"));
        }
        if !thc_core::settings::load(None)
            .get("team.lead_can_add")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Err(invalid(
                "adding teammates needs your OK · set [team] lead_can_add = true, or run it yourself",
            ));
        }
        if permission_mode.is_some() {
            return Err(invalid(
                "an agent cannot choose Claude's permission mode · omit --permission-mode to keep your default; only a human may choose auto or manual",
            ));
        }
        // The device's explicit delegation setting authorizes team operations for a write-tier
        // lead. Read tiers and explicit deny/confirm entries continue to apply.
        if policy.tier == thc_core::policy::Tier::Write {
            policy.allow.push("team".into());
        }
    }
    if !cli.dry_run {
        policy.check(&["team".into(), "add".into()], cli.yes)?;
    }
    Ok(())
}

fn running(root: &Path, cli: &Cli) -> Result<(Roster, Vault)> {
    let roster = load(root)?
        .filter(|r| !r.members.is_empty())
        .ok_or_else(|| invalid("no running team for this project · run thc team up first"))?;
    // Cached membership belongs to its original board, even from another shell. Explicit
    // overrides may not silently send task notes to a different board.
    if cli.board.is_some() || cli.vault.is_some() || std::env::var_os("THC_BOARD").is_some() {
        let (_, requested) = crate::team_board::resolve(cli)?;
        if requested.path != roster.board.path
            || requested.page.as_ref().map(|p| &p.id) != roster.board.page.as_ref().map(|p| &p.id)
        {
            return Err(invalid(
                "team belongs to another board · use thc team ls to inspect it",
            ));
        }
    }
    let vault = Vault::open(
        Paths::resolve(Some(&roster.board.path))?,
        crate::parse_actor(cli.actor.as_deref()),
        "cli",
    )?;
    Ok((roster, vault))
}

fn task_id(vault: &Vault, roster: &Roster, input: &str) -> Result<String> {
    let id = vault.store.resolve(input)?;
    let node = vault
        .store
        .node(&id)?
        .ok_or_else(|| invalid("task does not exist"))?;
    if node.status.is_none() {
        return Err(invalid("--task must name a task"));
    }
    if let Some(page) = &roster.board.page {
        let mut parent = node.parent;
        while let Some(p) = parent {
            if p == page.id {
                return Ok(id);
            }
            parent = vault.store.node(&p)?.and_then(|n| n.parent);
        }
        return Err(invalid("--task is outside this team's board"));
    }
    Ok(id)
}

fn audit(vault: &mut Vault, member: &Member, verb: &str, tasks: &[String]) -> Result<()> {
    let actor = vault
        .actor
        .name
        .as_deref()
        .unwrap_or(&vault.actor.kind)
        .to_string();
    let text = format!("◆ {actor} {verb} {} · {}", member.actor, member.role);
    vault.transact(|store| {
        let mut b = TxBuilder::new(store, thc_core::dates::today());
        let about = b.page("About", true)?.unwrap();
        let heading = store
            .children(&about)?
            .into_iter()
            .find(|n| n.text.lines().next() == Some("## Members"));
        let parent = match heading {
            Some(n) => n.id,
            None => b.create_from_capture(
                Some(about),
                &Capture {
                    text: "## Members".into(),
                    ..Default::default()
                },
                None,
            )?,
        };
        b.create_from_capture(
            Some(parent),
            &Capture {
                text: text.clone(),
                ..Default::default()
            },
            None,
        )?;
        for id in tasks {
            if store.node(id)?.is_some() {
                let text = format!("◆ {actor} {verb} {} for this task", member.actor);
                b.create_from_capture(
                    Some(id.clone()),
                    &Capture {
                        text,
                        ..Default::default()
                    },
                    None,
                )?;
            }
        }
        Ok((b.finish(), ()))
    })?;
    Ok(())
}

pub fn add(
    cli: &Cli,
    root: &Path,
    role: &str,
    agent: &str,
    model: Option<&str>,
    mode: Option<&str>,
    task: Option<&str>,
    out: &mut Out,
) -> Result<()> {
    let (mut roster, mut vault) = running(root, cli)?;
    if roster.members.iter().all(|m| m.state == "stopped") {
        return Err(invalid(
            "no running team for this project · run thc team up first",
        ));
    }

    let max = thc_core::settings::load(None)
        .get("team.max_agents")
        .and_then(|v| v.as_integer())
        .filter(|n| *n > 0)
        .unwrap_or(6) as usize;
    if roster.members.len() >= max {
        return Err(invalid(format!("team exceeds max_agents ({max})")));
    }
    let who = vault.actor.name.as_deref().unwrap_or(&vault.actor.kind);
    let (mut member, launcher) =
        members(&[role.into()], &[agent.into()], model, &roster.host, who)?.remove(0);
    let options = team_launch::options(agent, &launcher, model, mode)?;
    if roster.host == Host::Herdr && !team_launch::HERDR_KINDS.contains(&agent) {
        return Err(invalid(format!(
            "herdr has no agent kind {agent} · thc team agents"
        )));
    }
    let live = team_host::list(&roster.host, &roster.context, roster.host == Host::Herdr)?;
    let mut taken: BTreeSet<String> = roster.members.iter().map(|m| m.actor.clone()).collect();
    taken.extend(
        live.iter()
            .filter_map(|v| v.get("name").and_then(Value::as_str).map(str::to_string)),
    );
    let base = format!("{agent}-{role}");
    let mut name = base.clone();
    let mut suffix = 2;
    while taken.contains(&name) {
        name = format!("{base}-{suffix}");
        suffix += 1;
    }
    if name.len() > 32 {
        return Err(invalid("team actor name exceeds 32 characters"));
    }
    member.actor = name;
    member.task = task.map(|t| task_id(&vault, &roster, t)).transpose()?;
    let mut env = launcher.env.clone();
    env.insert("THC_ACTOR".into(), member.actor.clone());
    env.insert("THC_ROLE".into(), member.role.clone());
    env.insert(
        "THC_BOARD".into(),
        roster
            .board
            .page
            .as_ref()
            .map(|p| format!("{}:¶ {}", roster.board.path.display(), p.title))
            .unwrap_or_else(|| roster.board.path.display().to_string()),
    );
    env.insert("THC_SESSION_ID".into(), String::new());
    let mut prompt = format!(
        "You're the {} on this project's team. Run thc prime --role {} and follow it. Coordinate only through the board.",
        role, role
    );
    if let Some(id) = &member.task {
        let claim = crate::team_roles::claim(&roster.board, id, &member.actor);
        let board_path = team_launch::quote(&roster.board.path.to_string_lossy());
        prompt.push_str(&format!(" Your first task is {id}: read it with thc show {id} --vault {board_path} --depth 1 --json, then claim it with {claim} and start only on exit 0. If it was taken, fall back to thc next --role {role} --json. Never claim on another agent's behalf."));
    }
    let mut argv = launcher.command.clone();
    argv.extend(options.iter().cloned());
    argv.extend(
        launcher
            .prompt
            .iter()
            .map(|s| s.replace("{prompt}", &prompt)),
    );
    let command = format!(
        "cd {} && env {} {}",
        team_launch::quote(&root.to_string_lossy()),
        env.iter()
            .map(|(k, v)| team_launch::quote(&format!("{k}={v}")))
            .collect::<Vec<_>>()
            .join(" "),
        argv.iter()
            .map(|s| team_launch::quote(s))
            .collect::<Vec<_>>()
            .join(" ")
    );
    if cli.dry_run {
        if cli.json {
            out.json(&json!({"dry_run":true,"member":member,"command":command,"host":roster.host}));
        } else {
            out.line(command);
        }
        return Ok(());
    }
    let mut layout = Vec::new();
    let mut title = None;
    if roster.host == Host::Printed {
        member.state = "printed".into();
    } else {
        let (pane, identity, stack, tab_title) = team_host::create_member(&roster, &env, &argv)?;
        member.pane = Some(pane);
        member.host_identity = identity;
        member.state = "starting".into();
        layout = stack;
        title = tab_title;
    }
    roster.members.push(member.clone());
    save(&roster)?; // Before audit/start/prompt: partial failures retain the exact pane to remove.
    audit(
        &mut vault,
        &member,
        "added",
        &member.task.iter().cloned().collect::<Vec<_>>(),
    )?;
    if roster.host == Host::Herdr {
        let pane = member.pane.as_ref().unwrap();
        team_host::call(
            &roster.host,
            &roster.context,
            &["pane".into(), "rename".into(), pane.clone(), role.into()],
        )?;
        let mut args = vec![
            "agent".into(),
            "start".into(),
            member.actor.clone(),
            "--kind".into(),
            agent.into(),
            "--pane".into(),
            pane.clone(),
        ];
        if !options.is_empty() {
            args.push("--".into());
            args.extend(options);
        }
        team_host::call(&roster.host, &roster.context, &args)?;
        team_host::call(
            &roster.host,
            &roster.context,
            &[
                "agent".into(),
                "prompt".into(),
                member.actor.clone(),
                prompt,
            ],
        )?;
    } else if roster.host == Host::Wezterm {
        if let Some(title) = title {
            team_host::call(
                &roster.host,
                &roster.context,
                &[
                    "cli".into(),
                    "set-tab-title".into(),
                    "--pane-id".into(),
                    member.pane.clone().unwrap(),
                    title,
                ],
            )?;
        }
        team_host::balance(&roster.context, &layout)?;
    }
    if roster.host != Host::Printed {
        roster.members.last_mut().unwrap().state = "running".into();
    }
    save(&roster)?;
    if cli.json {
        out.json(&json!({"ok":true,"board":roster.board,"host":roster.host,"member":roster.members.last(),"command":command}));
    } else {
        out.line(format!(
            "added {} · pane {}{}",
            member.actor,
            member.pane.as_deref().unwrap_or("—"),
            member
                .task
                .as_ref()
                .map(|t| format!(" · on {t}"))
                .unwrap_or_default()
        ));
        if roster.host == Host::Printed {
            out.line(command);
        }
    }
    Ok(())
}

pub fn remove(cli: &Cli, root: &Path, target: &str, out: &mut Out) -> Result<()> {
    let (mut roster, mut vault) = running(root, cli)?;
    let index = if let Some(i) = roster.members.iter().position(|m| m.actor == target) {
        i
    } else {
        let candidates: Vec<_> = roster
            .members
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == target)
            .map(|(i, _)| i)
            .collect();
        match candidates.as_slice() {
            [i] => *i,
            [] => return Err(invalid(format!("no teammate {target}"))),
            _ => {
                return Err(invalid(format!(
                    "more than one {target} · use an actor name"
                )));
            }
        }
    };
    let mut member = roster.members[index].clone();
    let entries = status(&roster, &vault)?;
    let claims = entries[index]["claims"].as_array().unwrap();
    if member.task.is_none() {
        member.task = claims
            .first()
            .and_then(|n| n["id"].as_str())
            .map(str::to_string);
    }
    if cli.dry_run {
        out.json(&json!({"dry_run":true,"member":member,"claims":claims}));
        return Ok(());
    }
    if !claims.is_empty() {
        let ids = claims
            .iter()
            .filter_map(|n| n["id"].as_str())
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!(
            "{} is working on {ids} (doing) · claims will be kept",
            member.actor
        );
        if !cli.yes {
            if !std::io::stdin().is_terminal() || cli.json {
                return Err(invalid("remove anyway? · run in a terminal or pass --yes"));
            }
            eprint!("remove anyway? y/n ");
            std::io::stderr().flush()?;
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                return Err(invalid("team rm cancelled · nothing closed"));
            }
        }
    }
    if let Some(pane) = &member.pane {
        let panes = team_host::list(&roster.host, &roster.context, false)?;
        if let Some(current) = panes
            .iter()
            .find(|v| team_host::pane_id(v).as_ref() == Some(pane))
        {
            if member.host_identity.is_some()
                && member.host_identity != team_host::identity(current)
            {
                return Err(invalid(format!(
                    "pane {pane} has been replaced · not closed"
                )));
            }
            if roster.host == Host::Herdr {
                let agents = team_host::list(&roster.host, &roster.context, true)?;
                if agents.iter().any(|v| {
                    team_host::pane_id(v).as_ref() == Some(pane)
                        && v["name"].as_str() != Some(&member.actor)
                }) {
                    return Err(invalid(format!(
                        "pane {pane} now has another agent · not closed"
                    )));
                }
            }
            team_host::close(&roster.host, &roster.context, pane)?;
        }
    }
    roster.members.remove(index);
    save(&roster)?;
    let mut tasks: BTreeSet<String> = claims
        .iter()
        .filter_map(|n| n["id"].as_str().map(str::to_string))
        .collect();
    tasks.extend(member.task.iter().cloned());
    audit(
        &mut vault,
        &member,
        "removed",
        &tasks.into_iter().collect::<Vec<_>>(),
    )?;
    if cli.json {
        out.json(&json!({"ok":true,"removed":member.actor,"claims":claims}));
    } else {
        out.line(format!("removed {} · board and claims kept", member.actor));
    }
    Ok(())
}

pub fn agents(cli: &Cli, out: &mut Out) -> Result<()> {
    let host = team_host::detect();
    let settings = thc_core::settings::load(None);
    let configured = settings.get("team.agents").and_then(|v| v.as_table());
    let mut names: BTreeSet<String> = ["claude", "codex"].iter().map(|s| s.to_string()).collect();
    if let Some(t) = configured {
        names.extend(t.keys().cloned());
    }
    if host == Host::Herdr {
        names.extend(team_launch::HERDR_KINDS.iter().map(|s| s.to_string()));
    }
    let mut rows = Vec::new();
    for name in names {
        let source = if configured.is_some_and(|t| t.contains_key(&name)) {
            "config"
        } else if matches!(name.as_str(), "claude" | "codex") {
            "built-in"
        } else {
            "herdr"
        };
        let launcher = team_launch::load(&name, host == Host::Herdr);
        rows.push(match launcher {
            Ok(l) => json!({"name":name,"source":source,"launcher":l,"available":true}),
            Err(e) => json!({"name":name,"source":source,"available":false,"error":e.to_string()}),
        });
    }
    if cli.json {
        out.json(&json!({"agents":rows,"host":host}));
    } else {
        for row in rows {
            out.line(format!(
                "{} · {}{}",
                row["name"].as_str().unwrap(),
                row["source"].as_str().unwrap(),
                row["error"]
                    .as_str()
                    .map(|s| format!(" · {s}"))
                    .unwrap_or_default()
            ));
        }
    }
    Ok(())
}
