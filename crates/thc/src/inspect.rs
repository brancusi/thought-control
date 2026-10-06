//! Registered node inspection commands, migrated without changing their behavior.

use crate::{Ctx, out, review_cmd, summarize};
use anyhow::Result;
use serde_json::json;

pub const SHOW: crate::registry::Spec = crate::spec!("show", "Show a node with its children, properties, backlinks and alerts (default: today's journal)", ShowArgs, show, ids: true);
pub const HISTORY: crate::registry::Spec =
    crate::spec!("history", "Change history of a node", HistoryArgs, history, ids: true);
pub const DIFF: crate::registry::Spec = crate::spec!("diff", "Field-level changes to a node since a time or transaction", DiffArgs, diff, ids: true);

/// Show a node with its children, properties, backlinks and alerts (default: today's journal).
#[derive(clap::Args, Debug)]
pub struct ShowArgs {
    id: Option<String>,
    #[arg(long, default_value_t = 3)]
    depth: usize,
}

fn show(ctx: &mut Ctx, a: ShowArgs) -> Result<()> {
    let ShowArgs { id, depth } = a;
    crate::show(ctx, id, depth)?;
    Ok(())
}

/// Change history of a node.
#[derive(clap::Args, Debug)]
pub struct HistoryArgs {
    id: String,
}

fn history(ctx: &mut Ctx, a: HistoryArgs) -> Result<()> {
    let HistoryArgs { id } = a;
    let id = ctx.resolve(&id)?;
    let h = ctx.store().history(&id, ctx.limit)?;
    if ctx.out.json {
        ctx.out.json(&json!({ "id": id, "events": h }));
    } else {
        for e in h.iter().rev() {
            let when = out::ms_to_local(e.ms).replace('T', " ");
            let line = format!(
                "{}  {:<14} {:<14} {}",
                ctx.out.dim(&when),
                e.op,
                e.actor,
                summarize(&e.body)
            );
            ctx.out.line(line);
        }
    }
    Ok(())
}

/// Field-level changes to a node since a time or transaction.
#[derive(clap::Args, Debug)]
pub struct DiffArgs {
    id: String,
    /// e.g. 1d, 3h (default: 1d)
    #[arg(long, conflicts_with_all = ["tx", "as_of"], allow_hyphen_values = true)]
    since: Option<String>,
    /// Only this transaction's changes to the node.
    #[arg(long, conflicts_with = "as_of")]
    tx: Option<String>,
    /// Compare with how it was at this time.
    #[arg(long, value_name = "TIME", allow_hyphen_values = true)]
    as_of: Option<String>,
}

fn diff(ctx: &mut Ctx, a: DiffArgs) -> Result<()> {
    let DiffArgs {
        id,
        since,
        tx,
        as_of,
    } = a;
    review_cmd::diff(ctx, &id, since, tx, as_of)?;
    Ok(())
}
