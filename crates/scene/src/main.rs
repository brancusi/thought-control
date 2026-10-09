use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::json;
use std::path::PathBuf;
use thc_scene::runtime;

/// A terminal UI that is one serializable value: run a screen, then push whole UIs into it.
#[derive(Parser)]
#[command(name = "thc-scene")]
struct Cli {
    /// The control socket (default: $THC_SCENE_SOCKET, else thc-scene.sock in the temp dir).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a screen in this terminal (optionally starting from a UI file).
    Run {
        file: Option<PathBuf>,
        /// Reload the file whenever it changes.
        #[arg(long)]
        watch: bool,
    },
    /// Replace the running screen's whole UI with a file's (`-` reads stdin).
    Push { file: PathBuf },
    /// Replace one node, by id, with a node from a file (`-` reads stdin).
    Patch { id: String, file: PathBuf },
    /// Send keys to the running screen, as if typed.
    Key { keys: Vec<String> },
    /// Print the running screen's UI (or, with --state, everything: focus, selections, data).
    Get {
        #[arg(long)]
        state: bool,
        /// Only the frame and message counters.
        #[arg(long, conflicts_with = "state")]
        stats: bool,
    },
    /// The running screen's current frame, as text.
    Screen,
    /// Move, click or scroll the mouse on the running screen: `move|click|up|down X Y`.
    Mouse { kind: String, x: u16, y: u16 },
    /// Replace the running screen's layers with a JSON array from a file (`-` reads stdin).
    Layers { file: PathBuf },
    /// Render a UI file to text without a terminal.
    Render {
        file: PathBuf,
        #[arg(long, default_value = "100x30")]
        size: String,
        /// Keys to replay first, comma-separated (`j,j,tab`).
        #[arg(long, value_delimiter = ',')]
        keys: Vec<String>,
        /// Colours and attributes as ANSI (truecolour) escapes.
        #[arg(long)]
        ansi: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let socket = cli.socket.unwrap_or_else(runtime::default_socket);
    let reply = match cli.cmd {
        Cmd::Run { file, watch } => return runtime::run(file, watch, socket),
        Cmd::Render { file, size, keys, ansi } => {
            let (w, h) = size.split_once('x').ok_or_else(|| anyhow::anyhow!("--size is WxH"))?;
            print!("{}", runtime::render(runtime::load(&file)?, w.parse()?, h.parse()?, &keys, ansi)?);
            return Ok(());
        }
        Cmd::Push { file } => runtime::send(&socket, json!({"op": "push", "ui": runtime::load(&file)?}))?,
        Cmd::Patch { id, file } => {
            let raw = if file.as_os_str() == "-" {
                std::io::read_to_string(std::io::stdin())?
            } else {
                std::fs::read_to_string(&file)?
            };
            let node: serde_json::Value = serde_json::from_str(&raw)?;
            runtime::send(&socket, json!({"op": "patch", "id": id, "node": node}))?
        }
        Cmd::Key { keys } => {
            let mut last = json!(null);
            for key in keys {
                last = runtime::send(&socket, json!({"op": "key", "key": key}))?;
            }
            last
        }
        Cmd::Screen => {
            let r = runtime::send(&socket, json!({"op": "screen"}))?;
            print!("{}", r["screen"].as_str().unwrap_or_default());
            return Ok(());
        }
        Cmd::Mouse { kind, x, y } => runtime::send(&socket, json!({"op": "mouse", "kind": kind, "x": x, "y": y}))?,
        Cmd::Layers { file } => {
            let raw = if file.as_os_str() == "-" { std::io::read_to_string(std::io::stdin())? } else { std::fs::read_to_string(&file)? };
            let layers: serde_json::Value = serde_json::from_str(&raw)?;
            runtime::send(&socket, json!({"op": "layers", "layers": layers}))?
        }
        Cmd::Get { state, stats } => {
            let op = if stats { "stats" } else if state { "state" } else { "get" };
            runtime::send(&socket, json!({"op": op}))?
        }
    };
    println!("{}", serde_json::to_string_pretty(&reply)?);
    if reply.get("ok") == Some(&json!(false)) {
        std::process::exit(1);
    }
    Ok(())
}
