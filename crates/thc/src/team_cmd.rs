//! Project-local team lifecycle. Roster storage is shared with role prime and future team add.
use crate::{
    cli::Cli,
    out::Out,
    team_host::{self, Host},
    team_launch::{self, Launcher},
};
#[path = "team_profile.rs"]
mod profile;
#[path = "team_member.rs"]
mod team_member;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
};
use thc_core::{board::Board, error::invalid, vault::Vault};

#[derive(clap::Subcommand, Debug)]
pub enum TeamCmd {
    /// Start the configured roster, or one agent per listed role.
    Up {
        #[arg(num_args = 0..)]
        roles: Vec<String>,
        /// Agent for everyone, or ROLE=AGENT (repeatable).
        #[arg(long, action = clap::ArgAction::Append)]
        agent: Vec<String>,
        #[arg(long)]
        model: Option<String>,
        /// Explicitly create and configure a new project board.
        #[arg(long)]
        new_board: Option<String>,
    },
    /// Add one teammate to this project's running team.
    Add {
        role: String,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        model: Option<String>,
        /// Claude's permission mode; omit to keep the user's default.
        #[arg(long, value_parser = ["auto", "manual"])]
        permission_mode: Option<String>,
        /// First task; the new agent claims it itself with a guarded write.
        #[arg(long)]
        task: Option<String>,
    },
    /// Remove one teammate by actor, or by an unambiguous role; keeps claims.
    Rm {
        #[arg(value_name = "ACTOR")]
        member: String,
    },
    /// List available agent launchers and their source.
    Agents,
    /// List this project's remembered teammates and their live state/work.
    Ls,
    /// Close only thc's team panes, after confirmation (or --yes).
    Down,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Member {
    pub role: String,
    pub actor: String,
    pub agent: String,
    pub model: Option<String>,
    pub pane: Option<String>,
    pub added_by: String,
    pub added_at: String,
    pub state: String,
    pub task: Option<String>,
    #[serde(default)]
    pub host_identity: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Roster {
    pub project: PathBuf,
    pub board: Board,
    pub host: Host,
    pub context: BTreeMap<String, String>,
    pub members: Vec<Member>,
}

pub fn project_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?.canonicalize()?;
    let mut dir = cwd.clone();
    loop {
        if dir.join(".thc.toml").exists() || dir.join(".git").exists() || cache(&dir).exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Ok(cwd);
        }
    }
}
fn cache(root: &Path) -> PathBuf {
    thc_core::vault::default_cache(root).join("team.json")
}
pub fn load(root: &Path) -> Result<Option<Roster>> {
    let p = cache(root);
    thc_core::sandbox::check(&p);
    match std::fs::read(&p) {
        Ok(data) => {
            let roster: Roster = serde_json::from_slice(&data)?;
            if roster.project != root {
                return Err(invalid("team cache belongs to another project"));
            }
            Ok(Some(roster))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub fn save(roster: &Roster) -> Result<()> {
    let p = cache(&roster.project);
    thc_core::sandbox::check(&p);
    std::fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(roster)?)?;
    std::fs::rename(tmp, p)?;
    Ok(())
}
pub fn roster_json(vault: &Vault) -> Result<Value> {
    let Some(roster) = load(&project_root()?)? else {
        return Ok(json!([]));
    };
    if roster.board.path != vault.paths.vault {
        return Ok(json!([]));
    }
    Ok(json!(status(&roster, vault)?))
}

/// Serialize lifecycle changes across shells; don't wait behind another interactive prompt.
fn lock(root: &Path) -> Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    let p = cache(root).with_extension("lock");
    thc_core::sandbox::check(&p);
    std::fs::create_dir_all(p.parent().unwrap())?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(p)?;
    // SAFETY: a valid owned fd; closing it releases flock.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(invalid(
            "another team lifecycle command is running for this project",
        ));
    }
    Ok(file)
}

