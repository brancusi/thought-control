//! `thc ui`: the TUI's presentation state as an API (docs/ui-protocol.md). Running TUIs
//! listen on a socket; these commands find one, read its state, push a new one, patch it,
//! send it keys and render it. `render --state` and `replay` need no running TUI at all.

use crate::cli::Cli;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::path::PathBuf;
use thc_core::error::{invalid, usage};
use std::path::Path;
use thc_core::vault::{self, Paths, Vault};

pub const SPEC: crate::registry::Spec = crate::with_paths!("ui", "The TUI's state as an API: read, set, patch, send keys to and render a running TUI, or render and replay one headlessly (docs/ui-protocol.md)", UiArgs, run);

#[derive(Args, Debug)]
pub struct UiArgs {
    #[command(subcommand)]
    cmd: UiCmd,
}

#[derive(Subcommand, Debug)]
enum UiCmd {
    /// The running TUIs on this vault that answer the protocol (--all: on every vault).
    Ls {
        #[arg(long)]
        all: bool,
    },
    /// The UI state: a running TUI's, or with --default the state a new TUI starts with.
    State {
        /// The state a fresh TUI on this vault starts with (no TUI needed).
        #[arg(long)]
        default: bool,
        /// Leave out the history stack and recent lists.
        #[arg(long)]
        no_history: bool,
        /// Print the state alone, not `{rev, state}`.
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        target: Target,
    },
    /// Replace a running TUI's state with FILE (`-`: stdin). Fields left out take defaults.
    Set {
        file: String,
        /// Only if the TUI's rev is still N (else exit 4 and nothing changes).
        #[arg(long, value_name = "N")]
        if_rev: Option<u64>,
        #[command(flatten)]
        target: Target,
    },
    /// Patch a running TUI's state with a JSON merge patch (RFC 7396): '{"view":"tasks"}'.
    Patch {
        /// The patch, or @FILE.
        patch: String,
        #[arg(long, value_name = "N")]
        if_rev: Option<u64>,
        #[command(flatten)]
        target: Target,
    },
    /// Send requests: `keys SCRIPT`, `msgs FILE|-|JSON`, `render [WxH] [FORMAT]`, `state.get
    /// [no-history]`, `subscribe [WxH [FORMAT]] [state]`, `trace.get [all|since REV]`,
    /// `trace.checkpoint`, `hello`, or a raw JSON request. None: JSON requests from stdin.
    Send {
        words: Vec<String>,
        /// Print the payload: the frame for render, the state for state.get, JSON lines for
        /// trace.get.
        #[arg(long)]
        raw: bool,
        /// Have the TUI perform the effects of keys or msgs (quit, $EDITOR, the mouse mode).
        #[arg(long)]
        apply_effects: bool,
        #[command(flatten)]
        target: Target,
    },
    /// The frame a UI state draws: a running TUI's, or from a state file with no TUI running
    /// (`--state`).
    Render {
        /// The size, WxH (default: the TUI's own; 100x30 with --state).
        size: Option<String>,
        /// A state file (`-` for stdin): render headlessly on a scratch copy of the vault.
        #[arg(long, value_name = "FILE")]
        state: Option<String>,
        /// text, ansi, html or cells (JSON rows with style runs).
        #[arg(long, default_value = "text")]
        format: String,
        #[command(flatten)]
        target: Target,
    },
    /// Replay a UI trace (`thc tui --trace FILE`, `trace.get`) on a scratch copy of the vault as
    /// it was when the trace began: the last frame, or every line's with --every. Pin THC_NOW for
    /// identical output.
    Replay {
        file: String,
        /// The frames' size, WxH (default: the trace's own). Layout follows the trace's size, so
        /// this is the frame `thc ui render WxH` drew of the live TUI.
        #[arg(long)]
        size: Option<String>,
        #[arg(long, default_value = "text")]
        format: String,
        /// A frame after every line, separated by a line holding only a form feed.
        #[arg(long)]
        every: bool,
    },
}

