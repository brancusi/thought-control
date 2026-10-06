//! `thc view add|set|ls|rm` (docs/design/views.md §1.1).

use crate::Ctx;
use crate::cli::ViewCmd;
use anyhow::Result;
use serde_json::json;
use thc_core::views::{self, Section, Sectioned, Update, View};

fn count(ctx: &Ctx, q: &str) -> Option<usize> {
    ctx.store().query(q, ctx.out.today, 100_000).ok().map(|v| v.len())
}

/// Seed the system page and built-in views once (first use on this vault).
fn seed(ctx: &mut Ctx) -> Result<()> {
    // A system write, not the actor's: no tier check. Read-only mode skips it entirely.
    if !views::page_exists(ctx.store())? && !ctx.dry_run && ctx.vault.readonly.is_none() {
        ctx.system = true;
        let r = ctx.write(|b| views::ensure_seeded(b).map(|_| ()));
        ctx.system = false;
        r?;
    }
    Ok(())
}

fn where_shown(v: &View) -> String {
    let mut bits = Vec::new();
    if let Some(t) = v.tasks {
        bits.push(format!("tasks {t}"));
    }
    if v.bar {
        bits.push("bar".to_string());
    }
    bits.join(" · ")
}

fn view_json(ctx: &Ctx, v: &View) -> serde_json::Value {
    let mut j = json!(v);
    j["count"] = json!(count(ctx, &v.query));
    j
}

/// Whether the current vault is in the registry. One that isn't (a scratch vault named by
/// `--vault <path>` or `THC_VAULT`) is a world of its own: its views live in it, and nothing
/// it does reaches the registered vaults (your home above all).
fn registered(ctx: &Ctx) -> bool {
    thc_core::registry::Registry::load().by_path(&ctx.vault.paths.vault).is_some()
}

/// Run `f` with the home vault as the write target (a view that spans vaults lives there:
/// vaults.md §3.3), then put the current vault back. Returns whether it moved. An unregistered
/// vault is its own home.
pub(crate) fn in_home<R>(ctx: &mut Ctx, f: impl FnOnce(&mut Ctx) -> Result<R>) -> Result<(R, Option<String>)> {
    let reg = thc_core::registry::Registry::load();
    match reg.home.clone() {
        Some(h) if registered(ctx) => in_vault(ctx, &h, f),
        _ => Ok((f(ctx)?, None)),
    }
}

/// Run `f` with a registered vault as the target (None back when that's the current one).
pub(crate) fn in_vault<R>(ctx: &mut Ctx, name: &str, f: impl FnOnce(&mut Ctx) -> Result<R>) -> Result<(R, Option<String>)> {
    let reg = thc_core::registry::Registry::load();
    let Some(home) = reg.find(name).cloned() else { return Ok((f(ctx)?, None)) };
    match reg.by_path(&ctx.vault.paths.vault) {
        Some(e) if e.name == name => return Ok((f(ctx)?, None)),
        Some(_) => {}
        None => return Err(thc_core::error::usage(format!("this vault isn't registered, so it can't reach vault {name} · thc vault add <path> first"))),
    }
    let paths = thc_core::vault::Paths { vault: home.path.clone(), cache: thc_core::vault::default_cache(&home.path) };
    let mut v = thc_core::vault::Vault::open(paths, ctx.vault.actor.clone(), &ctx.vault.via.clone())?;
    std::mem::swap(&mut ctx.vault, &mut v);
    let r = f(ctx);
    std::mem::swap(&mut ctx.vault, &mut v);
    Ok((r?, Some(home.name)))
}

/// Where a sectioned view with this scope lives (vaults.md §3.3): home when it spans vaults,
/// the one vault it names, else here (None).
fn home_of(ctx: &Ctx, scope: Option<&str>) -> Option<String> {
    if !registered(ctx) {
        return None;
    }
    if views::spans_vaults(scope) {
        return thc_core::registry::Registry::load().home;
    }
    let s = scope?.trim();
    s.strip_prefix("vault:").filter(|n| thc_core::registry::valid_name(n)).map(str::to_string)
}

