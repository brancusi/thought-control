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
    advertise(paths, &socket);
    host.app(&route, &a.theme).socket(socket).run()
}

/// Say where this screen listens, beside the TUIs (`$TMPDIR/thc-ui/<pid>.json`, `kind:
/// "screen"`), so `thc ui` finds it the same way. Gone when the screen exits; a dead one's file
/// is pruned by the next lookup.
fn advertise(paths: &Paths, socket: &std::path::Path) {
    let dir = thc_tui::ui_discovery_dir();
    let _ = std::fs::create_dir_all(&dir);
    let pid = std::process::id();
    let file = dir.join(format!("{pid}.json"));
    let tty = unsafe {
        let p = libc::ttyname(libc::STDIN_FILENO);
        (!p.is_null()).then(|| std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned())
    };
    let vault_path = std::fs::canonicalize(&paths.vault).unwrap_or_else(|_| paths.vault.clone());
    let info = serde_json::json!({
        "pid": pid,
        "kind": "screen",
        "socket": socket,
        "vault": vault_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "vault_path": vault_path,
        "tty": tty,
        "proto": "fictty",
        "version": env!("CARGO_PKG_VERSION"),
        "started_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
    });
    if std::fs::write(&file, info.to_string() + "\n").is_ok() {
        thc_core::scratch::remove_at_exit(&file);
    }
}

// ---- `thc ui` against a screen ---------------------------------------------------------------------

/// The `thc ui` operations a screen answers, through fictty's socket.
pub const SCREEN_OPS: [&str; 5] = ["hello", "state.get", "render", "keys", "aside"];

/// One `thc ui` request (docs/ui-protocol.md) answered by a running `thc screen`, in the TUI
/// protocol's shape (`{"result"}` or `{"error": {kind, message}}`): `state.get` is fictty's
/// state, `render` its frame (as text, at its own size), `keys` a key script sent as fictty
/// keys. What a screen can't do says so (`unsupported`).
pub fn ui_request(socket: &std::path::Path, req: &serde_json::Value) -> Result<serde_json::Value> {
    use serde_json::json;
    let unsupported = |why: String| Ok(json!({"id": req["id"], "error": {"kind": "unsupported", "message": why}}));
    let op = req["op"].as_str().unwrap_or("");
    let state = |s: &std::path::Path| ask(s, &json!({"op": "state"}));
    // A screen's revision: every store write and push moves it.
    let rev = |st: &serde_json::Value| st["version"].as_u64().unwrap_or(0) + st["store"].as_object().map_or(0, |m| m.values().filter_map(|e| e["rev"].as_u64()).sum());
    let size = |st: &serde_json::Value| (st["data"]["$screen"]["value"]["width"].as_u64().unwrap_or(0), st["data"]["$screen"]["value"]["height"].as_u64().unwrap_or(0));
    let result = |r: serde_json::Value| Ok(json!({"id": req["id"], "result": r}));
    match op {
        "hello" => {
            let st = state(socket)?;
            let (w, h) = size(&st);
            result(json!({"proto": "fictty", "kind": "screen", "version": env!("CARGO_PKG_VERSION"), "rev": rev(&st), "ops": SCREEN_OPS, "size": [w, h]}))
        }
        "state.get" => {
            let st = state(socket)?;
            result(json!({"rev": rev(&st), "state": st}))
        }
        "render" => {
            let st = state(socket)?;
            let (w, h) = size(&st);
            let format = req["format"].as_str().unwrap_or("text");
            let asked = (req["w"].as_u64().unwrap_or(w), req["h"].as_u64().unwrap_or(h));
            if format != "text" {
                return unsupported(format!("thc screen draws its frame as text only (asked for {format})"));
            }
            if asked != (w, h) {
                return unsupported(format!("thc screen draws at its own size, {w}x{h} (asked for {}x{})", asked.0, asked.1));
            }
            let frame = ask(socket, &json!({"op": "screen"}))?;
            result(json!({"rev": rev(&st), "w": w, "h": h, "format": "text", "cursor": null, "frame": frame["screen"]}))
        }
        "keys" => {
            let keys = match fictty_keys(req["keys"].as_str().unwrap_or("")) {
                Ok(k) => k,
                Err(e) => return Ok(json!({"id": req["id"], "error": {"kind": "bad_keys", "message": e}})),
            };
            for k in &keys {
                ask(socket, &json!({"op": "key", "key": k}))?;
            }
            let st = state(socket)?;
            result(json!({"rev": rev(&st), "msgs": keys.iter().map(|k| json!({"msg": "key", "key": k})).collect::<Vec<_>>(), "effects": []}))
        }
        other => unsupported(format!("thc screen doesn't take `{other}`: it answers {}", SCREEN_OPS.join(", "))),
    }
}

