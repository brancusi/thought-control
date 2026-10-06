//! A node's lifecycle by id: done, reopen, skip, rm, restore.

use crate::Ctx;
use anyhow::Result;
use serde_json::json;
use thc_core::error::usage;

#[derive(clap::Args, Debug)]
pub struct Ids {
    ids: Vec<String>,
}

#[derive(clap::Args, Debug)]
pub struct One {
    id: String,
}

pub const DONE: crate::registry::Spec = crate::spec!("done", "Complete nodes (repeating nodes advance to their next date)", Ids, done, ids: true, verbs: |_| vec!["done".into()]);
pub const REOPEN: crate::registry::Spec = crate::spec!("reopen", "Reopen completed nodes", Ids, reopen, ids: true, verbs: |_| vec!["reopen".into()]);
pub const SKIP: crate::registry::Spec = crate::spec!("skip", "Skip the current occurrence of a repeating node", One, skip, ids: true, verbs: |_| vec!["skip".into()]);
pub const RM: crate::registry::Spec = crate::spec!("rm", "Delete nodes (and their children). Reversible with `thc restore` or `thc undo`", Ids, rm, ids: true, verbs: |_| vec!["rm".into(), "delete".into()]);
pub const RESTORE: crate::registry::Spec = crate::spec!("restore", "Restore deleted nodes", Ids, restore, ids: true, verbs: |_| vec!["restore".into()]);

fn resolve(ctx: &Ctx, ids: &[String]) -> Result<Vec<String>> {
    ids.iter().map(|i| ctx.resolve(i)).collect()
}

fn done(ctx: &mut Ctx, a: Ids) -> Result<()> {
    let ids = resolve(ctx, &a.ids)?;
    if ids.is_empty() {
        return Err(usage("give at least one id"));
    }
    let r = ctx.write(|b| {
        for id in &ids {
            b.complete(id)?;
        }
        Ok(())
    })?;
    if let Some((ev, ())) = r {
        ctx.report_write("done", &ev, &ids)?;
    }
    Ok(())
}

fn reopen(ctx: &mut Ctx, a: Ids) -> Result<()> {
    let ids = resolve(ctx, &a.ids)?;
    let r = ctx.write(|b| {
        for id in &ids {
            b.set_props(id, &[("status".into(), "todo".into()), ("done_at".into(), String::new())])?;
        }
        Ok(())
    })?;
    if let Some((ev, ())) = r {
        ctx.report_write("reopened", &ev, &ids)?;
    }
    Ok(())
}

fn skip(ctx: &mut Ctx, a: One) -> Result<()> {
    let id = ctx.resolve(&a.id)?;
    let r = ctx.write(|b| b.skip(&id))?;
    if let Some((ev, _)) = r {
        ctx.report_write("skipped", &ev, &[id])?;
    }
    Ok(())
}

fn rm(ctx: &mut Ctx, a: Ids) -> Result<()> {
    let ids = resolve(ctx, &a.ids)?;
    let r = ctx.write(|b| {
        let mut n = 0;
        for id in &ids {
            n += b.delete(id)?;
        }
        Ok(n)
    })?;
    if let Some((ev, n)) = r {
        if ctx.out.json {
            ctx.out.json(&json!({ "ok": true, "tx": ev.first().map(|e| &e.tx), "deleted": n }));
        } else {
            ctx.out.line(format!("deleted {n} node(s); `thc undo` to restore"));
        }
    }
    Ok(())
}

fn restore(ctx: &mut Ctx, a: Ids) -> Result<()> {
    let ids = resolve(ctx, &a.ids)?;
    let r = ctx.write(|b| {
        for id in &ids {
            b.restore(id)?;
        }
        Ok(())
    })?;
    if let Some((ev, ())) = r {
        ctx.report_write("restored", &ev, &ids)?;
    }
    Ok(())
}
