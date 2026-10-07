//! The interactive runtime: terminal events in, effects out. Everything that touches the
//! clock, the terminal, files or the clipboard lives here, never in the engine.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use caretline_next::protocol::Change;
use caretline_next::view::Role;
use caretline_next::{keymap, Effect, Frame, Key, KeyCode, Mods, Msg, Session, State};

use crate::hub::{self, Hub, Input};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::cursor::SetCursorStyle;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Terminal;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Converts a crossterm key to the engine's key type.
fn to_key(ev: &event::KeyEvent) -> Option<Key> {
    use event::KeyCode as C;
    let code = match ev.code {
        C::Char(c) => KeyCode::Char(c),
        C::Enter => KeyCode::Enter,
        C::Backspace => KeyCode::Backspace,
        C::Delete => KeyCode::Delete,
        C::Left => KeyCode::Left,
        C::Right => KeyCode::Right,
        C::Up => KeyCode::Up,
        C::Down => KeyCode::Down,
        C::Home => KeyCode::Home,
        C::End => KeyCode::End,
        C::PageUp => KeyCode::PageUp,
        C::PageDown => KeyCode::PageDown,
        C::Tab => KeyCode::Tab,
        C::BackTab => KeyCode::BackTab,
        C::Esc => KeyCode::Esc,
        _ => return None,
    };
    let m = ev.modifiers;
    Some(Key {
        code,
        mods: Mods {
            shift: m.contains(KeyModifiers::SHIFT),
            ctrl: m.contains(KeyModifiers::CONTROL),
            alt: m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::META),
            cmd: m.contains(KeyModifiers::SUPER),
        },
    })
}

/// Reads the system clipboard, if a known tool is available.
fn read_system_clipboard() -> Option<String> {
    let candidates: &[(&str, &[&str])] = &[
        ("pbpaste", &[]),
        ("wl-paste", &["--no-newline"]),
        ("xclip", &["-selection", "clipboard", "-o"]),
        ("xsel", &["--clipboard", "--output"]),
    ];
    for (cmd, args) in candidates {
        if let Ok(out) = Command::new(cmd).args(*args).stderr(Stdio::null()).output() {
            if out.status.success() {
                return String::from_utf8(out.stdout).ok();
            }
        }
    }
    None
}

