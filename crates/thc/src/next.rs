//! `thc next` and `thc board` (team.md §1): the queue on the project's work board.

use crate::{Ctx, out::node_json, team_roles};
use anyhow::Result;
use serde_json::{Value, json};
use thc_core::error::ThcError;

/// The next task to pick up: the first ready (open, unblocked, scheduled reached) and
/// unowned task in queue order (the outline order) on the work board.
#[derive(clap::Args, Debug)]
pub struct NextArgs {
    /// Only tasks owned by you (THC_ACTOR) instead of unowned ones.
    #[arg(long)]
    mine: bool,
    /// Tasks routed to this role, or tasks with no role.
    #[arg(long)]
    role: Option<String>,
    /// The queue's page or note, instead of the board's page.
    #[arg(long)]
    under: Option<String>,
    /// How many to list, in order (default 1).
    #[arg(short = 'n', long, default_value_t = 1)]
    count: usize,
}

pub const SPEC: crate::registry::Spec = crate::spec!("next", "The next task to pick up: the first ready, unowned task in queue order on the work board", NextArgs, run, board: true, settings: true);

/// `thc board`: no arguments.
#[derive(clap::Args, Debug)]
pub struct BoardArgs {}

pub const BOARD: crate::registry::Spec = crate::spec!("board", "The resolved project work board and why it was chosen", BoardArgs, board, board: true, settings: true);

fn board(ctx: &mut Ctx, _: BoardArgs) -> Result<()> {
    let board = ctx.board.as_ref().expect("a board command resolved its board");
    if ctx.out.json {
        ctx.out.json(&serde_json::to_value(board)?);
    } else {
        let page = board.page.as_ref().map(|p| format!(" · ¶ {}", p.title)).unwrap_or_default();
        ctx.out.line(format!("{}{page} · {}", board.vault, board.describe()));
    }
    Ok(())
}

/// `thc next`: the queue is the capture target's outline (or `--under`, or the whole
/// vault), in outline order; the first ready task nobody owns (`--mine`: that you own) is next.
fn run(ctx: &mut Ctx, a: NextArgs) -> Result<()> {
    let NextArgs { mine, role, under, count } = a;
    let (role, under) = (role.as_deref(), under.as_deref());
    let (root, title) = match under {
        Some(u) => {
            let id = if u.trim().starts_with('¶') {
                ctx.store().find_root_by_title(u.trim().trim_start_matches('¶').trim(), false)?.ok_or_else(|| thc_core::error::not_found(format!("no page {u}")))?
            } else {
                ctx.resolve(u).or_else(|e| ctx.store().find_root_by_title(u, false)?.ok_or(e))?
            };
            let t = ctx.store().node(&id)?.map(|n| n.title.clone().unwrap_or_else(|| n.text.clone())).unwrap_or_default();
            (Some(id), Some(t))
        }
        None => {
            let board = ctx.board.as_ref().unwrap();
            (board.page.as_ref().map(|p| p.id.clone()), board.page.as_ref().map(|p| p.title.clone()))
        },
    };
    let me = ctx.vault.actor.name.clone().unwrap_or_else(|| ctx.vault.actor.kind.clone());
    if team_roles::first_plan_waiting(ctx, role)? {
        if ctx.out.json { ctx.out.json(&json!({"waiting":"first-plan","queue":{"id":root,"title":title},"count":0,"items":[],"claim":null})); }
        else { ctx.out.line("waiting for the lead's first plan"); }
        return Ok(());
    }
    let picked = team_roles::queue(ctx, root.as_deref(), mine, role, count)?;
    let queue = title.as_deref().map_or("the vault".to_string(), |t| format!("¶ {t}"));
    if picked.is_empty() {
        let what = if mine { format!("ready task owned by {me}") } else { "unowned ready task".to_string() };
        return Err(ThcError::NotFound(format!("no {what} in {queue}")).into());
    }
    if ctx.out.json {
        let items: Vec<Value> = picked.iter().map(|n| node_json(ctx.store(), n)).collect();
        let claim = team_roles::claim(ctx.board.as_ref().unwrap(), &picked[0].id, &me);
        ctx.out.json(&json!({ "queue": { "id": root, "title": title }, "count": items.len(), "items": items, "claim": claim }));
    } else {
        for n in &picked {
            let l = ctx.out.node_line(ctx.store(), n, 0, true);
            ctx.out.line(l);
        }
        let id = ctx.store().short(&picked[0].id);
        ctx.out.line(format!("  from {queue} · claim: {}", team_roles::claim(ctx.board.as_ref().unwrap(), &id, &me)));
    }
    Ok(())
}