/// A `thc ui` key script (`gg<cr>`, `<c-o>`, `<m-s>`, `<s-tab>`) as fictty's key names
/// (`g`, `enter`, `c-o`, `a-s`, `s-tab`). Pastes, clicks and fixtures aren't keys.
pub fn fictty_keys(script: &str) -> std::result::Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut rest = script;
    while let Some(c) = rest.chars().next() {
        if c != '<' {
            out.push(if c == ' ' { "space".to_string() } else { c.to_string() });
            rest = &rest[c.len_utf8()..];
            continue;
        }
        let end = rest.find('>').ok_or_else(|| format!("unclosed token: {rest}"))?;
        let inner = &rest[1..end];
        rest = &rest[end + 1..];
        // `<lt>` is the `<` key itself.
        if inner == "lt" {
            out.push("<".into());
            continue;
        }
        let parts: Vec<&str> = inner.split('-').collect();
        let (mods, base) = match parts.split_last() {
            Some((b, m)) if !b.is_empty() => (m.to_vec(), b.to_string()),
            // `<c-->`: the key is `-`.
            _ => (parts[..parts.len().saturating_sub(2)].to_vec(), "-".to_string()),
        };
        let base = match base.as_str() {
            "cr" | "enter" | "return" => "enter".to_string(),
            "esc" => "esc".into(),
            "tab" => "tab".into(),
            "bs" | "backspace" => "bs".into(),
            "space" => "space".into(),
            "del" | "delete" => "del".into(),
            "up" | "down" | "left" | "right" | "home" | "end" | "pgup" | "pgdn" => base.clone(),
            f if f.starts_with('f') && f[1..].parse::<u8>().is_ok() => base.clone(),
            s if s.chars().count() == 1 => s.to_string(),
            _ => return Err(format!("thc screen has no key <{inner}>")),
        };
        let mut name = String::new();
        // fictty's order: c- a- d- s-.
        for (m, p) in [("c", "c-"), ("m", "a-"), ("a", "a-"), ("d", "d-"), ("s", "s-")] {
            if mods.contains(&m) && !name.contains(p) {
                name.push_str(p);
            }
        }
        if mods.iter().any(|m| !["c", "m", "a", "d", "s"].contains(m)) {
            return Err(format!("thc screen has no key <{inner}>"));
        }
        name.push_str(&base);
        out.push(name);
    }
    Ok(out)
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

/// `thc ui aside` against a running `thc screen` (no TUI on this vault): the same rules
/// (thc_screen::sidebar::aside_requests), as store writes on the screen's socket, the stack's
/// guarded by its revision and retried when the person changed it in between. `ls` reads.
#[allow(clippy::too_many_arguments)]
pub fn aside(paths: &Paths, socket: &std::path::Path, actor: Option<&str>, target: Option<&str>, pin: bool, fold: bool, close: bool, ls: bool) -> Result<Option<serde_json::Value>> {
    use serde_json::json;
    use thc_screen::sidebar::Stack;
    let socket = socket.to_path_buf();
    let vault = Vault::open(paths.clone(), Actor { kind: "human".into(), name: None }, "screen")?;
    for _ in 0..5 {
        let cur = ask(&socket, &json!({"op": "read", "key": "sidebar"}))?;
        if ls {
            // Each panel with a title to read: its page's, else its day, view or query.
            let mut out = Stack::from_value(&cur["value"]).to_value();
            for p in out["open"].as_array_mut().into_iter().flatten() {
                let r = p["ref"].as_str().unwrap_or_default().to_string();
                let title = vault.store.node(&r).ok().flatten().map(|n| n.label()).unwrap_or(r);
                p["title"] = title.into();
            }
            return Ok(Some(json!({"sidebar": out})));
        }
        let t = target.ok_or_else(|| usage("thc ui aside <page | day | @view | #tag | query> (or --ls)"))?;
        let (key, stack, reqs) = thc_screen::sidebar::aside_requests(&vault.store, &cur, t, actor, pin, fold, close, thc_core::dates::today())?;
        let r = ask(&socket, &reqs[0])?;
        if r["ok"].as_bool() != Some(true) {
            continue;
        }
        for q in &reqs[1..] {
            ask(&socket, q)?;
        }
        // Kept on this device as the screen keeps it (sidebar.md §11).
        thc_screen::sidebar::save(&paths.cache, &stack);
        return Ok(Some(json!({"ok": true, "panel": key, "sidebar": stack.to_value(), "rev": r["rev"]})));
    }
    Err(thc_core::error::invalid("the stack kept changing under the write; try again".to_string()))
}

#[cfg(test)]
mod tests {
    /// `thc ui send keys` scripts as fictty's key names.
    #[test]
    fn key_scripts_become_fictty_keys() {
        let k = |s: &str| super::fictty_keys(s).unwrap();
        assert_eq!(k("gg<cr>"), vec!["g", "g", "enter"]);
        assert_eq!(k("<c-o><m-s><s-tab><esc> a"), vec!["c-o", "a-s", "s-tab", "esc", "space", "a"]);
        assert_eq!(k("<c-m-s-k><d-z><up><f2><lt>"), vec!["c-a-s-k", "d-z", "up", "f2", "<"]);
        assert!(super::fictty_keys("<sclick:1,2>").is_err(), "a click isn't a key");
        assert!(super::fictty_keys("<cr").is_err());
    }
}