/// Write a sectioned view where it lives, and remove a copy of it left anywhere else in
/// scope (here, home): one view, one place.
fn write_sectioned(ctx: &mut Ctx, name: &str, scope_arg: Option<Option<String>>, sections: &[Section], live: Option<String>) -> Result<(Option<(Vec<thc_core::event::Event>, Sectioned)>, Option<String>)> {
    let today = ctx.out.today;
    let write = |c: &mut Ctx| -> Result<Option<(Vec<thc_core::event::Event>, Sectioned)>> {
        seed(c)?;
        c.write(|b| views::set_sectioned(b, name, scope_arg.as_ref().map(|o| o.as_deref()), sections, today))
    };
    let (r, moved) = match &live {
        Some(v) => in_vault(ctx, v, write)?,
        None => (write(ctx)?, None),
    };
    // Stale copies: here (if it now lives elsewhere) and home (if it now lives elsewhere).
    if !registered(ctx) {
        return Ok((r, moved));
    }
    let here = out_name();
    let reg = thc_core::registry::Registry::load();
    let lives_in = live.clone().unwrap_or_else(|| here.clone());
    let mut stale: Vec<String> = vec![here.clone()];
    if let Some(h) = reg.home.clone() {
        stale.push(h);
    }
    stale.retain(|v| *v != lives_in);
    stale.dedup();
    for v in stale {
        let remove = |c: &mut Ctx| -> Result<()> {
            if views::sectioned(c.store(), name)?.is_some_and(|s| s.edited) {
                c.write(|b| views::remove(b, name).map(|_| ()))?;
            }
            Ok(())
        };
        in_vault(ctx, &v, remove)?;
    }
    Ok((r, moved))
}

fn out_name() -> String {
    crate::out::VAULT_NAME.get().cloned().unwrap_or_default()
}

fn sectioned_json(s: &Sectioned) -> serde_json::Value {
    json!({ "name": s.name, "scope": s.scope, "sections": s.sections, "builtin": s.builtin, "edited": s.edited })
}

/// Your version of a sectioned view (here, else in the home vault), if you've made or edited it.
pub(crate) fn edited_sectioned(ctx: &mut Ctx, name: &str) -> Result<Option<Sectioned>> {
    Ok(find_sectioned(ctx, name)?.filter(|s| s.edited))
}

/// The sectioned view for a name, from where it lives: the home vault when it spans vaults or
/// is a built-in that does, else this one.
pub(crate) fn find_sectioned(ctx: &mut Ctx, name: &str) -> Result<Option<Sectioned>> {
    if let Some(s) = views::sectioned(ctx.store(), name)?.filter(|s| s.edited) {
        return Ok(Some(s));
    }
    // Where else it may live: home first (spanning views), then the vault it names.
    if !registered(ctx) {
        return Ok(views::sectioned(ctx.store(), name)?);
    }
    let reg = thc_core::registry::Registry::load();
    let here = out_name();
    let mut order: Vec<String> = reg.home.iter().cloned().collect();
    order.extend(reg.vaults.iter().map(|e| e.name.clone()));
    order.dedup();
    for v in order.into_iter().filter(|v| *v != here) {
        let (found, _) = in_vault(ctx, &v, |c| views::sectioned(c.store(), name))?;
        if let Some(f) = found.filter(|f| f.edited) {
            return Ok(Some(f));
        }
    }
    Ok(views::sectioned(ctx.store(), name)?)
}

#[derive(clap::Args, Debug)]
pub struct ViewArgs {
    #[command(subcommand)]
    cmd: ViewCmd,
}

pub const SPEC: crate::registry::Spec = crate::spec!("view", "Saved views: named queries (`thc q @work`). They sync like notes and undo like any write", ViewArgs, |ctx: &mut Ctx, a: ViewArgs| run(ctx, a.cmd), verbs: |m| match m.subcommand_name() {
    Some("add" | "set" | "rm" | "reset" | "copy") => vec!["view".into(), "views".into()],
    _ => vec![],
});

