//! `caretline`: a terminal text editor on the caretline-next engine.
//!
//! The binary is the Elm runtime: it turns terminal events into messages, feeds them to
//! the pure `update`, performs the effects it returns (file writes, the clipboard), and
//! draws `view`'s frame. Headless modes replay messages or key scripts and print frames.

mod bench;
mod client;
mod hub;
mod runtime;

use std::fs;
use std::io::{self, Read, Write};
use std::process::ExitCode;

use caretline_next::trace::{parse_msgs, replay_trace};
use caretline_next::outline::markdown;
use caretline_next::{script_to_msgs_for, update, view, Msg, OutlineConfig, State, Viewport};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "caretline",
    version,
    about = "A terminal text editor with serializable state and exact replay",
    after_help = "Examples:\n  caretline notes.md\n  caretline --outline todo.md\n  caretline --new-state notes.md > s.json\n  caretline --state s.json --keys 'hello<cr>' --snapshot 80x24\n  caretline --state s.json --msgs m.jsonl --snapshot 80x24 --format ansi\n  caretline notes.md --trace t.jsonl   (then: caretline --replay t.jsonl --snapshot 80x24)\n\nState protocol (see docs/caretline/protocol.md):\n  caretline notes.md --listen          serve it from the running editor\n  caretline serve [FILE] [--socket P]  a headless engine on stdio or a socket\n  caretline send --latest state.get    a client (render WxH, keys S, msgs F, set-state F)\n  caretline bench                      protocol benchmarks"
)]
struct Args {
    /// The file to edit (created on first save if it doesn't exist).
    file: Option<String>,

    /// Start from this saved state (JSON) instead of a file.
    #[arg(long, value_name = "STATE.json")]
    state: Option<String>,

    /// Start from a trace: its initial state with every recorded message applied.
    #[arg(long, value_name = "TRACE.jsonl", conflicts_with_all = ["state", "file"])]
    replay: Option<String>,

    /// Apply these messages (JSON Lines or a JSON array; `-` reads stdin), headless.
    #[arg(long, value_name = "MSGS.jsonl")]
    msgs: Option<String>,

    /// Apply this key script through the keymap, headless (e.g. 'abc<cr><s-left><c-z>').
    #[arg(long, value_name = "SCRIPT", conflicts_with = "msgs")]
    keys: Option<String>,

    /// Print the rendered frame at WIDTHxHEIGHT (a resize applied after the messages).
    #[arg(long, value_name = "WxH")]
    snapshot: Option<String>,

    /// Resize to WIDTHxHEIGHT before applying messages or keys.
    #[arg(long, value_name = "WxH")]
    size: Option<String>,

    /// Snapshot format.
    #[arg(long, default_value = "text", value_parser = ["text", "ansi"])]
    format: String,

    /// Write the final state (JSON) after a headless run (`-` for stdout).
    #[arg(long, value_name = "OUT.json")]
    dump_state: Option<String>,

    /// Print the effects `update` returned during a headless run, one JSON per line.
    #[arg(long)]
    effects: bool,

    /// Interactive: append the initial state and every message to this trace (JSON Lines).
    #[arg(long, value_name = "TRACE.jsonl")]
    trace: Option<String>,

    /// Print an initial state for FILE as JSON and exit (handy for fixtures).
    #[arg(long, value_name = "FILE")]
    new_state: Option<String>,

    /// Don't capture the mouse (keeps the terminal's own text selection).
    #[arg(long)]
    no_mouse: bool,

    /// Interactive: also serve the state protocol on a Unix socket (default
    /// $TMPDIR/caretline-<pid>.sock), advertised in $TMPDIR/caretline/<pid>.json.
    /// Put FILE before this flag, or it is read as the socket path.
    #[arg(long, value_name = "PATH", num_args = 0..=1, default_missing_value = "")]
    listen: Option<String>,

    /// Hide the status bar: every row shows text (also for --new-state).
    #[arg(long)]
    no_status_bar: bool,

    /// Edit FILE as an outline: blocks, lists and tasks with their own keys (Tab, Ctrl-T,
    /// Alt-Up/Down…), and Markdown in and out. See docs/caretline/outline.md.
    #[arg(long)]
    outline: bool,

    /// With --outline: the outline layout (markers in a hang, a column per depth, folds),
    /// with plain hang glyphs.
    #[arg(long)]
    layout: bool,

    /// Interactive: keep at most this many lines in the in-memory trace `trace.get` serves
    /// (older segments are dropped first). The --trace file keeps everything.
    #[arg(long, value_name = "LINES", default_value_t = caretline_next::session::DEFAULT_TRACE_LIMIT)]
    trace_limit: usize,
}

