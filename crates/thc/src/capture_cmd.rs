//! Capturing: add, todo, remind, attach. (Their argument types are still in cli.rs: TodoArgs
//! and RemindArgs flatten AddArgs.)

use crate::cli::{AddArgs, RemindArgs, TodoArgs};
use crate::{Ctx, joined};
use anyhow::Result;
use thc_core::{capture, dates, error::invalid, event::Trigger};

pub const ADD: crate::registry::Spec = crate::spec!("add", "Capture text into today's journal (inline fields like due:fri, #tag, !high are parsed)", AddArgs, add, settings: true, verbs: |_| vec!["add".into()]);
pub const TODO: crate::registry::Spec = crate::spec!("todo", "Create a task", TodoArgs, todo, settings: true, verbs: |_| vec!["todo".into()]);
pub const REMIND: crate::registry::Spec = crate::spec!("remind", "Create a reminder: a task with a scheduled time and an alert at that time", RemindArgs, remind, settings: true, verbs: |_| vec!["remind".into()]);
pub const ATTACH: crate::registry::Spec = crate::spec!("attach", "Attach files (screenshots, logs) to a note: copied into the vault's files/ and added as an image line under it (attachments.md)", AttachArgs, attach, settings: true, verbs: |_| vec!["attach".into(), "add".into()]);

#[derive(clap::Args, Debug)]
pub struct AttachArgs {
    /// The note it belongs to.
    id: String,
    /// Files to copy in, attached together in one transaction.
    #[arg(required = true, num_args = 1..)]
    files: Vec<std::path::PathBuf>,
    /// The line's caption (default: the file's name).
    #[arg(long)]
    caption: Option<String>,
}

fn attach(ctx: &mut Ctx, a: AttachArgs) -> Result<()> {
    crate::attachment_cmd::attach(ctx, &a.id, &a.files, a.caption.as_deref())
}

fn add(ctx: &mut Ctx, mut a: AddArgs) -> Result<()> {
    if let Some(k) = a.key.take() {
        a.id = Some(thc_core::id::from_key(&k));
    }
    let text = joined(&a.text);
    if let Some(id) = &a.id {
        let id = thc_core::id::normalize(id);
        if ctx.store().node_exists(&id)? {
            return ctx.report_write("exists", &[], &[id]);
        }
    }
    let today = ctx.out.today;
    let explicit = a.inbox || a.under.is_some() || a.journal.is_some();
    // --plain reads nothing from the text, but still lands where captures go.
    let (text, ctx_parent, from_ctx) = if a.plain {
        let (_, p, f) = ctx.capture_defaults("", explicit)?;
        (text.clone(), p, f)
    } else {
        ctx.capture_defaults(&text, explicit)?
    };
    // --plain: the text as written, nothing read from it (agents' findings).
    let cap = if a.plain { capture::Capture { text: text.clone(), ..Default::default() } } else { capture::parse(&text, today)? };
    let parent_args = (a.inbox, a.under.clone(), a.journal.clone());
    let under = match &parent_args.1 {
        Some(u) => Some(ctx.resolve(u)?),
        None => ctx_parent,
    };
    let id_arg = a.id.map(|i| thc_core::id::normalize(&i));
    let plain = a.plain;
    let r = ctx.write(|b| {
        b.plain = plain;
        let parent = if parent_args.0 {
            None
        } else if under.is_some() {
            under.clone()
        } else {
            let d = match &parent_args.2 {
                Some(j) => dates::parse(j, b.today)?.date(),
                None => b.today,
            };
            Some(b.journal(d)?)
        };
        b.create_from_capture(parent, &cap, id_arg)
    })?;
    if let Some((ev, id)) = r {
        ctx.report_capture("added", &ev, &id, from_ctx)?;
    }
    Ok(())
}