pub fn run(ctx: &mut Ctx, cmd: ViewCmd) -> Result<()> {
    match cmd {
        ViewCmd::Add { name, query, title, tasks, bar, capture } => {
            views::validate_query(ctx.store(), &query, ctx.out.today)?;
            let r = ctx.write(|b| views::add(b, &name, &query, title.as_deref(), tasks, bar, capture.as_deref()))?;
            if let Some((ev, _)) = r {
                let name = views::normalize_name(&name)?;
                let n = count(ctx, &query).unwrap_or(0);
                if ctx.out.json {
                    let v = views::find(ctx.store(), &name)?.map(|v| view_json(ctx, &v));
                    ctx.out.json(&json!({ "ok": true, "tx": ev.first().map(|e| &e.tx), "view": v }));
                } else {
                    ctx.out.line(format!("added view @{name} · {n} match right now · thc q @{name}"));
                }
            }
        }
        ViewCmd::Set { name, query, title, tasks, bar, no_bar, capture, section, scope } if !section.is_empty() || scope.is_some() || views::sectioned(ctx.store(), &name)?.is_some_and(|v| v.builtin) => {
            // A sectioned view: its sections and scope (vaults.md §3.2).
            let name = views::normalize_name(&name)?;
            if query.is_some() || title.is_some() || tasks.is_some() || bar || no_bar || capture.is_some() {
                return Err(thc_core::error::usage(format!("@{name} has sections · set them with --section TITLE QUERY (and --scope)")));
            }
            let had = find_sectioned(ctx, &name)?;
            let sections: Vec<Section> = if section.is_empty() {
                had.as_ref().map(|h| h.sections.clone()).ok_or_else(|| thc_core::error::usage(format!("@{name} has no sections yet · --section TITLE QUERY")))?
            } else {
                section.chunks(2).map(|c| Section { title: c[0].clone(), query: c[1].clone() }).collect()
            };
            let scope_arg: Option<Option<String>> = scope.map(|s| if s.trim().is_empty() { None } else { Some(s) });
            let effective_scope = match &scope_arg {
                Some(s) => s.clone(),
                None => had.as_ref().and_then(|h| h.scope.clone()),
            };
            // Spanning vaults: it lives in the home vault (a team vault never holds one); one
            // vault: in that vault (vaults.md §3.3). A built-in without a scope stays here.
            let live = home_of(ctx, effective_scope.as_deref());
            let (r, moved) = write_sectioned(ctx, &name, scope_arg, &sections, live)?;
            if let Some((ev, v)) = r {
                if ctx.out.json {
                    ctx.out.json(&json!({ "ok": true, "tx": ev.first().map(|e| &e.tx), "view": sectioned_json(&v), "kept_in": moved }));
                } else {
                    let n = v.sections.len();
                    let why = if views::spans_vaults(v.scope.as_deref()) { "spans vaults, so it's kept in" } else { "kept in" };
                    let place = moved.map(|h| ctx.out.dim(&format!(" · {why} {h}"))).unwrap_or_default();
                    // Sections replace the list: say what went, so a one-section set can't lose
                    // Today quietly.
                    let dropped: Vec<String> = had.as_ref().map(|h| h.sections.iter().filter(|o| !v.sections.iter().any(|x| x.title == o.title)).map(|o| o.title.clone()).collect()).unwrap_or_default();
                    let gone = if dropped.is_empty() { String::new() } else { ctx.out.dim(&format!(" · dropped {} · thc undo brings them back", dropped.join(", "))) };
                    ctx.out.line(format!("updated view @{} · {n} section{}{}{gone}{place}", v.name, if n == 1 { "" } else { "s" }, v.scope.as_ref().map(|s| format!(" · {s}")).unwrap_or_default()));
                }
            }
        }
        ViewCmd::Reset { name } => {
            let name = views::normalize_name(&name)?;
            let here_has = views::sectioned(ctx.store(), &name)?.is_some_and(|v| v.edited);
            let do_reset = |c: &mut Ctx| -> Result<Option<(Vec<thc_core::event::Event>, ())>> { c.write(|b| views::reset(b, &name)) };
            let (r, _) = if here_has { (do_reset(ctx)?, None) } else { in_home(ctx, do_reset)? };
            if let Some((ev, ())) = r {
                if ctx.out.json {
                    ctx.out.json(&json!({ "ok": true, "tx": ev.first().map(|e| &e.tx), "reset": name }));
                } else {
                    ctx.out.line(format!("@{name} is the shipped one again · thc undo brings yours back"));
                }
            }
        }
        ViewCmd::Copy { from, to } => {
            let from_s = find_sectioned(ctx, &from)?;
            let to = views::normalize_name(&to)?;
            match from_s {
                Some(src) => {
                    let sections = src.sections.clone();
                    let live = home_of(ctx, src.scope.as_deref());
                    let (r, moved) = write_sectioned(ctx, &to, Some(src.scope.clone()), &sections, live)?;
                    if let Some((ev, v)) = r {
                        if ctx.out.json {
                            ctx.out.json(&json!({ "ok": true, "tx": ev.first().map(|e| &e.tx), "view": sectioned_json(&v), "kept_in": moved }));
                        } else {
                            ctx.out.line(format!("copied @{} to @{} · thc view set {} --scope 'vault:<name>' to narrow it", src.name, v.name, v.name));
                        }
                    }
                }
                None => {
                    let src = views::find(ctx.store(), &from)?.ok_or_else(|| thc_core::error::not_found(format!("no view @{}", from.trim_start_matches('@'))))?;
                    let r = ctx.write(|b| views::add(b, &to, &src.query, src.title.as_deref(), None, false, src.capture.as_deref()))?;
                    if let Some((ev, _)) = r {
                        if ctx.out.json {
                            ctx.out.json(&json!({ "ok": true, "tx": ev.first().map(|e| &e.tx), "view": to }));
                        } else {
                            ctx.out.line(format!("copied @{} to @{to}", src.name));
                        }
                    }
                }
            }
        }
        ViewCmd::Set { name, query, title, tasks, bar, no_bar, capture, .. } => {
            seed(ctx)?;
            if let Some(q) = &query {
                views::validate_query(ctx.store(), q, ctx.out.today)?;
            }
            let u = Update {
                query: query.as_deref(),
                title: title.as_deref().map(|t| if t.is_empty() { None } else { Some(t) }),
                tasks: tasks.map(|t| if t == 0 { None } else { Some(t) }),
                bar: if bar { Some(true) } else if no_bar { Some(false) } else { None },
                capture: capture.as_deref().map(|c| if c.is_empty() { None } else { Some(c) }),
            };
            let r = ctx.write(|b| views::set(b, &name, u))?;
            if let Some((ev, v)) = r {
                let now = views::find(ctx.store(), &v.name)?.unwrap_or(v);
                if ctx.out.json {
                    let j = view_json(ctx, &now);
                    ctx.out.json(&json!({ "ok": true, "tx": ev.first().map(|e| &e.tx), "view": j }));
                } else if ev.is_empty() {
                    ctx.out.line(format!("@{} unchanged", now.name));
                } else {
                    ctx.out.line(format!("updated view @{} · {}", now.name, now.query));
                }
            }
        }
        ViewCmd::Ls => {
            seed(ctx)?;
            // Sectioned views: the built-ins (yours where edited) and any you made, from here
            // and, for those spanning vaults, from home.
            let mut sectioned: Vec<Sectioned> = views::all_sectioned(ctx.store())?;
            let (home_s, _) = in_home(ctx, |c| views::all_sectioned(c.store()))?;
            for h in home_s {
                match sectioned.iter_mut().find(|s| s.name == h.name) {
                    Some(s) if !s.edited && h.edited => *s = h,
                    Some(_) => {}
                    None => sectioned.push(h),
                }
            }
            let names: Vec<String> = sectioned.iter().map(|s| s.name.clone()).collect();
            let all: Vec<View> = views::list(ctx.store())?.into_iter().filter(|v| !names.contains(&v.name)).collect();
            if ctx.out.json {
                let items: Vec<_> = all.iter().map(|v| view_json(ctx, v)).collect();
                let secs: Vec<_> = sectioned.iter().map(sectioned_json).collect();
                ctx.out.json(&json!({ "views": items, "sectioned": secs }));
                return Ok(());
            }
            for s in &sectioned {
                let kind = match (s.builtin, s.edited) {
                    (true, false) => "built-in",
                    (true, true) => "built-in · edited",
                    _ => "sections",
                };
                let scope = s.scope.clone().unwrap_or_else(|| "this vault".into());
                ctx.out.line(format!("@{:<10} {}", s.name, ctx.out.dim(&format!("{kind} · {scope}"))));
                for sec in &s.sections {
                    let q: String = sec.query.chars().take(60).collect();
                    ctx.out.line(format!("  {:<12} {}", sec.title, ctx.out.dim(&q)));
                }
            }
            if all.is_empty() {
                ctx.out.line("no views · thc view add <name> '<query>'");
            } else {
                let w = all.iter().map(|v| v.name.len()).max().unwrap_or(4) + 1;
                for v in &all {
                    let n = count(ctx, &v.query).map(|n| n.to_string()).unwrap_or_else(|| "?".into());
                    let q: String = v.query.chars().take(42).collect();
                    let shown = ctx.out.dim(&where_shown(v));
                    ctx.out.line(format!("@{:<w$}  {:<42} {:>4}   {shown}", v.name, q, n));
                }
            }
        }
        ViewCmd::Rm { name } => {
            seed(ctx)?;
            let r = ctx.write(|b| views::remove(b, &name))?;
            if let Some((ev, v)) = r {
                if ctx.out.json {
                    ctx.out.json(&json!({ "ok": true, "tx": ev.first().map(|e| &e.tx), "removed": v.name }));
                } else {
                    ctx.out.line(format!("removed view @{} · thc undo brings it back", v.name));
                }
            }
        }
    }
    Ok(())
}
