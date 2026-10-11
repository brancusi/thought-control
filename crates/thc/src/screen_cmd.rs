//! `thc screen`: thc's views on fictty (crates/thc-screen). Live in this terminal by default;
//! `--render WxH` prints one frame without a terminal; `--feed NAME` prints a view's data.

use crate::cli::Cli;
use anyhow::{Context, Result};
use clap::Args;
use thc_core::error::usage;
use thc_core::event::Actor;
use thc_core::vault::{Paths, Vault};
use thc_screen::{Route, ThcHost, View};

pub const SPEC: crate::registry::Spec = crate::with_paths!(
    "screen",
    "thc's views on fictty (preview): run live here, or --render WxH one frame without a terminal",
    ScreenArgs,
    run
);

#[derive(Args, Debug)]
pub struct ScreenArgs {
    /// Print one frame of this size (`120x32`) instead of running in the terminal.
    #[arg(long, value_name = "WxH")]
    render: Option<String>,
    /// today, inbox, tasks, pages, journal, search, log, detail (with --id) or outline (with --id).
    #[arg(long, default_value = "today")]
    view: String,
    /// ember-dark, ember-light, redacted or newsprint.
    #[arg(long, default_value = "ember-dark")]
    theme: String,
    /// The Tasks filter or the Search words.
    #[arg(long)]
    query: Option<String>,
    /// The node a detail or outline shows.
    #[arg(long)]
    id: Option<String>,
    /// The journal day (`2026-10-11`, or anything `thc` reads as a date).
    #[arg(long)]
    date: Option<String>,
    /// With --render: keys to type first, comma-separated (`j,j,enter`).
    #[arg(long, value_delimiter = ',')]
    keys: Vec<String>,
    /// With --render: colours and attributes as ANSI escapes.
    #[arg(long)]
    ansi: bool,
    /// Print a view's data (a feed: today, inbox, tasks, …) as JSON instead of drawing it.
    #[arg(long, value_name = "NAME")]
    feed: Option<String>,
    /// The control socket agents reach the live screen on (default: one per vault in the temp
    /// dir).
    #[arg(long)]
    socket: Option<std::path::PathBuf>,
}

fn run(cli: &Cli, paths: &Paths, a: ScreenArgs) -> Result<()> {
    let _ = cli;
    // The screen writes as the person at the keyboard, whatever THC_ACTOR says.
    let vault = Vault::open(paths.clone(), Actor { kind: "human".into(), name: None }, "screen").context("opening the vault")?;
    let today = thc_core::dates::today();
    let view = View::parse(&a.view).ok_or_else(|| usage(format!("no view {:?} · today, inbox, tasks, pages, journal, search, log, detail, outline", a.view)))?;
    let id = match &a.id {
        Some(i) => Some(vault.store.resolve(i)?),
        None => None,
    };
    let date = a.date.as_deref().map(|d| thc_core::dates::parse(d, today).map(|v| v.date().format("%Y-%m-%d").to_string())).transpose()?;
    let route = Route { view, id, query: a.query.clone(), date, back: None };
    if thc_screen::theme::theme(&a.theme).is_none() {
        return Err(usage(format!("no theme {:?} · {}", a.theme, thc_screen::theme::NAMES.join(", "))));
    }
    if let Some(name) = &a.feed {
        let host = ThcHost::new(vault);
        let args = serde_json::json!({"id": route.id, "query": route.query, "date": route.date});
        println!("{}", serde_json::to_string_pretty(&host.feed(name, &args)?)?);
        return Ok(());
    }
    if let Some(size) = &a.render {
        let (w, h) = size.split_once('x').and_then(|(w, h)| Some((w.parse::<u16>().ok()?, h.parse::<u16>().ok()?))).ok_or_else(|| usage(format!("--render takes WxH, like 120x32 (not {size:?})")))?;
        let host = ThcHost::new(vault);
        let keys: Vec<&str> = a.keys.iter().map(String::as_str).collect();
        print!("{}", thc_screen::render(&host, &route, &a.theme, w, h, &keys, a.ansi)?);
        return Ok(());
    }
    let host = ThcHost::new(vault).watching().persisting();
    let socket = a.socket.unwrap_or_else(|| default_socket(paths));
    host.app(&route, &a.theme).socket(socket).run()
}

/// The control socket of `thc screen` on this vault: one per vault, in the temp dir (or `/tmp`
/// when that's too deep to bind), since a Unix socket path must stay short.
pub fn default_socket(paths: &Paths) -> std::path::PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    paths.vault.hash(&mut h);
    thc_core::proto::screen_socket("thc-screen", &format!("{:08x}", h.finish() as u32))
}

/// One request on a screen's socket, its reply.
fn ask(socket: &std::path::Path, req: &serde_json::Value) -> Result<serde_json::Value> {
    use std::io::{BufRead, Write};
    let mut s = std::os::unix::net::UnixStream::connect(socket).with_context(|| format!("can't reach thc screen at {}", socket.display()))?;
    writeln!(s, "{req}")?;
    let mut line = String::new();
    std::io::BufReader::new(s).read_line(&mut line)?;
    Ok(serde_json::from_str(&line)?)
}

