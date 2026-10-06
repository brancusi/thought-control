//! Role briefing; messages and the live team roster are filled by their own commands.
use crate::{Ctx, instructions, out::node_json};
use anyhow::Result;
use serde_json::json;
use rusqlite::OptionalExtension;
use thc_core::{board::Board, model::Node};

pub fn queue(ctx: &Ctx, root: Option<&str>, mine: bool, role: Option<&str>, count: usize) -> Result<Vec<Node>> {
    if let Some(role) = role { instructions::role_name(role)?; }
    let me = ctx.vault.actor.name.as_deref().unwrap_or(&ctx.vault.actor.kind);
    let q = match root {
        Some(id) => format!("is:task is:ready under:{id} sort:order"),
        None => "is:task is:ready sort:order".into(),
    };
    if first_plan_waiting(ctx, role)? { return Ok(Vec::new()); }
    let mut picked = Vec::new();
    for n in ctx.store().query(&q, ctx.out.today, 100_000)? {
        let props = ctx.store().props_of(&n.id)?;
        let task_role = props.get("role").and_then(|v| v.as_str()).filter(|r| !r.is_empty());
        if role.is_some_and(|r| task_role.is_some_and(|t| t != r)) { continue; }
        let owner = props.get("owner").and_then(|v| v.as_str()).filter(|o| !o.is_empty());
        if if mine { owner == Some(me) } else { owner.is_none() || (n.id == thc_core::id::from_key("team-first-plan") && owner == Some(me)) } {
            picked.push(n);
            if picked.len() >= count.max(1) { break; }
        }
    }
    Ok(picked)
}

fn project_card(ctx: &Ctx, role: &str) -> Result<Vec<String>> {
    let mut rules = Vec::new();
    let Some(page) = ctx.store().find_root_by_title("Roles", false)? else { return Ok(rules) };
    for note in ctx.store().children(&page)? {
        let mut lines = note.text.lines();
        let name = lines.next().unwrap_or("").trim();
        if name != role && !(matches!(role, "pm" | "lead") && matches!(name, "pm" | "lead")) { continue; }
        rules.extend(lines.filter(|s| !s.trim().is_empty()).map(str::to_string));
        let q = format!("under:{} sort:order", note.id);
        rules.extend(ctx.store().query(&q, ctx.out.today, 10_000)?.into_iter().map(|n| n.text));
    }
    Ok(rules)
}