/// Writes the system clipboard with a known tool, else with the OSC 52 escape sequence.
fn write_system_clipboard(text: &str) {
    let candidates: &[(&str, &[&str])] = &[
        ("pbcopy", &[]),
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for (cmd, args) in candidates {
        let child = Command::new(cmd)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if child.wait().map(|s| s.success()).unwrap_or(false) {
                return;
            }
        }
    }
    let encoded = base64(text.as_bytes());
    let mut out = io::stdout();
    let _ = write!(out, "\x1b]52;c;{encoded}\x07");
    let _ = out.flush();
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                s.push(T[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// Writes a file through a temporary sibling and a rename, so a failed write never leaves
/// a half-written file.
fn write_file(path: &str, text: &str) -> io::Result<()> {
    let target = Path::new(path);
    let dir = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let tmp = dir.join(format!(".{name}.caretline-tmp"));
    fs::write(&tmp, text)?;
    if let Ok(meta) = fs::metadata(target) {
        let _ = fs::set_permissions(&tmp, meta.permissions());
    }
    fs::rename(&tmp, target).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Performs one effect for the local user. Returns the message that reports its result.
fn perform(effect: &Effect, quit: &mut bool) -> Option<Msg> {
    match effect {
        Effect::WriteFile { path, text } => Some(match write_file(path, text) {
            Ok(()) => Msg::Saved,
            Err(e) => Msg::SaveFailed { err: e.to_string() },
        }),
        Effect::ClipboardSet { text } => {
            write_system_clipboard(text);
            None
        }
        Effect::Quit => {
            *quit = true;
            None
        }
    }
}

fn style(role: Role) -> Style {
    match role {
        Role::Text => Style::default(),
        Role::Selection => Style::default().bg(Color::Blue).fg(Color::White),
        Role::Status => Style::default().add_modifier(Modifier::REVERSED),
        Role::StatusAccent => Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
    }
}

fn restore_terminal(kitty: bool, mouse: bool) {
    let mut out = io::stdout();
    if kitty {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    if mouse {
        let _ = execute!(out, DisableMouseCapture);
    }
    let _ = execute!(
        out,
        DisableBracketedPaste,
        SetCursorStyle::DefaultUserShape,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
}

/// Options for the interactive editor.
pub struct Interactive<'a> {
    pub trace: Option<&'a str>,
    pub mouse: bool,
    /// Serve the state protocol on this socket.
    pub listen: Option<PathBuf>,
    /// The file name to advertise in the discovery file.
    pub file: Option<&'a str>,
}

pub fn run_interactive(state: State, opts: Interactive<'_>) -> Result<(), String> {
    let trace = match opts.trace {
        Some(p) => Some(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .map_err(|e| format!("{p}: {e}"))?,
        ),
        None => None,
    };
    let mut hub = Hub::new(Session::new(state), trace);
    let (tx, rx) = mpsc::channel::<Input>();

    // Bind before touching the terminal, so a bad path is an ordinary error.
    let listening = match &opts.listen {
        Some(path) => {
            let mut l = hub::listen(path, tx.clone())?;
            l.advertise(opts.file).map_err(|e| format!("discovery file: {e}"))?;
            Some(l)
        }
        None => None,
    };

    let mouse = opts.mouse;
    enable_raw_mode().map_err(|e| format!("terminal: {e}"))?;
    let kitty = supports_keyboard_enhancement().unwrap_or(false);
    let mut out = io::stdout();
    let _ = execute!(out, EnterAlternateScreen, EnableBracketedPaste, SetCursorStyle::SteadyBar);
    if mouse {
        let _ = execute!(out, EnableMouseCapture);
    }
    if kitty {
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal(kitty, mouse);
        previous_hook(info);
    }));

    // Terminal events join socket requests on one queue: one order, one trace.
    let term_tx = tx.clone();
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if term_tx.send(Input::Terminal(ev)).is_err() {
                break;
            }
        }
    });
    drop(tx);

    let status = listening.as_ref().map(|l| format!("listening on {}", l.path.display()));
    let result = event_loop(&mut hub, rx, status);
    restore_terminal(kitty, mouse);
    if let Some(l) = &listening {
        eprintln!("caretline: served the state protocol on {}", l.path.display());
    }
    drop(listening);
    result
}

/// Turns a terminal event into messages (none for events the editor ignores).
fn terminal_msgs(state: &State, ev: Event) -> Vec<Msg> {
    match ev {
        Event::Key(k) if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
            match to_key(&k).and_then(|key| keymap(&key)) {
                // The keymap is pure, so a paste key carries no text; fill it in here from
                // the system clipboard so the trace records exactly what was pasted.
                Some(Msg::Paste { text: None }) => vec![Msg::Paste { text: read_system_clipboard() }],
                Some(msg) => vec![msg],
                None => vec![],
            }
        }
        Event::Paste(text) => vec![Msg::Paste { text: Some(text) }],
        Event::Resize(width, height) => vec![Msg::Resize { width, height }],
        Event::Mouse(m) => {
            let text_rows = state.viewport.text_rows() as u16;
            let extend = m.modifiers.contains(KeyModifiers::SHIFT);
            match m.kind {
                MouseEventKind::Down(MouseButton::Left) if m.row < text_rows => {
                    vec![Msg::Click { col: m.column, row: m.row, extend }]
                }
                MouseEventKind::Drag(MouseButton::Left) => vec![Msg::Click {
                    col: m.column,
                    row: m.row.min(text_rows.saturating_sub(1)),
                    extend: true,
                }],
                MouseEventKind::ScrollUp => vec![Msg::Scroll { rows: -3 }],
                MouseEventKind::ScrollDown => vec![Msg::Scroll { rows: 3 }],
                _ => vec![],
            }
        }
        _ => vec![],
    }
}