/// What `thc ui aside <target>` opens (sidebar.md §10.3): `today` or a date is a day, `@today`
/// (`inbox`, `tasks`, `log`) a view, `#tag` or any other query a list, a page id or title a page.
pub fn panel_for(store: &thc_core::store::Store, target: &str) -> Result<thc_screen::sidebar::Panel> {
    use thc_screen::sidebar::{Kind, Panel};
    let t = target.trim();
    if t == "today" {
        return Ok(Panel::new(Kind::Day, "today"));
    }
    if let Some(v) = t.strip_prefix('@').filter(|v| ["today", "inbox", "tasks", "log"].contains(v)) {
        return Ok(Panel::new(Kind::View, v));
    }
    if t.starts_with('#') || t.starts_with('@') || t.contains(':') {
        return Ok(Panel::new(Kind::Query, t));
    }
    if let Ok(id) = store.resolve(t) {
        return Ok(Panel::new(Kind::Page, &id));
    }
    if let Some(id) = store.find_root_by_title(t, false)? {
        return Ok(Panel::new(Kind::Page, &id));
    }
    if let Ok(d) = thc_core::dates::parse(t, thc_core::dates::today()) {
        return Ok(Panel::new(Kind::Day, &d.date().format("%Y-%m-%d").to_string()));
    }
    Err(thc_core::error::not_found(format!("no page or day {t:?}")))
}

/// `thc ui aside` against a running `thc screen` (no TUI on this vault): the same rules
/// (sidebar::Stack), as a store write on the screen's socket guarded by the entry's revision,
/// retried when the person changed the stack in between. An agent's panel says who opened it;
/// the person's focus never moves. `close` takes a panel away (never a pinned one); `ls` reads.
pub fn aside(paths: &Paths, actor: Option<&str>, target: Option<&str>, pin: bool, fold: bool, close: bool, ls: bool) -> Result<Option<serde_json::Value>> {
    use serde_json::json;
    use thc_screen::sidebar::Stack;
    let socket = default_socket(paths);
    if !socket.exists() {
        return Ok(None);
    }
    let by = actor.map(|a| format!("agent:{a}")).unwrap_or_else(|| "person".into());
    for _ in 0..5 {
        let cur = ask(&socket, &json!({"op": "read", "key": "sidebar"}))?;
        let mut stack = Stack::from_value(&cur["value"]);
        if ls {
            // Each panel with a title to read: its page's, else its day, view or query.
            let v = Vault::open(paths.clone(), Actor { kind: "human".into(), name: None }, "screen")?;
            let mut out = stack.to_value();
            for p in out["open"].as_array_mut().into_iter().flatten() {
                let r = p["ref"].as_str().unwrap_or_default().to_string();
                let title = v.store.node(&r).ok().flatten().map(|n| n.label()).unwrap_or(r);
                p["title"] = title.into();
            }
            return Ok(Some(json!({"sidebar": out})));
        }
        let t = target.ok_or_else(|| usage("thc ui aside <page | day | @view | #tag | query> (or --ls)"))?;
        let store = Vault::open(paths.clone(), Actor { kind: "human".into(), name: None }, "screen")?;
        let mut p = panel_for(&store.store, t)?;
        let key = p.key();
        if close {
            if stack.open.iter().any(|q| q.key() == key && q.pinned) {
                return Err(thc_core::error::invalid(format!("{t} is pinned: only the person can close it")));
            }
            stack.close(&key);
        } else {
            if actor.is_some() {
                p.opened_by = Some(by.clone());
            }
            stack.open(p);
            if let Some(q) = stack.open.iter().position(|q| q.key() == key) {
                if pin && !stack.open[q].pinned {
                    stack.toggle_pin(&key);
                }
                if fold {
                    stack.toggle_fold(&key);
                }
            }
        }
        let rev = cur["rev"].as_u64();
        let r = ask(&socket, &json!({"op": "write", "key": "sidebar", "write": "put", "value": stack.to_value(), "if_rev": rev, "by": by}))?;
        if r["ok"].as_bool() != Some(true) {
            continue;
        }
        // Kept on this device as the screen keeps it (sidebar.md §11).
        thc_screen::sidebar::save(&paths.cache, &stack);
        let beside = !stack.open.is_empty();
        ask(&socket, &json!({"op": "write", "key": "sidebar_shown", "write": "put", "value": true, "by": by}))?;
        ask(&socket, &json!({"op": "write", "key": "sidebar_beside", "write": "put", "value": beside, "by": by}))?;
        if let (Some(a), false) = (actor, close) {
            ask(&socket, &json!({"op": "toast", "text": format!("◆ {a} opened {t} beside you"), "tone": "agent", "ms": 4000}))?;
        }
        return Ok(Some(json!({"ok": true, "panel": key, "sidebar": stack.to_value(), "rev": r["rev"]})));
    }
    Err(thc_core::error::invalid("the stack kept changing under the write; try again".to_string()))
}