/// Which running TUI: by pid. Left out, the only one on this vault.
#[derive(Args, Debug)]
struct Target {
    /// The TUI's pid (`thc ui ls`).
    #[arg(long, value_name = "PID")]
    session: Option<u32>,
}

fn parse_size(s: &str) -> Result<(u16, u16)> {
    let (w, h) = s.split_once('x').ok_or_else(|| usage(format!("size {s}: want WxH, like 100x30")))?;
    let (w, h): (u16, u16) = (w.parse().map_err(|_| usage(format!("size {s}: bad width")))?, h.parse().map_err(|_| usage(format!("size {s}: bad height")))?);
    if w == 0 || h == 0 {
        return Err(usage("a size is at least 1x1"));
    }
    Ok((w, h))
}

fn read_arg(path: &str) -> Result<String> {
    if path == "-" {
        return Ok(std::io::read_to_string(std::io::stdin())?);
    }
    std::fs::read_to_string(path).with_context(|| format!("can't read {path}"))
}

/// A UI state from JSON text: a bare state, or `thc ui state --json`'s `{rev, state}`.
fn state_json(text: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(text).map_err(|e| invalid(format!("the state isn't JSON: {e}")))?;
    Ok(match v.get("state") {
        Some(s) if s.is_object() && v.get("rev").is_some() => s.clone(),
        _ => v,
    })
}

/// A scratch copy of the vault, opened as the TUI would open it: a headless render or replay
/// may write (keys do), and never to the real vault.
struct Scratch {
    root: Option<PathBuf>,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(r) = self.root.take().filter(|r| r.starts_with(std::env::temp_dir())) {
            let _ = std::fs::remove_dir_all(r);
        }
    }
}

fn scratch_vault(cli: &Cli, paths: &Paths) -> Result<(Scratch, Vault)> {
    scratch_vault_at(cli, paths, None)
}

/// A scratch copy as of a frontier (a trace's start): see `vault::scratch_copy_at`.
fn scratch_vault_at(cli: &Cli, paths: &Paths, at: Option<&vault::Frontier>) -> Result<(Scratch, Vault)> {
    if !paths.vault.join(vault::VAULT_MARKER).exists() {
        return Err(usage(format!("{} is not a vault (missing {}); run `thc init`", paths.vault.display(), vault::VAULT_MARKER)));
    }
    thc_core::settings::init(Some(&paths.vault));
    let copy = vault::scratch_copy_at(paths, at)?;
    let root = copy.vault.parent().map(|p| p.to_path_buf());
    let mut v = Vault::open(copy, crate::parse_actor(cli.actor.as_deref()), "tui")?;
    v.origin = Some(paths.clone());
    Ok((Scratch { root }, v))
}

