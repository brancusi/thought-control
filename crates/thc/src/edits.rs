//! Changing a node by id: set, text, tag, link, unlink.

use crate::{Ctx, joined};
use anyhow::Result;
use serde_json::json;
use thc_core::error::{ThcError, usage};

pub const SET: crate::registry::Spec = crate::spec!("set", "Set properties: thc set <id> due=fri priority=high client=acme (empty value unsets)", SetArgs, set, ids: true, verbs: |_| vec!["set".into()]);
pub const TEXT: crate::registry::Spec = crate::spec!("text", "Replace a node's text", TextArgs, text, ids: true, verbs: |_| vec!["text".into()]);
pub const TAG: crate::registry::Spec = crate::spec!("tag", "Add or remove tags: thc tag <id> work +urgent -someday", TagArgs, tag, ids: true, verbs: |_| vec!["tag".into()]);
pub const LINK: crate::registry::Spec = crate::spec!("link", "Link two nodes: thc link <a> <b> --rel blocks  (a blocks b). Rels: blocks, relates, or your own", LinkArgs, link, ids: true, verbs: |_| vec!["link".into()]);
pub const UNLINK: crate::registry::Spec = crate::spec!("unlink", "Remove a link (all relations between a and b unless --rel is given)", UnlinkArgs, unlink, ids: true, verbs: |_| vec!["unlink".into()]);

#[derive(clap::Args, Debug)]
pub struct SetArgs {
    id: String,
    #[arg(required = true, value_name = "KEY=VALUE")]
    pairs: Vec<String>,
}

fn set(ctx: &mut Ctx, a: SetArgs) -> Result<()> {
    let SetArgs { id, pairs } = a;
    let id = ctx.resolve(&id)?;
    let mut kv = Vec::new();
    for p in &pairs {
        let (k, v) = p.split_once('=').ok_or_else(|| usage(format!("expected KEY=VALUE, got {p:?}")))?;
        kv.push((k.trim().to_lowercase(), v.to_string()));
    }
    let r = ctx.write(|b| b.set_props(&id, &kv))?;
    if let Some((ev, ())) = r {
        ctx.report_write("updated", &ev, &[id])?;
    }
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct TextArgs {
    id: String,
    text: Vec<String>,
    /// Overwrite even though the node has an open sync conflict (otherwise exit 4).
    #[arg(long)]
    force: bool,
}

fn text(ctx: &mut Ctx, a: TextArgs) -> Result<()> {
    let TextArgs { id, text, force } = a;
    let id = ctx.resolve(&id)?;
    let text = joined(&text);
    // Don't overwrite one side of an open text conflict by accident (daemon.md §4.2).
    if !force && ctx.store().conflict_details(Some(&id))?.iter().any(|c| c.kind == "text") {
        return Err(ThcError::Conflict(format!(
            "{} has an open sync conflict · thc conflict ls · resolve it, or pass --force",
            ctx.store().short(&id)
        ))
        .into());
    }
    let r = ctx.write(|b| b.set_text(&id, &text))?;
    if let Some((ev, ())) = r {
        ctx.report_write("updated", &ev, &[id])?;
    }
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct TagArgs {
    id: String,
    #[arg(required = true, allow_hyphen_values = true)]
    tags: Vec<String>,
}

fn tag(ctx: &mut Ctx, a: TagArgs) -> Result<()> {
    let TagArgs { id, tags } = a;
    let id = ctx.resolve(&id)?;
    let (mut add, mut remove) = (vec![], vec![]);
    for t in tags {
        if let Some(r) = t.strip_prefix('-') {
            remove.push(r.to_string());
        } else {
            add.push(t.trim_start_matches('+').to_string());
        }
    }
    let r = ctx.write(|b| b.add_tags(&id, &add, &remove))?;
    if let Some((ev, ())) = r {
        ctx.report_write("tagged", &ev, &[id])?;
    }
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct LinkArgs {
    a: String,
    b: String,
    #[arg(long, default_value = "relates")]
    rel: String,
}

fn link(ctx: &mut Ctx, a: LinkArgs) -> Result<()> {
    let LinkArgs { a, b, rel } = a;
    let (a, b) = (ctx.resolve(&a)?, ctx.resolve(&b)?);
    let r = ctx.write(|bld| bld.link(&a, &b, &rel))?;
    if let Some((ev, added)) = r {
        if ctx.out.json {
            ctx.out.json(&json!({ "ok": true, "tx": ev.first().map(|e| &e.tx), "changed": added as usize }));
        } else if added {
            ctx.out.line(format!("linked {} {rel} {}", ctx.store().short(&a), ctx.store().short(&b)));
        } else {
            ctx.out.line("already linked");
        }
    }
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct UnlinkArgs {
    a: String,
    b: String,
    #[arg(long)]
    rel: Option<String>,
}

fn unlink(ctx: &mut Ctx, a: UnlinkArgs) -> Result<()> {
    let UnlinkArgs { a, b, rel } = a;
    let (a, b) = (ctx.resolve(&a)?, ctx.resolve(&b)?);
    let r = ctx.write(|bld| bld.unlink(&a, &b, rel.as_deref()))?;
    if let Some((ev, n)) = r {
        if ctx.out.json {
            ctx.out.json(&json!({ "ok": true, "tx": ev.first().map(|e| &e.tx), "changed": n }));
        } else {
            ctx.out.line(if n == 0 { "no such link".to_string() } else { format!("unlinked {} link(s)", n) });
        }
    }
    Ok(())
}