pub fn run(cli: &Cli, cmd: &TeamCmd) -> Result<()> {
    let mut out = Out::new(cli.json);
    let result = inner(cli, cmd, &mut out);
    out.flush();
    result
}
fn inner(cli: &Cli, cmd: &TeamCmd, out: &mut Out) -> Result<()> {
    let root = project_root()?;
    let readonly = matches!(cmd, TeamCmd::Ls | TeamCmd::Agents);
    if matches!(cmd, TeamCmd::Add { .. } | TeamCmd::Rm { .. }) {
        let mode = match cmd {
            TeamCmd::Add {
                permission_mode, ..
            } => permission_mode.as_deref(),
            _ => None,
        };
        team_member::authorize(cli, mode)?;
    } else if !readonly && !cli.dry_run {
        thc_core::policy::Policy::load(&crate::parse_actor(cli.actor.as_deref()), cli.readonly)?
            .check(&["team".into()], cli.yes)?;
    }
    let _lock = if !readonly && !cli.dry_run {
        Some(lock(&root)?)
    } else {
        None
    };
    match cmd {
        TeamCmd::Up {
            roles,
            agent,
            model,
            new_board,
        } => up(
            cli,
            &root,
            roles,
            agent,
            model.as_deref(),
            new_board.as_deref(),
            out,
        ),
        TeamCmd::Ls => {
            let Some(roster) = load(&root)? else {
                crate::team_board::resolve(cli)?;
                if cli.json {
                    out.json(&json!({"project":root,"team":[]}));
                } else {
                    out.line("no team for this project · thc team up pm designer engineer");
                }
                return Ok(());
            };
            let selection = thc_core::vault::Paths::resolve(Some(&roster.board.path))?;
            let vault = Vault::open(selection, crate::parse_actor(cli.actor.as_deref()), "cli")?;
            let entries = status(&roster, &vault)?;
            if cli.json {
                out.json(
                    &json!({"project":root,"board":roster.board,"host":roster.host,"team":entries}),
                );
            } else {
                for e in entries {
                    let claims = e["claims"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|n| {
                            format!(
                                "{} {}",
                                n["id"].as_str().unwrap_or(""),
                                n["text"].as_str().unwrap_or("")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    let last = e["last_activity"].as_str().unwrap_or("—");
                    let state = if e["state"] == "stopped" && !claims.is_empty() {
                        format!("idle since {last}")
                    } else {
                        e["state"].as_str().unwrap_or("").into()
                    };
                    let agent = match e["model"].as_str() {
                        Some(model) => format!("{} ({model})", e["agent"].as_str().unwrap_or("")),
                        None => e["agent"].as_str().unwrap_or("").into(),
                    };
                    out.line(format!(
                        "{} · {} · {agent} · pane {} · {state}",
                        e["role"].as_str().unwrap_or(""),
                        e["actor"].as_str().unwrap_or(""),
                        e["pane"].as_str().unwrap_or("—")
                    ));
                    out.line(format!(
                        "  added by {} · {} · unread {} · last activity {last}",
                        e["added_by"].as_str().unwrap_or(""),
                        if claims.is_empty() {
                            "no claim"
                        } else {
                            &claims
                        },
                        e["unread"]
                    ));
                }
            }
            Ok(())
        }
        TeamCmd::Down => down(cli, &root, out),
        TeamCmd::Add {
            role,
            agent,
            model,
            permission_mode,
            task,
        } => team_member::add(
            cli,
            &root,
            role,
            agent,
            model.as_deref(),
            permission_mode.as_deref(),
            task.as_deref(),
            out,
        ),
        TeamCmd::Rm { member } => team_member::remove(cli, &root, member, out),
        TeamCmd::Agents => team_member::agents(cli, out),
    }
}

fn members(
    roles: &[String],
    choices: &[String],
    model: Option<&str>,
    host: &Host,
    actor: &str,
) -> Result<Vec<(Member, Launcher)>> {
    let mut default = "claude".to_string();
    let mut overrides = BTreeMap::new();
    for choice in choices {
        if let Some((r, a)) = choice.split_once('=') {
            if !roles.iter().any(|role| role == r) {
                return Err(invalid(format!("agent override names missing role {r}")));
            }
            overrides.insert(r.to_string(), a.to_string());
        } else {
            default = choice.clone();
        }
    }
    let max = profile::max_agents();
    if roles.len() > max {
        return Err(invalid(format!("team exceeds max_agents ({max})")));
    }
    let mut names = std::collections::BTreeSet::new();
    let mut output = Vec::new();
    for role in roles {
        crate::instructions::role_name(role)?;
        let agent = overrides.get(role).unwrap_or(&default);
        let launcher = team_launch::load(agent, *host == Host::Herdr)?;
        let base = format!("{agent}-{role}");
        let mut name = base.clone();
        let mut suffix = 2;
        while names.contains(&name) {
            name = format!("{base}-{suffix}");
            suffix += 1;
        }
        if name.len() > 32 {
            return Err(invalid("team actor name exceeds 32 characters"));
        }
        names.insert(name.clone());
        if let Some(model) = model {
            team_launch::safe(agent, model)?;
        }
        output.push((
            Member {
                role: role.clone(),
                actor: name,
                agent: agent.clone(),
                model: model.map(str::to_string),
                pane: None,
                added_by: actor.into(),
                added_at: thc_core::dates::now_stamp(),
                state: "planned".into(),
                task: None,
                host_identity: None,
            },
            launcher,
        ));
    }
    Ok(output)
}

fn up(
    cli: &Cli,
    root: &Path,
    roles: &[String],
    agents: &[String],
    model: Option<&str>,
    new_board: Option<&str>,
    out: &mut Out,
) -> Result<()> {
    if load(root)?.is_some_and(|r| r.members.iter().any(|m| m.pane.is_some())) {
        return Err(invalid(
            "this project already has team panes · thc team ls or thc team down first",
        ));
    }
    let profile = profile::load(root)?;
    // Explicit roles retain their existing lead-left layout unless a layout is configured.
    let roles: Vec<_> = roles
        .iter()
        .filter(|r| matches!(r.as_str(), "pm" | "lead"))
        .chain(
            roles
                .iter()
                .filter(|r| !matches!(r.as_str(), "pm" | "lead")),
        )
        .cloned()
        .collect();
    let host = team_host::detect();
    let actor = crate::parse_actor(cli.actor.as_deref());
    let who = actor.name.as_deref().unwrap_or(&actor.kind);
    let mut modes = BTreeMap::new();
    let planned = if roles.is_empty() {
        let mut planned = Vec::new();
        let mut names = std::collections::BTreeSet::new();
        for entry in profile::entries(&profile, agents, model)? {
            if entry.permission_mode.is_some() && actor.kind == "agent" {
                return Err(invalid(
                    "agents cannot choose a permission mode · run this profile yourself, or omit permission_mode",
                ));
            }
            for _ in 0..entry.count {
                let (mut member, launcher) = members(
                    &[entry.role.clone()],
                    &[entry.agent.clone()],
                    entry.model.as_deref(),
                    &host,
                    who,
                )?
                .remove(0);
                let base = member.actor.clone();
                let mut suffix = 2;
                while names.contains(&member.actor) {
                    member.actor = format!("{base}-{suffix}");
                    suffix += 1;
                }
                if member.actor.len() > 32 {
                    return Err(invalid("team actor name exceeds 32 characters"));
                }
                names.insert(member.actor.clone());
                modes.insert(member.actor.clone(), entry.permission_mode.clone());
                planned.push((member, launcher));
            }
        }
        planned
    } else {
        members(&roles, agents, model, &host, who)?
    };
    // Validate all native options before preparing a board or opening a pane.
    for (m, launcher) in &planned {
        team_launch::options(
            &m.agent,
            launcher,
            m.model.as_deref(),
            modes.get(&m.actor).and_then(|m| m.as_deref()),
        )?;
        if host == Host::Herdr && !team_launch::HERDR_KINDS.contains(&m.agent.as_str()) {
            return Err(invalid(format!(
                "herdr has no agent kind {} · thc team agents",
                m.agent
            )));
        }
    }
    let total = planned.len();
    let layout = profile
        .layout
        .or_else(|| roles.is_empty().then(profile::Layout::default));
    let plain: Vec<_> = planned.iter().map(|(m, _)| m.clone()).collect();
    let context = team_host::context(&host);
    if host == Host::Herdr {
        let live = team_host::list(&host, &context, true)?;
        for m in &plain {
            if live
                .iter()
                .any(|v| v.get("name").and_then(Value::as_str) == Some(&m.actor))
            {
                return Err(invalid(format!(
                    "herdr already has agent {} · choose another role or stop it explicitly",
                    m.actor
                )));
            }
        }
    }
    let Some((vault, board, fresh)) =
        crate::team_board::prepare(cli, root, &plain, new_board, out)?
    else {
        return Ok(());
    };
    let board_env = board
        .page
        .as_ref()
        .map(|p| format!("{}:¶ {}", board.path.display(), p.title))
        .unwrap_or_else(|| board.path.display().to_string());
    if !cli.json {
        if let Some(layout) = &layout {
            out.line(format!(
                "layout · {} · {} columns{}",
                layout.tab,
                layout.columns.min(total),
                if layout.pm.is_some() {
                    " · pm bottom-right"
                } else {
                    ""
                }
            ));
        }
        out.line(format!(
            "team up · board {}{} · {}",
            board.vault,
            board
                .page
                .as_ref()
                .map(|p| format!(" · ¶ {}", p.title))
                .unwrap_or_default(),
            plain
                .iter()
                .map(|m| format!("{} ({})", m.role, m.agent))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        out.flush();
    }
    let mut roster = Roster {
        project: root.to_path_buf(),
        board,
        host: host.clone(),
        context,
        members: plain.clone(),
    };
    let mut commands = Vec::new();
    let mut panes = Vec::new();
    let cells = layout.as_ref().map(|layout| layout.cells(&plain));
    let order: Vec<usize> = if cli.dry_run || host == Host::Printed {
        (0..total).collect()
    } else {
        cells
            .as_ref()
            .map(|cells| cells.iter().map(|cell| cell.member).collect())
            .unwrap_or_else(|| (0..total).collect())
    };
    let mut created = BTreeMap::new();
    for index in order {
        let (mut m, launcher) = planned[index].clone();
        let mut env = launcher.env.clone();
        env.insert("THC_ACTOR".into(), m.actor.clone());
        env.insert("THC_ROLE".into(), m.role.clone());
        env.insert("THC_BOARD".into(), board_env.clone());
        // Never inherit the parent's session id as the child's identity.
        env.insert("THC_SESSION_ID".into(), String::new());
        let mut prompt = format!(
            "You're the {} on this project's team. Run thc prime --role {} and follow it. Coordinate only through the board.",
            m.role, m.role
        );
        if fresh && matches!(m.role.as_str(), "pm" | "lead") {
            prompt.push_str(" Read the README and folder, refine About, write the first ordered tasks with roles/priorities/dependencies, then complete First plan and ask the human to accept it before routing work.");
        }
        let options = team_launch::options(
            &m.agent,
            &launcher,
            m.model.as_deref(),
            modes.get(&m.actor).and_then(|m| m.as_deref()),
        )?;
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
        commands.push(json!({"role":m.role,"actor":m.actor,"command":command}));
        if cli.dry_run {
            if !cli.json {
                out.line(format!("# {}\n{command}", m.role));
            }
            roster.members[index] = m;
            continue;
        }
        if host == Host::Printed {
            m.state = "printed".into();
            roster.members[index] = m.clone();
            save(&roster)?;
            if !cli.json {
                out.line(format!("# {}\n{command}", m.role));
            }
            continue;
        }
        let (pane, identity) = if let (Some(layout), Some(cells)) = (&layout, &cells) {
            let cell = cells.iter().find(|cell| cell.member == index).unwrap();
            let parent = cell
                .parent
                .map(|member| {
                    created
                        .get(&member)
                        .map(String::as_str)
                        .ok_or_else(|| invalid("team layout parent was not created"))
                })
                .transpose()?;
            team_host::create_cell(
                &host,
                root,
                &roster.context,
                &env,
                &argv,
                parent,
                cell.direction,
                cell.remaining,
                &layout.tab,
            )?
        } else {
            team_host::create(&host, root, &env, &argv, &panes, total)?
        };
        m.host_identity = identity;
        m.pane = Some(pane.clone());
        m.state = "starting".into();
        roster.members[index] = m.clone();
        // Persist immediately after creation: failures in naming/start/prompt remain recoverable.
        save(&roster)?;
        panes.push(pane.clone());
        created.insert(index, pane.clone());
        if host == Host::Herdr {
            team_host::call(
                &host,
                &roster.context,
                &["pane".into(), "rename".into(), pane.clone(), m.role.clone()],
            )?;
            let mut start = vec![
                "agent".into(),
                "start".into(),
                m.actor.clone(),
                "--kind".into(),
                m.agent.clone(),
                "--pane".into(),
                pane,
            ];
            if !options.is_empty() {
                start.push("--".into());
                start.extend(options);
            }
            team_host::call(&host, &roster.context, &start)?;
            team_host::call(
                &host,
                &roster.context,
                &["agent".into(), "prompt".into(), m.actor.clone(), prompt],
            )?;
        } else if panes.len() == 1 {
            team_host::call(
                &host,
                &roster.context,
                &[
                    "cli".into(),
                    "set-tab-title".into(),
                    "--pane-id".into(),
                    pane,
                    layout
                        .as_ref()
                        .map(|l| l.tab.clone())
                        .unwrap_or_else(|| "team".into()),
                ],
            )?;
        }
        roster.members[index].state = "running".into();
        save(&roster)?;
    }
    let _ = vault;
    if cli.json {
        out.json(&json!({"ok":true,"dry_run":cli.dry_run,"board":roster.board,"host":host,"team":roster.members,"commands":commands,"model_passed":true,"layout":layout}));
    }
    Ok(())
}

pub fn status(roster: &Roster, vault: &Vault) -> Result<Vec<Value>> {
    let live = team_host::list(&roster.host, &roster.context, roster.host == Host::Herdr).ok();
    let q = roster
        .board
        .page
        .as_ref()
        .map(|p| format!("is:task status:doing under:{}", p.id))
        .unwrap_or_else(|| "is:task status:doing".into());
    let tasks = vault.store.query(&q, thc_core::dates::today(), 100_000)?;
    let mut output = Vec::new();
    for m in &roster.members {
        let mut v = serde_json::to_value(m)?;
        let found = live.as_ref().and_then(|rows| {
            rows.iter().find(|r| {
                r.get("pane_id").map(|p| {
                    p.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| p.to_string())
                }) == m.pane
            })
        });
        v["state"] = json!(if m.pane.is_none() {
            m.state.clone()
        } else if let Some(r) = found {
            if roster.host == Host::Herdr && r.get("name").and_then(Value::as_str) != Some(&m.actor)
            {
                "replaced".into()
            } else {
                r.get("agent_status")
                    .or_else(|| r.get("state"))
                    .or_else(|| r.get("status"))
                    .and_then(Value::as_str)
                    .unwrap_or("running")
                    .into()
            }
        } else if live.is_some() {
            "stopped".into()
        } else {
            "unavailable".into()
        });
        let claims: Vec<_> = tasks
            .iter()
            .filter(|n| {
                vault
                    .store
                    .props_of(&n.id)
                    .ok()
                    .is_some_and(|p| p.get("owner").and_then(Value::as_str) == Some(&m.actor))
            })
            .map(|n| json!({"id":n.id,"text":n.text}))
            .collect();
        v["task"] = claims
            .first()
            .map(|n| n["id"].clone())
            .unwrap_or(Value::Null);
        v["claims"] = json!(claims);
        let last: Option<i64> = vault.store.conn.query_row(
            "SELECT max(ms) FROM events WHERE actor=?1",
            [format!("agent:{}", m.actor)],
            |r| r.get(0),
        )?;
        v["last_activity"] = json!(last.map(crate::out::ms_to_local));
        let recipient = thc_core::messages::Recipient::new(Some(&m.role), Some(&m.actor));
        let unread = thc_core::messages::list(&vault.store, &recipient, true, 100_000)?.len();
        v["unread"] = json!(unread);
        output.push(v);
    }
    Ok(output)
}

fn down(cli: &Cli, root: &Path, out: &mut Out) -> Result<()> {
    let Some(mut roster) = load(root)? else {
        crate::team_board::resolve(cli)?;
        if cli.json {
            out.json(&json!({"ok":true,"closed":0}));
        } else {
            out.line("no team panes to close");
        }
        return Ok(());
    };
    let panes = team_host::list(&roster.host, &roster.context, false)?;
    if cli.dry_run {
        out.json(&json!({"dry_run":true,"team":roster.members}));
        return Ok(());
    }
    if !cli.yes {
        if !std::io::stdin().is_terminal() || cli.json {
            return Err(invalid(
                "closing team panes needs confirmation · run in a terminal or pass --yes",
            ));
        }
        eprint!(
            "close {} team panes? y/n ",
            roster.members.iter().filter(|m| m.pane.is_some()).count()
        );
        std::io::stderr().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            return Err(invalid("team down cancelled · nothing closed"));
        }
    }
    let agents = if roster.host == Host::Herdr {
        team_host::list(&roster.host, &roster.context, true)?
    } else {
        Vec::new()
    };
    // Validate every live target before closing any of them.
    for m in &roster.members {
        let Some(pane) = m.pane.as_deref() else {
            continue;
        };
        if agents.iter().any(|a| {
            a.get("pane_id").and_then(Value::as_str) == Some(pane)
                && a.get("name").and_then(Value::as_str) != Some(&m.actor)
        }) {
            return Err(invalid(format!(
                "pane {pane} now has another agent · not closed"
            )));
        }
        if let Some(current) = panes
            .iter()
            .find(|p| team_host::pane_id(p).as_deref() == Some(pane))
        {
            if m.host_identity.is_some() && m.host_identity != team_host::identity(current) {
                return Err(invalid(format!(
                    "pane {pane} has been replaced · not closed"
                )));
            }
        }
    }
    let mut closed = 0;
    for i in 0..roster.members.len() {
        let Some(pane) = roster.members[i].pane.clone() else {
            continue;
        };
        let exists = panes.iter().any(|p| {
            p.get("pane_id").map(|p| {
                p.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| p.to_string())
            }) == Some(pane.clone())
        });
        if exists {
            team_host::close(&roster.host, &roster.context, &pane)?;
            closed += 1;
        }
        roster.members[i].pane = None;
        roster.members[i].state = "stopped".into();
        save(&roster)?;
    }
    if cli.json {
        out.json(&json!({"ok":true,"closed":closed}));
    } else {
        out.line(format!(
            "closed {closed} team panes · board and claims kept"
        ));
        for m in roster.members {
            out.line(format!("  {} · stopped", m.actor));
        }
    }
    Ok(())
}