fn run(cli: &Cli, paths: &Paths, a: UiArgs) -> Result<()> {
    match a.cmd {
        UiCmd::Ls { all } => ls(cli, paths, all),
        UiCmd::Render { size, state: Some(file), format, .. } => {
            let state = state_json(&read_arg(&file)?)?;
            let size = match size {
                Some(s) => parse_size(&s)?,
                None => (100, 30),
            };
            let (_scratch, v) = scratch_vault(cli, paths)?;
            let frame = thc_tui::ui_render(v, Some(&state), size, &format).map_err(|e| invalid(format!("{e:#}")))?;
            print!("{frame}");
            Ok(())
        }
        UiCmd::Render { size, state: None, format, target } => {
            let mut req = json!({"op": "render", "format": format});
            if let Some(s) = size {
                let (w, h) = parse_size(&s)?;
                req["w"] = json!(w);
                req["h"] = json!(h);
            }
            let r = request(paths, &target, &req)?;
            match r.get("frame").and_then(Value::as_str) {
                Some(f) => print!("{f}"),
                None => println!("{r}"),
            }
            Ok(())
        }
        UiCmd::Replay { file, size, format, every } => {
            let trace = read_arg(&file)?;
            let size = size.as_deref().map(parse_size).transpose()?;
            // One scratch copy per segment, as of where it starts; all removed when done.
            let mut scratches = Vec::new();
            let open = |at: Option<&vault::Frontier>| {
                let (s, v) = scratch_vault_at(cli, paths, at)?;
                scratches.push(s);
                Ok(v)
            };
            let frames = thc_tui::ui_replay(open, &trace, size, &format, every).map_err(|e| invalid(format!("{e:#}")))?;
            drop(scratches);
            print!("{}", frames.join("\u{c}\n"));
            Ok(())
        }
        UiCmd::State { default: true, raw, .. } => {
            let (_scratch, v) = scratch_vault(cli, paths)?;
            let state = thc_tui::ui_default_state(v)?;
            let out = if raw { state } else { json!({"rev": 0, "state": state}) };
            println!("{}", serde_json::to_string_pretty(&out)?);
            Ok(())
        }
        UiCmd::State { default: false, no_history, raw, target } => {
            let r = request(paths, &target, &json!({"op": "state.get", "history": !no_history}))?;
            let out = if raw { r["state"].clone() } else { r };
            println!("{}", serde_json::to_string_pretty(&out)?);
            Ok(())
        }
        UiCmd::Set { file, if_rev, target } => {
            may_control(cli)?;
            let state = state_json(&read_arg(&file)?)?;
            let mut req = json!({"op": "state.set", "state": state});
            control_fields(cli, &mut req, if_rev);
            println!("{}", request(paths, &target, &req)?);
            Ok(())
        }
        UiCmd::Patch { patch, if_rev, target } => {
            may_control(cli)?;
            let text = match patch.strip_prefix('@') {
                Some(f) => read_arg(f)?,
                None => patch,
            };
            let patch: Value = serde_json::from_str(&text).map_err(|e| invalid(format!("the patch isn't JSON: {e}")))?;
            let mut req = json!({"op": "patch", "patch": patch});
            control_fields(cli, &mut req, if_rev);
            println!("{}", request(paths, &target, &req)?);
            Ok(())
        }
        UiCmd::Send { words, raw, apply_effects, target } => send(cli, paths, &target, words, raw, apply_effects),
    }
}

/// Read-tier agents can read a TUI but not steer it (policy.md §2).
fn may_control(cli: &Cli) -> Result<()> {
    let actor = crate::parse_actor(cli.actor.as_deref());
    let policy = thc_core::policy::Policy::load(&actor, cli.readonly)?;
    if policy.tier == thc_core::policy::Tier::Read || policy.readonly.is_some() {
        return Err(thc_core::error::ThcError::Refused {
            kind: "denied",
            message: format!("{} can read the TUI's state but not change it ({} tier)", policy.who, policy.tier.name()),
            hint: "a write or full tier in [actors] lets an agent steer the TUI".into(),
            actor: policy.who.clone(),
            tier: policy.tier.name().into(),
            verb: "ui".into(),
            config: Some(policy.source.clone()),
        }
        .into());
    }
    Ok(())
}

/// The actor (an agent's change says who in the TUI's toast) and the rev guard.
fn control_fields(cli: &Cli, req: &mut Value, if_rev: Option<u64>) {
    let actor = crate::parse_actor(cli.actor.as_deref());
    if actor.kind == "agent" {
        req["actor"] = json!(actor.name.unwrap_or_else(|| "agent".into()));
    }
    if let Some(r) = if_rev {
        req["if_rev"] = json!(r);
    }
}

/// The real path of the vault this command runs on (a project's .thc.toml, THC_VAULT…).
fn real_vault(paths: &Paths) -> PathBuf {
    std::fs::canonicalize(&paths.vault).unwrap_or_else(|_| paths.vault.clone())
}

fn on_vault(s: &Value, vault: &Path) -> bool {
    s["vault_path"].as_str().map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p))).as_deref() == Some(vault)
}

