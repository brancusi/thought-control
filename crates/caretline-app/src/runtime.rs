//! The interactive runtime: terminal events in, effects out. Everything that touches the
//! clock, the terminal, files or the clipboard lives here, never in the engine.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use caretline_next::trace::TraceLine;
use caretline_next::view::Role;
use caretline_next::{keymap, update, view, Effect, Key, KeyCode, Mods, Msg, State};
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

fn now_ms() -> u64 {
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

struct Session {
    state: State,
    trace: Option<File>,
    quit: bool,
}

impl Session {
    /// Records and applies one message, then performs its effects (which may feed back
    /// more messages, recorded the same way).
    fn dispatch(&mut self, msg: Msg) {
        let mut queue = vec![msg];
        while let Some(msg) = queue.pop() {
            if let Some(trace) = &mut self.trace {
                let _ = writeln!(trace, "{}", TraceLine::Msg(msg.clone()).to_line());
                let _ = trace.flush();
            }
            for effect in update(&mut self.state, msg) {
                match effect {
                    Effect::WriteFile { path, text } => {
                        queue.insert(0, match write_file(&path, &text) {
                            Ok(()) => Msg::Saved,
                            Err(e) => Msg::SaveFailed { err: e.to_string() },
                        });
                    }
                    Effect::ClipboardSet { text } => write_system_clipboard(&text),
                    Effect::Quit => self.quit = true,
                }
            }
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

pub fn run_interactive(state: State, trace_path: Option<&str>, mouse: bool) -> Result<(), String> {
    let trace = match trace_path {
        Some(p) => {
            let mut f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .map_err(|e| format!("{p}: {e}"))?;
            writeln!(f, "{}", TraceLine::State(Box::new(state.clone())).to_line())
                .map_err(|e| format!("{p}: {e}"))?;
            Some(f)
        }
        None => None,
    };

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

    let result = event_loop(state, trace);
    restore_terminal(kitty, mouse);
    result
}

fn event_loop(state: State, trace: Option<File>) -> Result<(), String> {
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(|e| format!("terminal: {e}"))?;
    let mut session = Session { state, trace, quit: false };

    session.dispatch(Msg::Tick { now_ms: now_ms() });
    if let Ok(size) = terminal.size() {
        if (size.width, size.height) != (session.state.viewport.width, session.state.viewport.height) {
            session.dispatch(Msg::Resize { width: size.width, height: size.height });
        }
    }

    while !session.quit {
        let frame = view(&session.state);
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
            .map_err(|e| format!("draw: {e}"))?;

        let ev = event::read().map_err(|e| format!("input: {e}"))?;
        let msgs: Vec<Msg> = match ev {
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
                let text_rows = session.state.viewport.text_rows() as u16;
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
        };
        if msgs.is_empty() {
            continue;
        }
        session.dispatch(Msg::Tick { now_ms: now_ms() });
        for msg in msgs {
            session.dispatch(msg);
        }
    }
    Ok(())
}
