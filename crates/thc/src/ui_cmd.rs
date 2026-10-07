//! `thc ui`: the TUI's presentation state as an API (docs/ui-protocol.md). Running TUIs
//! listen on a socket; these commands find one, read its state, push a new one, patch it,
//! send it keys and render it. `render --state` and `replay` need no running TUI at all.

use crate::cli::Cli;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::path::PathBuf;
use thc_core::error::{invalid, usage};
use thc_core::vault::{self, Paths, Vault};

pub const SPEC: crate::registry::Spec = crate::with_paths!("ui", "The TUI's state as an API: read, set, patch, send keys to and render a running TUI, or render and replay one headlessly (docs/ui-protocol.md)", UiArgs, run);

#[derive(Args, Debug)]
pub struct UiArgs {
    #[command(subcommand)]
    cmd: UiCmd,
}

#[derive(Subcommand, Debug)]
enum UiCmd {
    /// The frame a UI state draws: from a state file with no TUI running (`--state`), or from a
    /// running TUI.
    Render {
        /// The size, WxH (default: the state's own session size, else 100x30).
        size: Option<String>,
        /// A state file (`-` for stdin): render headlessly on a scratch copy of the vault.
        #[arg(long, value_name = "FILE")]
        state: Option<String>,
        /// text, ansi, html or cells (JSON rows with style runs).
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Replay a UI trace (`thc tui --trace FILE`, `trace.get`) on a scratch copy of the vault:
    /// the last frame, or every line's with --every. Pin THC_NOW for identical output.
    Replay {
        file: String,
        /// The size, WxH (default: the trace's own).
        #[arg(long)]
        size: Option<String>,
        #[arg(long, default_value = "text")]
        format: String,
        /// A frame after every line, separated by a line holding only `\f`.
        #[arg(long)]
        every: bool,
    },
    /// The UI state: a running TUI's, or with --default the state a new TUI starts with.
    State {
        /// The state a fresh TUI on this vault starts with (no TUI needed).
        #[arg(long)]
        default: bool,
    },
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
    if !paths.vault.join(vault::VAULT_MARKER).exists() {
        return Err(usage(format!("{} is not a vault (missing {}); run `thc init`", paths.vault.display(), vault::VAULT_MARKER)));
    }
    thc_core::settings::init(Some(&paths.vault));
    let copy = vault::scratch_copy(paths)?;
    let root = copy.vault.parent().map(|p| p.to_path_buf());
    let mut v = Vault::open(copy, crate::parse_actor(cli.actor.as_deref()), "tui")?;
    v.origin = Some(paths.clone());
    Ok((Scratch { root }, v))
}

fn run(cli: &Cli, paths: &Paths, a: UiArgs) -> Result<()> {
    match a.cmd {
        UiCmd::Render { size, state: Some(file), format } => {
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
        UiCmd::Render { state: None, .. } => Err(usage("render: pass --state FILE (a running TUI's frame comes with the live protocol)")),
        UiCmd::Replay { file, size, format, every } => {
            let trace = read_arg(&file)?;
            let size = size.as_deref().map(parse_size).transpose()?;
            let (_scratch, v) = scratch_vault(cli, paths)?;
            let frames = thc_tui::ui_replay(v, &trace, size, &format, every).map_err(|e| invalid(format!("{e:#}")))?;
            print!("{}", frames.join("\u{c}\n"));
            Ok(())
        }
        UiCmd::State { default: true } => {
            let (_scratch, v) = scratch_vault(cli, paths)?;
            let state = thc_tui::ui_default_state(v)?;
            println!("{}", serde_json::to_string_pretty(&json!({"rev": 0, "state": state}))?);
            Ok(())
        }
        UiCmd::State { default: false } => Err(usage("state: pass --default (a running TUI's state comes with the live protocol)")),
    }
}