fn ls(cli: &Cli, paths: &Paths, all: bool) -> Result<()> {
    let here = real_vault(paths);
    let list: Vec<Value> = thc_tui::ui_sessions().into_iter().filter(|s| all || on_vault(s, &here)).collect();
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&json!({"sessions": list}))?);
        return Ok(());
    }
    if list.is_empty() {
        println!("no TUI is running on {}{}", vault::tilde(&paths.vault), if all { "" } else { " (--all: every vault)" });
        return Ok(());
    }
    for s in &list {
        println!("{:<8} {:<14} {:<14} {}", s["pid"], s["vault"].as_str().unwrap_or(""), s["tty"].as_str().unwrap_or("-"), s["socket"].as_str().unwrap_or(""));
    }
    Ok(())
}

/// The TUI to talk to: `--session PID`, else the only one on this vault.
fn pick(paths: &Paths, target: &Target) -> Result<Value> {
    let all = thc_tui::ui_sessions();
    if let Some(pid) = target.session {
        return all.into_iter().find(|s| s["pid"].as_u64() == Some(pid as u64)).ok_or_else(|| thc_core::error::not_found(format!("no TUI with pid {pid} answers · thc ui ls")));
    }
    let here = real_vault(paths);
    let mine: Vec<Value> = all.into_iter().filter(|s| on_vault(s, &here)).collect();
    match mine.len() {
        0 => Err(thc_core::error::not_found(format!("no TUI is running on {} · start one with thc, or pass --session", vault::tilde(&paths.vault)))),
        1 => Ok(mine.into_iter().next().unwrap()),
        _ => Err(thc_core::error::ThcError::Ambiguous { prefix: "--session".into(), candidates: mine.iter().map(|s| format!("{} ({})", s["pid"], s["tty"].as_str().unwrap_or("-"))).collect() }.into()),
    }
}

fn connect(s: &Value) -> Result<std::os::unix::net::UnixStream> {
    let sock = s["socket"].as_str().unwrap_or_default();
    std::os::unix::net::UnixStream::connect(sock).with_context(|| format!("can't reach the TUI at {sock}"))
}

/// One request, its result (an error response becomes the matching exit code).
fn request(paths: &Paths, target: &Target, req: &Value) -> Result<Value> {
    let s = pick(paths, target)?;
    let mut stream = connect(&s)?;
    let r = roundtrip(&mut stream, req)?;
    result(r)
}

fn roundtrip(stream: &mut std::os::unix::net::UnixStream, req: &Value) -> Result<Value> {
    use std::io::{BufRead, Write};
    writeln!(stream, "{req}")?;
    let mut line = String::new();
    std::io::BufReader::new(stream.try_clone()?).read_line(&mut line)?;
    serde_json::from_str(&line).with_context(|| format!("the TUI answered something that isn't JSON: {line}"))
}

fn result(r: Value) -> Result<Value> {
    if let Some(e) = r.get("error") {
        let kind = e["kind"].as_str().unwrap_or("error");
        let msg = e["message"].as_str().unwrap_or("").to_string();
        return Err(match kind {
            "stale" => thc_core::error::ThcError::Stale { message: msg, hint: "re-read with thc ui state, then decide".into(), node: String::new(), rev: None, changed: vec![] }.into(),
            "invalid" | "bad_keys" => invalid(msg),
            "trimmed" => thc_core::error::not_found(msg),
            _ => usage(format!("{kind}: {msg}")),
        });
    }
    Ok(r.get("result").cloned().unwrap_or(Value::Null))
}