/// Applies messages from the local user (or the runtime itself), performing their effects,
/// and tells subscribers.
fn dispatch_local(hub: &mut Hub, msgs: Vec<Msg>, quit: &mut bool, source: &str) {
    if msgs.is_empty() {
        return;
    }
    let mut applied = Vec::new();
    for msg in msgs {
        let (_, m) = hub.session.apply_with(msg, &mut |e| perform(e, quit));
        applied.extend(m);
    }
    let change = Change { rev: hub.session.rev(), msgs: applied, state_set: false };
    hub.changed(&change, source);
}

fn event_loop(hub: &mut Hub, rx: Receiver<Input>, status: Option<String>) -> Result<(), String> {
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(|e| format!("terminal: {e}"))?;
    let mut quit = false;
    let mut term = terminal.size().map(|s| (s.width, s.height)).ok();

    let mut start = vec![Msg::Tick { now_ms: now_ms() }];
    let v = hub.session.state().viewport;
    if let Some((w, h)) = term {
        if (w, h) != (v.width, v.height) {
            start.push(Msg::Resize { width: w, height: h });
        }
    }
    if let Some(text) = status {
        start.push(Msg::ShowStatus { text });
    }
    dispatch_local(hub, start, &mut quit, "runtime");

    let mut process = |hub: &mut Hub, input: Input, quit: &mut bool| match input {
        Input::Connect { client, out } => hub.connect(client, out),
        Input::Disconnect { client } => hub.disconnect(client),
        Input::Line { client, line } => {
            // Pushed messages' effects are returned, not performed, unless the request
            // sets apply_effects.
            let change = hub.request(client, &line, Some(&mut |e| perform(e, quit)));
            // A replaced state keeps the terminal's size.
            if let (Some(c), Some((w, h))) = (change, term) {
                let v = hub.session.state().viewport;
                if c.state_set && (w, h) != (v.width, v.height) {
                    dispatch_local(hub, vec![Msg::Resize { width: w, height: h }], quit, "runtime");
                }
            }
        }
        Input::Terminal(ev) => {
            if let Event::Resize(w, h) = ev {
                term = Some((w, h));
            }
            let msgs = terminal_msgs(hub.session.state(), ev);
            if !msgs.is_empty() {
                let mut all = vec![Msg::Tick { now_ms: now_ms() }];
                all.extend(msgs);
                dispatch_local(hub, all, quit, "terminal");
            }
        }
    };

    let mut drawn: Option<u64> = None;
    while !quit {
        if drawn != Some(hub.session.rev()) {
            draw(&mut terminal, &hub.session.frame())?;
            drawn = Some(hub.session.rev());
        }
        let Ok(input) = rx.recv() else { break };
        process(hub, input, &mut quit);
        // Apply everything already queued before drawing again.
        while !quit {
            match rx.try_recv() {
                Ok(input) => process(hub, input, &mut quit),
                Err(_) => break,
            }
        }
    }
    Ok(())
}

fn draw(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, frame: &Frame) -> Result<(), String> {
    terminal
        .draw(|f| {
            let area = f.area();
            let buf = f.buffer_mut();
            for y in 0..frame.height.min(area.height) {
                for x in 0..frame.width.min(area.width) {
                    let cell = frame.cell(x, y);
                    if cell.symbol.is_empty() {
                        continue;
                    }
                    let w = caretline_next::view::display_width(&cell.symbol).max(1);
                    buf.set_stringn(x, y, &cell.symbol, w, style(cell.role));
                }
            }
            if let Some((x, y)) = frame.cursor {
                if x < area.width && y < area.height {
                    f.set_cursor_position((x, y));
                }
            }
        })
        .map(|_| ())
        .map_err(|e| format!("draw: {e}"))
}