/// `caretline serve`: a headless engine speaking the state protocol.
#[derive(Parser, Debug)]
#[command(
    name = "caretline serve",
    about = "Serve the state protocol (JSON lines) on stdin/stdout, or on a Unix socket",
    after_help = "Examples:\n  echo '{\"op\":\"hello\"}' | caretline serve notes.md\n  caretline serve --state s.json --socket /tmp/cl.sock\n\nSee docs/caretline/protocol.md."
)]
struct ServeArgs {
    /// The document to load (empty when it doesn't exist).
    file: Option<String>,
    /// Start from this saved state (JSON).
    #[arg(long, value_name = "STATE.json")]
    state: Option<String>,
    /// The viewport for a new state.
    #[arg(long, value_name = "WxH", default_value = "80x24")]
    size: String,
    /// Listen on this Unix socket (many clients) instead of stdin/stdout.
    #[arg(long, value_name = "PATH")]
    socket: Option<String>,
    /// Append the initial state and every change to this trace (JSON Lines).
    #[arg(long, value_name = "TRACE.jsonl")]
    trace: Option<String>,
    /// Don't tick to the real time before client messages (fully deterministic; clients
    /// send `now_ms` or tick messages themselves).
    #[arg(long)]
    no_clock: bool,
    /// Hide the status bar: every row shows text.
    #[arg(long)]
    no_status_bar: bool,
    /// Serve FILE as an outline document (see docs/caretline/outline.md).
    #[arg(long)]
    outline: bool,
    /// With --outline: the outline layout, with plain hang glyphs.
    #[arg(long)]
    layout: bool,
    /// Keep at most this many lines in the in-memory trace `trace.get` serves (older
    /// segments are dropped first). The --trace file keeps everything.
    #[arg(long, value_name = "LINES", default_value_t = caretline_next::session::DEFAULT_TRACE_LIMIT)]
    trace_limit: usize,
}

fn serve(args: ServeArgs) -> Result<(), String> {
    let (width, height) = parse_size(&args.size)?;
    let mut state = match &args.state {
        Some(path) => State::from_json(&read_input(path)?).map_err(|e| format!("{path}: {e}"))?,
        None => match &args.file {
            Some(path) => new_state(&read_file_or_empty(path)?, Some(path.clone()), Viewport { width, height }, args.outline || args.layout),
            None => new_state("", None, Viewport { width, height }, args.outline || args.layout),
        },
    };
    if (args.outline || args.layout) && state.doc.outline.is_none() {
        state.enable_outline(OutlineConfig::default());
    }
    if args.layout && state.view.layout.is_none() {
        state.view.layout = Some(caretline_next::OutlineLayout { hang_glyphs: true, ..Default::default() });
    }
    if let (Some(path), Some(_)) = (&args.file, &args.state) {
        state.doc.path = Some(path.clone());
    }
    if args.no_status_bar {
        state.view.config.status_bar = false;
    }
    let trace = match &args.trace {
        Some(p) => Some(
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .map_err(|e| format!("{p}: {e}"))?,
        ),
        None => None,
    };
    let mut session = caretline_next::Session::new(state);
    session.set_trace_limit(args.trace_limit);
    let mut hub = hub::Hub::new(session, trace);
    hub.clock = !args.no_clock;
    let (tx, rx) = std::sync::mpsc::channel();
    match &args.socket {
        Some(path) => {
            let _listening = hub::listen(std::path::Path::new(path), tx)?;
            hub::serve(hub, rx, false);
        }
        None => {
            let writer = hub::spawn_stdio(&tx);
            drop(tx);
            hub::serve(hub, rx, true);
            let _ = writer.join();
        }
    }
    Ok(())
}

/// A new state for a file's text: an outline document (read as Markdown) or plain text.
fn new_state(text: &str, path: Option<String>, viewport: Viewport, outline: bool) -> State {
    if outline {
        markdown::load(text, path, viewport, OutlineConfig::default())
    } else {
        State::new(text, path, viewport)
    }
}

fn parse_size(s: &str) -> Result<(u16, u16), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("bad size {s:?}: expected WIDTHxHEIGHT, like 80x24"))?;
    let w: u16 = w.trim().parse().map_err(|_| format!("bad width in {s:?}"))?;
    let h: u16 = h.trim().parse().map_err(|_| format!("bad height in {s:?}"))?;
    if w == 0 || h == 0 {
        return Err(format!("bad size {s:?}: both sides must be at least 1"));
    }
    Ok((w, h))
}

fn read_input(path: &str) -> Result<String, String> {
    if path == "-" {
        let mut s = String::new();
        io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| format!("stdin: {e}"))?;
        Ok(s)
    } else {
        fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
    }
}

