//! Registered listing commands, migrated without changing their behavior.

use crate::{Ctx, emit_list, joined, print_tree, tree_json};
use anyhow::Result;
use serde_json::json;
use thc_core::dates;

pub const TODAY: crate::registry::Spec = crate::spec!("today", "Overdue, due/scheduled today, and today's alerts", TodayArgs, today, listing: true);
pub const AGENDA: crate::registry::Spec =
    crate::spec!("agenda", "Upcoming days", AgendaArgs, agenda, listing: true);
pub const INBOX: crate::registry::Spec =
    crate::spec!("inbox", "Root items with no page or journal", InboxArgs, inbox, listing: true);
pub const JOURNAL: crate::registry::Spec = crate::spec!("journal", "Show a journal day (default today)", JournalArgs, journal, listing: true);
pub const PAGES: crate::registry::Spec = crate::spec!(
    "pages",
    "List pages (alias for `page ls`)",
    PagesArgs,
    pages
);
pub const SEARCH: crate::registry::Spec =
    crate::spec!("search", "Full-text search", SearchArgs, search, listing: true);

/// Overdue, due/scheduled today, and today's alerts.
#[derive(clap::Args, Debug)]
pub struct TodayArgs {
    /// Print exactly what the daemon's `today` method returns (the ThoughtBar panel; implies --json).
    #[arg(long)]
    panel: bool,
    /// Say why each row is here (`← scheduled · repeating`).
    #[arg(long)]
    why: bool,
}

fn today(ctx: &mut Ctx, a: TodayArgs) -> Result<()> {
    let TodayArgs { panel, why } = a;
    if panel {
        let mut v = thc_daemon::today_panel(ctx.store())?;
        v["context"] = json!(ctx.context.device);
        ctx.out.json(&v);
    } else {
        crate::today(ctx, why)?;
    }
    Ok(())
}

/// Upcoming days.
#[derive(clap::Args, Debug)]
pub struct AgendaArgs {
    #[arg(long, default_value_t = 7)]
    days: i64,
    /// Say why each row is here.
    #[arg(long)]
    why: bool,
}

fn agenda(ctx: &mut Ctx, a: AgendaArgs) -> Result<()> {
    let AgendaArgs { days, why } = a;
    crate::agenda(ctx, days, why)?;
    Ok(())
}

/// Root items with no page or journal.
#[derive(clap::Args, Debug)]
pub struct InboxArgs {}

fn inbox(ctx: &mut Ctx, _a: InboxArgs) -> Result<()> {
    let nodes = ctx.store().nodes_where(
            &format!("n.parent IS NULL AND n.title IS NULL AND n.journal IS NULL AND n.is_tag=0 AND n.deleted=0 ORDER BY n.created_ms DESC LIMIT {}", ctx.limit),
            &[],
        )?;
    let (nodes, total) = ctx.cfilter(nodes);
    if !ctx.context_line(nodes.len(), total, "in the inbox", "") {
        emit_list(ctx, "Inbox", &nodes, false);
    }
    Ok(())
}

/// Show a journal day (default today).
#[derive(clap::Args, Debug)]
pub struct JournalArgs {
    date: Option<String>,
}

fn journal(ctx: &mut Ctx, a: JournalArgs) -> Result<()> {
    let JournalArgs { date } = a;
    let d = match date {
        Some(s) => dates::parse(&s, ctx.out.today)?.date(),
        None => ctx.out.today,
    };
    let key = d.format("%Y-%m-%d").to_string();
    let entries = ctx.store().journal_entries(&key)?;
    let (entries, total) = ctx.cfilter(entries);
    if ctx.out.json {
        let items = tree_json(ctx, &entries, 3)?;
        let cj = ctx.context_json();
        ctx.out.json(&json!({ "context": cj, "journal": key, "id": ctx.store().journal_node(&key)?, "items": items }));
    } else {
        if ctx.context_line(entries.len(), total, "written", "") {
            return Ok(());
        }
        let title = format!(
            "{} · {}",
            d.format("%A, %B %-d %Y"),
            dates::relative(d, ctx.out.today)
        );
        ctx.out.heading(&title);
        if entries.is_empty() {
            let hint = ctx.out.dim("  (empty; `thc add …` writes here)");
            ctx.out.line(hint);
        }
        for e in &entries {
            print_tree(ctx, e, 0, 3)?;
        }
    }
    Ok(())
}

/// List pages (alias for `page ls`).
#[derive(clap::Args, Debug)]
pub struct PagesArgs {}

fn pages(ctx: &mut Ctx, _a: PagesArgs) -> Result<()> {
    let nodes = ctx.store().nodes_where(
            &format!("n.parent IS NULL AND n.title IS NOT NULL AND n.is_tag=0 AND n.deleted=0 AND {} ORDER BY n.title COLLATE NOCASE LIMIT {}", thc_core::views::HIDDEN_SQL, ctx.limit),
            &[],
        )?;
    emit_list(ctx, "Pages", &nodes, false);
    Ok(())
}

/// Full-text search.
#[derive(clap::Args, Debug)]
pub struct SearchArgs {
    terms: Vec<String>,
}

fn search(ctx: &mut Ctx, a: SearchArgs) -> Result<()> {
    let SearchArgs { terms } = a;
    let nodes = ctx.store().search(&joined(&terms), ctx.limit)?;
    // Search is never filtered by a context; it still says one is on.
    if ctx.context.active.is_some() {
        let n = nodes.len();
        ctx.context_line(n, n, "", " · search ignores contexts");
    }
    crate::ocr_cmd::emit_search(ctx, &nodes, &joined(&terms))
}
