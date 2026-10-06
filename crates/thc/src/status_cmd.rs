//! `thc status` (docs/design/status.md §4): where the board stands, as text or
//! JSON, and `--collect`, which fills in token props from the agents' own session logs (§2).

use crate::Ctx;
use anyhow::Result;
use serde_json::json;
use std::path::PathBuf;
use thc_core::status::{self, Report, Tokens, duration, tokens_short};

pub const SPEC: crate::registry::Spec = crate::spec!("status", "Where the board stands: done, in flight, blocked, momentum, times and tokens", StatusArgs, run, board: true, settings: true, verbs: |m| if m.get_flag("collect") { vec!["set".into()] } else { vec![] });

#[derive(clap::Args, Debug)]
pub struct StatusArgs {
    /// today, 3d (default), 7d, Nd, all, or a date.
    #[arg(long, alias = "range", default_value = "3d")]
    since: String,
    /// Only tasks owned by this actor.
    #[arg(long)]
    by: Option<String>,
    /// First reconcile token props from the agents' session logs (usage numbers only), as actor
    /// `collector`, one undoable transaction per task.
    #[arg(long)]
    collect: bool,
}

fn run(ctx: &mut Ctx, a: StatusArgs) -> Result<()> {
    let now = thc_core::dates::now_local();
    let board = ctx.board.as_ref().expect("status resolved its board").clone();
    let root = board.page.as_ref().map(|p| p.id.clone());
    if a.collect {
        let range = status::Range::parse(&a.since, now)?;
        let r = status::report(ctx.store(), root.as_deref(), range, a.by.as_deref(), now)?;
        let n = collect(ctx, &r)?;
        if !ctx.out.json {
            ctx.out.line(format!("collected tokens for {n} task{}", if n == 1 { "" } else { "s" }));
        }
    }
    let range = status::Range::parse(&a.since, now)?;
    let r = status::report(ctx.store(), root.as_deref(), range, a.by.as_deref(), now)?;
    if ctx.out.json {
        let mut v = serde_json::to_value(&r)?;
        v["board"] = json!({ "vault": board.vault, "page": board.page.as_ref().map(|p| &p.title), "id": root });
        ctx.out.json(&v);
        return Ok(());
    }
    text(ctx, &board, &r);
    Ok(())
}