/// A file's text, or empty when it doesn't exist yet.
fn read_file_or_empty(path: &str) -> Result<String, String> {
    match fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| format!("{path}: not UTF-8 text")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("{path}: {e}")),
    }
}

fn run() -> Result<(), String> {
    let argv: Vec<String> = std::env::args().collect();
    match argv.get(1).map(String::as_str) {
        Some("serve") => {
            let args = std::iter::once("caretline serve".to_string()).chain(argv[2..].iter().cloned());
            return serve(ServeArgs::parse_from(args));
        }
        Some("send") => return client::main(&argv[2..]),
        Some("bench") => return bench::main(&argv[2..]),
        _ => {}
    }
    let args = Args::parse();

    if let Some(path) = &args.new_state {
        let text = read_file_or_empty(path)?;
        let viewport = match &args.size {
            Some(size) => {
                let (width, height) = parse_size(size)?;
                Viewport { width, height }
            }
            None => Viewport { width: 80, height: 24 },
        };
        let mut state = new_state(&text, Some(path.clone()), viewport, args.outline || args.layout);
        state.view.config.status_bar = !args.no_status_bar;
        println!("{}", state.to_json());
        return Ok(());
    }

    let mut state = if let Some(trace) = &args.replay {
        replay_trace(&read_input(trace)?)?.0
    } else if let Some(path) = &args.state {
        State::from_json(&read_input(path)?).map_err(|e| format!("{path}: {e}"))?
    } else {
        let (width, height) = crossterm::terminal::size().unwrap_or((80, 24));
        let viewport = Viewport { width, height };
        match &args.file {
            Some(path) => new_state(&read_file_or_empty(path)?, Some(path.clone()), viewport, args.outline || args.layout),
            None => new_state("", None, viewport, args.outline || args.layout),
        }
    };
    if (args.outline || args.layout) && state.doc.outline.is_none() {
        state.enable_outline(OutlineConfig::default());
    }
    if args.layout && state.view.layout.is_none() {
        state.view.layout = Some(caretline_next::OutlineLayout { hang_glyphs: true, ..Default::default() });
    }
    if let (Some(path), Some(_)) = (&args.file, &args.state) {
        // A file given with a state names where the state saves.
        state.doc.path = Some(path.clone());
    }

    let headless = args.snapshot.is_some()
        || args.dump_state.is_some()
        || args.msgs.is_some()
        || args.keys.is_some()
        || args.replay.is_some();
    if !headless {
        let listen = match args.listen.as_deref() {
            None => None,
            Some("") => Some(hub::default_socket_path()?),
            Some(p) => Some(std::path::PathBuf::from(p)),
        };
        if args.no_status_bar {
            state.view.config.status_bar = false;
        }
        return runtime::run_interactive(
            state,
            runtime::Interactive {
                trace: args.trace.as_deref(),
                mouse: !args.no_mouse,
                listen,
                file: args.file.as_deref(),
                trace_limit: args.trace_limit,
            },
        );
    }

    let mut out = io::stdout().lock();
    let print_effects = args.effects;
    let mut apply = |state: &mut State, msg: Msg| -> Result<(), String> {
        for effect in update(state, msg) {
            if print_effects {
                let line = serde_json::to_string(&effect).map_err(|e| e.to_string())?;
                writeln!(out, "{line}").map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    };
    if let Some(size) = &args.size {
        let (width, height) = parse_size(size)?;
        apply(&mut state, Msg::Resize { width, height })?;
    }
    let msgs = if let Some(path) = &args.msgs {
        parse_msgs(&read_input(path)?)?
    } else if let Some(script) = &args.keys {
        script_to_msgs_for(script, state.doc.now_ms, state.doc.outline.is_some())?
    } else {
        Vec::new()
    };
    for msg in msgs {
        apply(&mut state, msg)?;
    }
    if let Some(size) = &args.snapshot {
        let (width, height) = parse_size(size)?;
        if (width, height) != (state.view.viewport.width, state.view.viewport.height) {
            apply(&mut state, Msg::Resize { width, height })?;
        }
    }
    drop(apply);
    if args.snapshot.is_some() {
        let frame = view(&state);
        let text = if args.format == "ansi" {
            frame.to_ansi()
        } else {
            frame.to_text()
        };
        write!(out, "{text}").map_err(|e| e.to_string())?;
    }
    if let Some(path) = &args.dump_state {
        let json = state.to_json();
        if path == "-" {
            writeln!(out, "{json}").map_err(|e| e.to_string())?;
        } else {
            fs::write(path, json + "\n").map_err(|e| format!("{path}: {e}"))?;
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("caretline: {e}");
            ExitCode::FAILURE
        }
    }
}