/// `thc ui send`: words or raw JSON, like `caretline send`.
fn send(cli: &Cli, paths: &Paths, target: &Target, words: Vec<String>, raw: bool, apply_effects: bool) -> Result<()> {
    use std::io::{BufRead, Write};
    let s = pick(paths, target)?;
    let mut stream = connect(&s)?;
    if words.is_empty() {
        // JSON requests from stdin, one per line; every response printed.
        let mut reader = std::io::BufReader::new(stream.try_clone()?);
        let mut failed = false;
        for line in std::io::stdin().lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            writeln!(stream, "{line}")?;
            let mut resp = String::new();
            reader.read_line(&mut resp)?;
            failed |= resp.contains("\"error\"");
            print!("{resp}");
        }
        if failed {
            return Err(usage("a request failed"));
        }
        return Ok(());
    }
    let w: Vec<&str> = words.iter().map(String::as_str).collect();
    let mut req = match w.as_slice() {
        ["hello"] | ["trace.checkpoint"] | ["unsubscribe"] => json!({"op": w[0]}),
        ["state.get"] => json!({"op": "state.get"}),
        ["state.get", "no-history"] => json!({"op": "state.get", "history": false}),
        ["keys", script] => json!({"op": "keys", "keys": script}),
        ["msgs", src] => {
            let text = match *src {
                "-" => std::io::read_to_string(std::io::stdin())?,
                t if t.trim_start().starts_with('[') || t.trim_start().starts_with('{') => t.to_string(),
                f => read_arg(f)?,
            };
            let v: Value = serde_json::from_str(&text).map_err(|e| invalid(format!("msgs: not JSON: {e}")))?;
            json!({"op": "msgs", "msgs": if v.is_array() { v } else { json!([v]) }})
        }
        ["render", rest @ ..] => {
            let mut r = json!({"op": "render"});
            for x in rest {
                if x.contains('x') && x.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    let (wd, h) = parse_size(x)?;
                    r["w"] = json!(wd);
                    r["h"] = json!(h);
                } else {
                    r["format"] = json!(x);
                }
            }
            r
        }
        ["trace.get"] => json!({"op": "trace.get"}),
        ["trace.get", "all"] => json!({"op": "trace.get", "all": true}),
        ["trace.get", "since", rev] => json!({"op": "trace.get", "since_rev": rev.parse::<u64>().map_err(|_| usage("trace.get since REV"))?}),
        ["subscribe", rest @ ..] => {
            let mut r = json!({"op": "subscribe"});
            let mut frame = json!({});
            for x in rest {
                if *x == "state" {
                    r["with_state"] = json!(true);
                } else if x.contains('x') && x.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    let (wd, h) = parse_size(x)?;
                    frame["w"] = json!(wd);
                    frame["h"] = json!(h);
                } else {
                    frame["format"] = json!(x);
                }
            }
            if frame.as_object().is_some_and(|o| !o.is_empty()) {
                r["frame"] = frame;
            }
            r
        }
        [one] if one.trim_start().starts_with('{') => serde_json::from_str(one).map_err(|e| invalid(format!("not JSON: {e}")))?,
        _ => return Err(usage(format!("thc ui send: unknown request {:?} · see thc ui send --help", words.join(" ")))),
    };
    let op = req["op"].as_str().unwrap_or("").to_string();
    if matches!(op.as_str(), "keys" | "msgs" | "state.set" | "patch") {
        may_control(cli)?;
        let guard = req.get("if_rev").and_then(Value::as_u64);
        control_fields(cli, &mut req, guard);
        if apply_effects {
            req["apply_effects"] = json!(true);
        }
    }
    if op == "subscribe" {
        writeln!(stream, "{req}")?;
        let reader = std::io::BufReader::new(stream.try_clone()?);
        let mut out = std::io::stdout().lock();
        for line in reader.lines() {
            let line = line?;
            if raw {
                if let Some(f) = serde_json::from_str::<Value>(&line).ok().and_then(|v| v["frame"]["frame"].as_str().map(str::to_string)) {
                    writeln!(out, "{f}")?;
                    out.flush()?;
                    continue;
                }
            }
            writeln!(out, "{line}")?;
            out.flush()?;
        }
        return Ok(());
    }
    let r = result(roundtrip(&mut stream, &req)?)?;
    if raw {
        match op.as_str() {
            "render" => {
                if let Some(f) = r["frame"].as_str() {
                    print!("{f}");
                    return Ok(());
                }
            }
            "state.get" => {
                println!("{}", serde_json::to_string_pretty(&r["state"])?);
                return Ok(());
            }
            "trace.get" => {
                for l in r["trace"].as_array().into_iter().flatten() {
                    println!("{l}");
                }
                return Ok(());
            }
            _ => {}
        }
    }
    println!("{}", json!({"result": r}));
    Ok(())
}