fn text(ctx: &mut Ctx, board: &thc_core::board::Board, r: &Report) {
    let page = board.page.as_ref().map(|p| format!(" · ¶ {}", p.title)).unwrap_or_default();
    let c = &r.counts;
    let med = |m: Option<i64>| m.map(|m| duration(m * 60_000)).unwrap_or_else(|| "—".into());
    ctx.out.line(format!("Status · {}{page} · {}", board.vault, r.range.label));
    ctx.out.line(format!(
        "{} done   {} doing   {} ready   {} blocked   {} to review      worked {} · waited {} (medians of {})",
        c.done, c.doing, c.ready, c.blocked, c.to_review, med(r.timing.worked_median), med(r.timing.waited_median), r.timing.sample
    ));
    let days: Vec<String> = r.momentum.days.iter().map(|d| format!("{} {}", d.date.get(5..).unwrap_or(&d.date), if d.done == 0 { "·".to_string() } else { format!("{} {}", "▇".repeat(d.done.min(20)), d.done) })).collect();
    ctx.out.line(format!("momentum  {}", days.join("   ")));
    let tok = |t: &Option<Tokens>| t.as_ref().map(|t| format!("{}{}", if t.source.as_deref() == Some("self") { "~" } else { "" }, tokens_short(t.shown()))).unwrap_or_else(|| "—".into());
    let by = |o: &Option<String>| o.as_ref().map(|o| format!("◆ {o}")).unwrap_or_default();
    let flight: Vec<&status::Task> = {
        let mut v: Vec<&status::Task> = r.tasks.iter().filter(|t| t.status == "doing").collect();
        v.sort_by_key(|t| (!t.long, std::cmp::Reverse(t.worked)));
        v
    };
    ctx.out.line("");
    ctx.out.line(format!("In flight  {}", flight.len()));
    for t in flight {
        let started = t.times.started.map(|s| hm(s)).unwrap_or_else(|| "—".into());
        let for_ = t.worked.map(|w| duration(w * 60_000)).unwrap_or_else(|| "—".into());
        ctx.out.line(format!("  {}  {:<44} {:>6}  {:>7} {} {:>6}  {}", t.short, cut(&t.title, 44), started, for_, if t.long { "⚠" } else { " " }, tok(&t.tokens), by(&t.owner)));
    }
    let landed: Vec<&status::Task> = r.tasks.iter().filter(|t| t.done.is_some()).collect();
    ctx.out.line("");
    ctx.out.line(format!("Landed  {} today · {} in range", c.done_today, landed.len()));
    for t in landed {
        let reopened = if t.reopened > 0 { format!(" ↺ {}", t.reopened) } else { String::new() };
        let title = cut(&format!("{}{reopened}", t.title), 40);
        if t.untracked {
            ctx.out.line(format!("  {}  {:<40} {:>6}  {:>6}  {:>7}  {:>7}  {:>6}  untracked", t.short, title, "—", t.times.done.map(hm).unwrap_or_default(), "—", "—", tok(&t.tokens)));
            continue;
        }
        let worked = if t.batch { "<1m".to_string() } else { t.worked.map(|w| duration(w * 60_000)).unwrap_or_else(|| "—".into()) };
        let waited = t.waited.map(|w| duration(w * 60_000)).unwrap_or_else(|| "—".into());
        ctx.out.line(format!(
            "  {}  {:<40} {:>6}  {:>6}  {:>7}  {:>7}  {:>6}  {}",
            t.short,
            title,
            t.times.started.map(hm).unwrap_or_default(),
            t.times.done.map(hm).unwrap_or_default(),
            worked,
            waited,
            tok(&t.tokens),
            by(&t.owner)
        ));
    }
    if !r.blocked.is_empty() {
        ctx.out.line("");
        ctx.out.line(format!("Blocked  {}", c.blocked));
        let short = |id: &str| ctx.store().short(id);
        let lines: Vec<String> = r
            .blocked
            .iter()
            .flat_map(|b| {
                if !b.holds_up.is_empty() {
                    let direct: Vec<String> = b.direct.iter().map(|d| short(d)).collect();
                    vec![
                        format!("  ◼ {} {} holds up {} task{}   [{}] {}", b.short, cut(&b.title, 40), b.holds_up.len(), if b.holds_up.len() == 1 { "" } else { "s" }, b.status, by(&b.owner)),
                        format!("      directly {}", direct.join(", ")),
                    ]
                } else {
                    let by_: Vec<String> = b.blocked_by.iter().map(|d| short(d)).collect();
                    vec![format!("    {} {} ← {}", b.short, cut(&b.title, 44), by_.join(", "))]
                }
            })
            .collect();
        for l in lines {
            ctx.out.line(l);
        }
    }
    ctx.out.line("");
    ctx.out.line(format!("tokens known for {} of {}", r.tokens.known, r.tokens.of));
}

fn hm(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(ms).single().map(|d| d.format("%H:%M").to_string()).unwrap_or_default()
}

fn cut(s: &str, n: usize) -> String {
    let s = s.lines().next().unwrap_or("");
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}

// ---- tokens from session logs (§2) --------------------------------------------------------------

/// Manual collection also covers in-flight work, through the same session reader as the daemon.
fn collect(ctx: &mut Ctx, r: &Report) -> Result<usize> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let mut n = 0;
    for t in r.tasks.iter().filter(|t| t.done.is_some() || t.status == "doing") {
        let Some(input) = thc_core::token_usage::input(ctx.store(), &t.id)? else { continue };
        let Some(tokens) = thc_core::token_usage::collect(&home, &input, r.range.to_ms) else { continue };
        let kv = input.props(&tokens, r.range.to_ms);
        let me = std::mem::replace(&mut ctx.vault.actor, thc_core::event::Actor { kind: "agent".into(), name: Some("collector".into()) });
        let collector_policy = thc_core::policy::Policy::load(&ctx.vault.actor, ctx.readonly);
        let w = (|| {
            if !ctx.dry_run {
                collector_policy?.check(&["set".into()], ctx.yes)?;
            }
            ctx.write(|b| {
                if thc_core::token_usage::input(b.store, &input.id)?.as_ref() != Some(&input) {
                    return Err(thc_core::error::invalid("claim changed during token collection"));
                }
                b.set_props(&input.id, &kv)
            })
        })();
        ctx.vault.actor = me;
        if w?.is_some() { n += 1; }
    }
    Ok(n)
}
