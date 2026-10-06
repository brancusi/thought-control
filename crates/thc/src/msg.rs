//! CLI for ordinary addressed notes; implementation kept out of main's dispatch.

use crate::{Ctx, out::node_json};
use anyhow::Result;
use clap::Args;
use serde_json::json;
use thc_core::{
    error::invalid,
    messages::{self, Recipient},
};

pub const SPEC: crate::registry::Spec = crate::spec!("msg", "Send an addressed note, or mark messages read", MsgArgs, run, board: true, verbs: |m| <MsgArgs as clap::FromArgMatches>::from_arg_matches(m).map(|a| verbs(&a)).unwrap_or_default());
pub const LIST: crate::registry::Spec = crate::spec!("msgs", "List addressed messages (read-only)", MsgsArgs, listing, board: true);

#[derive(Args, Debug)]
pub struct MsgArgs {
    /// Recipient role/actor, or `read` to acknowledge messages.
    #[arg(value_name = "TO|read")]
    pub to: String,
    /// Plain message text, or message ids after `read`.
    #[arg(value_name = "TEXT|ID")]
    pub text: Vec<String>,
    /// Place the message under this task (otherwise ¶ Messages on the board).
    #[arg(long = "on", visible_alias = "about", value_name = "TASK")]
    pub about: Option<String>,
    /// Idempotency key for sending.
    #[arg(long)]
    pub key: Option<String>,
    /// With `read`: acknowledge every unread message for this recipient.
    #[arg(long)]
    pub all: bool,
}

#[derive(Args, Debug)]
pub struct MsgsArgs {
    /// Role or actor to list (default: THC_ROLE and your actor).
    #[arg(long = "for", value_name = "ROLE|ACTOR")]
    pub recipient: Option<String>,
    /// Show only messages this recipient has not acknowledged.
    #[arg(long)]
    pub unread: bool,
}

pub fn identity(ctx: &Ctx) -> Recipient {
    let actor = ctx.vault.actor.name.as_deref().unwrap_or("human");
    Recipient::current(Some(actor))
}

fn recipient(ctx: &Ctx, target: Option<&str>) -> Result<Recipient> {
    match target {
        Some(t) => {
            let mut r = Recipient::target(t)?;
            if r.actor.is_none() {
                r.actor = identity(ctx).actor;
            }
            Ok(r)
        }
        None => Ok(identity(ctx)),
    }
}

pub fn verbs(args: &MsgArgs) -> Vec<String> {
    if args.to == "read" { vec!["msg".into(), "set".into()] } else { vec!["msg".into(), "add".into()] }
}

pub fn run(ctx: &mut Ctx, args: MsgArgs) -> Result<()> {
    if args.to == "read" {
        if args.about.is_some() || args.key.is_some() {
            return Err(invalid("msg read does not take --on or --key"));
        }
        if (args.all && !args.text.is_empty()) || (!args.all && args.text.is_empty()) {
            return Err(invalid("msg read takes message ids or --all"));
        }
        let recipient = identity(ctx);
        let ids = if args.all {
            // Explicit --all must not silently stop at the display limit.
            messages::list(ctx.store(), &recipient, true, usize::MAX)?.into_iter().map(|n| n.id).collect()
        } else {
            args.text.iter().map(|id| ctx.resolve(id)).collect::<Result<Vec<_>>>()?
        };
        if let Some((events, ())) = ctx.write(|b| messages::mark_read(b, &ids, &recipient))? {
            ctx.report_write("read", &events, &ids)?;
        }
    } else {
        if args.all {
            return Err(invalid("--all is for msg read"));
        }
        messages::validate(&args.to)?;
        let text = args.text.join(" ");
        if text.trim().is_empty() {
            return Err(invalid("message text is empty"));
        }
        let id = args.key.as_deref().map(thc_core::id::from_key);
        if let Some(id) = &id {
            if ctx.store().node_exists(id)? {
                return ctx.report_write("exists", &[], &[id.clone()]);
            }
        }
        let parent = args.about.as_deref().map(|id| ctx.resolve(id)).transpose()?;
        let on = parent.as_deref().map(|id| ctx.store().short(id));
        let from = ctx.vault.actor.name.clone().unwrap_or_else(|| "human".into());
        if let Some((events, id)) = ctx.write(|b| messages::send(b, &args.to, &from, &text, parent, id))? {
            if ctx.out.json {
                ctx.report_write("sent", &events, &[id])?;
            } else {
                let place = on.map(|s| format!(" · on {s}")).unwrap_or_default();
                ctx.out.line(format!("sent to {}{place}", args.to));
            }
        }
    }
    Ok(())
}

pub fn listing(ctx: &mut Ctx, args: MsgsArgs) -> Result<()> {
    let recipient = recipient(ctx, args.recipient.as_deref())?;
    let nodes = messages::list(ctx.store(), &recipient, args.unread, ctx.limit)?;
    if ctx.out.json {
        let items: Vec<_> = nodes.iter().map(|n| node_json(ctx.store(), n)).collect();
        ctx.out.json(&json!({ "count": items.len(), "items": items, "recipient": { "role": recipient.role, "actor": recipient.actor } }));
    } else {
        ctx.list(Some("Messages"), &nodes, true);
    }
    Ok(())
}

/// Role prime acknowledges just what it will display, using the normal write guard.
/// A dry-run emits only its standard ops object. Read-only briefings leave receipts untouched.
pub fn brief(ctx: &mut Ctx, role: &str) -> Result<Option<Vec<thc_core::model::Node>>> {
    let recipient = Recipient::new(Some(role), identity(ctx).actor.as_deref());
    let nodes = messages::list(ctx.store(), &recipient, true, ctx.limit)?;
    if !nodes.is_empty() && !ctx.readonly && ctx.vault.readonly.is_none() {
        let ids = nodes.iter().map(|n| n.id.clone()).collect::<Vec<_>>();
        let old_verbs = std::mem::replace(&mut ctx.verbs, vec!["msg".into(), "set".into()]);
        let result = ctx.write(|b| messages::mark_read(b, &ids, &recipient));
        ctx.verbs = old_verbs;
        if result?.is_none() { return Ok(None); }
    }
    Ok(Some(nodes))
}

pub fn show_brief(ctx: &mut Ctx, role: &str, nodes: &[thc_core::model::Node]) -> Result<()> {
    ctx.out.line(format!("\nmessages for {role} ({} unread)", nodes.len()));
    for n in nodes {
        let props = ctx.store().props_of(&n.id)?;
        let from = props.get("from").and_then(|v| v.as_str()).unwrap_or("unknown");
        let at = crate::out::ms_to_local(n.created_ms);
        let time = at.get(11..).unwrap_or(&at);
        let on = n.parent.as_deref().and_then(|id| ctx.store().node(id).ok().flatten())
            .filter(|n| n.status.is_some()).map(|p| format!(" · on {}", ctx.store().short(&p.id))).unwrap_or_default();
        ctx.out.line(format!("  ◆ {from} · {time}{on} · {} {}", ctx.store().short(&n.id), ctx.store().render_text(&n.text)));
    }
    Ok(())
}