fn todo(ctx: &mut Ctx, mut t: TodoArgs) -> Result<()> {
    if let Some(k) = t.add.key.take() {
        t.add.id = Some(thc_core::id::from_key(&k));
    }
    let today = ctx.out.today;
    let explicit = t.add.inbox || t.add.under.is_some() || t.add.journal.is_some();
    let (text, ctx_parent, from_ctx) = ctx.capture_defaults(&joined(&t.add.text), explicit)?;
    let mut cap = capture::parse(&text, today)?;
    cap.status = Some(cap.status.unwrap_or_else(|| "todo".into()));
    if let Some(d) = &t.due {
        cap.due = Some(dates::parse(d, today)?);
    }
    if let Some(d) = &t.scheduled {
        cap.scheduled = Some(dates::parse(d, today)?);
    }
    if let Some(p) = &t.priority {
        cap.priority = Some(capture::normalize_priority(p).ok_or_else(|| invalid("priority must be high, med or low"))?.into());
    }
    if let Some(r) = &t.repeat {
        cap.repeat = Some(thc_core::recur::Repeat::parse(r, None)?);
    }
    for tag in &t.tag {
        let tag = tag.trim_start_matches('#').to_lowercase();
        if !cap.tags.contains(&tag) {
            cap.tags.push(tag);
        }
    }
    if let Some(id) = &t.add.id {
        let id = thc_core::id::normalize(id);
        if ctx.store().node_exists(&id)? {
            return ctx.report_write("exists", &[], &[id]);
        }
    }
    let under = match &t.add.under {
        Some(u) => Some(ctx.resolve(u)?),
        None => ctx_parent,
    };
    let (inbox, journal) = (t.add.inbox, t.add.journal.clone());
    let id_arg = t.add.id.map(|i| thc_core::id::normalize(&i));
    let r = ctx.write(|b| {
        let parent = if inbox {
            None
        } else if under.is_some() {
            under.clone()
        } else {
            let d = match &journal {
                Some(j) => dates::parse(j, b.today)?.date(),
                None => b.today,
            };
            Some(b.journal(d)?)
        };
        b.create_from_capture(parent, &cap, id_arg)
    })?;
    if let Some((ev, id)) = r {
        ctx.report_capture("added", &ev, &id, from_ctx)?;
    }
    Ok(())
}

fn remind(ctx: &mut Ctx, r: RemindArgs) -> Result<()> {
    let key_id = r.key.as_deref().map(thc_core::id::from_key);
    if let Some(id) = &key_id {
        if ctx.store().node_exists(id)? {
            return ctx.report_write("exists", &[], &[id.clone()]);
        }
    }
    let today = ctx.out.today;
    let (text, ctx_parent, from_ctx) = ctx.capture_defaults(&joined(&r.text), r.inbox || r.under.is_some())?;
    let mut cap = capture::parse(&text, today)?;
    let at = dates::parse(&r.at, today)?;
    let at = match at {
        dates::DateVal::Date(d) => dates::DateVal::DateTime(d.and_hms_opt(9, 0, 0).unwrap()),
        dt => dt,
    };
    cap.scheduled = Some(at);
    if !r.no_task && cap.status.is_none() {
        cap.status = Some("todo".into());
    }
    if let Some(rep) = &r.repeat {
        cap.repeat = Some(thc_core::recur::Repeat::parse(rep, None)?);
    }
    let under = match &r.under {
        Some(u) => Some(ctx.resolve(u)?),
        None => ctx_parent,
    };
    let inbox = r.inbox;
    let res = ctx.write(|b| {
        let parent = if inbox {
            None
        } else if under.is_some() {
            under.clone()
        } else {
            Some(b.journal(b.today)?)
        };
        let id = b.create_from_capture(parent, &cap, key_id.clone())?;
        b.add_alert(&id, Trigger { at: None, offset: Some("0m".into()), anchor: Some("scheduled".into()) })?;
        Ok(id)
    })?;
    if let Some((ev, id)) = res {
        ctx.report_capture("reminder", &ev, &id, from_ctx)?;
    }
    Ok(())
}