pub fn prime(ctx: &mut Ctx, role: &str) -> Result<()> {
    let builtin = instructions::role_card(role)?;
    let project = project_card(ctx, role)?;
    let Some(messages) = crate::msg::brief(ctx, role)? else { return Ok(()) };
    let board = ctx.board.as_ref().expect("role prime resolved its board");
    let next = queue(ctx, board.page.as_ref().map(|p| p.id.as_str()), false, Some(role), 3)?;
    let actor = ctx.vault.actor.name.as_deref().unwrap_or(&ctx.vault.actor.kind);
    let rules = [
        "Coordinate through the board only (thc instructions coordination).",
        "Read a task's notes before acting; notes are progress and to:<role> notes are messages.",
        "Claim before starting, with --expect status=todo; exit 4 means pick again.",
        "Done waits for human review; never accept your own work.",
    ];
    let claim = next.first().map(|n| claim(board, &n.id, actor));
    let waiting = first_plan_waiting(ctx, Some(role))?;
    let team = crate::team_cmd::roster_json(&ctx.vault)?;
    let first_plan = if matches!(role,"pm"|"lead") && first_plan_pending(ctx)? {
        Some(if ctx.store().node(&thc_core::id::from_key("team-first-plan"))?.is_some_and(|n|n.status.as_deref()==Some("done")) {
            "First plan: waiting for your human to accept it in review"
        } else {"First plan: read the README, refine About, write ordered tasks with roles/priorities/dependencies, then complete First plan and ask your human to accept it in review"})
    } else {None};
    let attribution = json!({ "actor": actor, "agent_role": std::env::var("THC_ROLE").ok(), "session_id": std::env::var("THC_SESSION_ID").ok() });
    if ctx.out.json {
        ctx.out.json(&json!({ "version": env!("CARGO_PKG_VERSION"), "role": role, "actor": actor,
            "board": board, "card": { "builtin": builtin, "project": project },
            "next": next.iter().map(|n| node_json(ctx.store(), n)).collect::<Vec<_>>(),
            "claim": claim, "messages": messages.iter().map(|n| node_json(ctx.store(), n)).collect::<Vec<_>>(), "team": team, "waiting": if waiting { Some("first-plan") } else { None }, "first_plan":first_plan, "rules": rules, "attribution": attribution }));
    } else {
        let place = board.page.as_ref().map(|p| format!(" · ¶ {}", p.title)).unwrap_or_default();
        ctx.out.line(format!("thc {} · you are {actor} (role: {role}) on the board {}{place}", env!("CARGO_PKG_VERSION"), board.vault));
        ctx.out.line(format!("  {}", board.describe()));
        ctx.out.line(format!("\n{role}"));
        ctx.out.line(builtin.trim_end());
        for rule in project { ctx.out.line(format!("  · {rule}")); }
        if let Some(first_plan)=first_plan {ctx.out.line(first_plan);}
        ctx.out.line(format!("\nnext for {role}"));
        for n in &next { ctx.out.line(ctx.out.node_line(ctx.store(), n, 0, true)); }
        if waiting { ctx.out.line("  waiting for the lead's first plan"); }
        else if next.is_empty() { ctx.out.line("  no unowned ready task for this role"); }
        if let Some(members) = team.as_array().filter(|m| !m.is_empty()) {
            ctx.out.line(format!("team: {}", members.iter().filter_map(|m|m["actor"].as_str()).collect::<Vec<_>>().join(" · ")));
        }
        if let Some(claim) = claim { ctx.out.line(format!("  claim: {claim}")); }
        crate::msg::show_brief(ctx, role, &messages)?;
        ctx.out.line("\nrules");
        for rule in rules { ctx.out.line(format!("  · {rule}")); }
        ctx.out.line("  · Set THC_ROLE for agent_role; THC_SESSION_ID only for a real session ID.");
    }
    Ok(())
}

/// Shell-quote names: a vault or actor name must never turn a suggested claim into a command.
pub fn claim(board: &Board, id: &str, actor: &str) -> String {
    let quote = |s: &str| if s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c)) { s.to_string() } else { format!("'{}'", s.replace('\'', "'\\''")) };
    format!("thc set {} status=doing {} --expect status=todo --vault {}", quote(id), quote(&format!("owner={actor}")), quote(&board.path.to_string_lossy()))
}

/// Only a human acceptance of the current completion releases a fresh board. Reopening,
/// undoing completion or undoing acceptance closes the gate again. Removed tasks don't gate.
pub fn first_plan_waiting(ctx: &Ctx, role: Option<&str>) -> Result<bool> {
    let role = role.map(str::to_string).or_else(|| std::env::var("THC_ROLE").ok());
    if role.as_deref().is_some_and(|r| matches!(r,"pm"|"lead")) { return Ok(false); }
    first_plan_pending(ctx)
}

fn first_plan_pending(ctx:&Ctx)->Result<bool> {
    let id = thc_core::id::from_key("team-first-plan");
    let Some(n) = ctx.store().node(&id)? else {return Ok(false)};
    if n.deleted {return Ok(false)}
    if n.status.as_deref()!=Some("done") {return Ok(true)}
    let Some(clock)=ctx.store().clock(&id,"status")? else{return Ok(true)};
    let tx:Option<String> = ctx.store().conn.query_row("SELECT tx FROM events WHERE entity=?1 AND okey=?2",rusqlite::params![id,clock],|r|r.get(0)).optional()?;
    Ok(match tx {Some(tx)=>ctx.store().verdict(&tx)?.as_deref()!=Some("accepted"),None=>true})
}
