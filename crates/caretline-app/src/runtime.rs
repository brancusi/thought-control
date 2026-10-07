//! The interactive runtime: terminal events in, effects out. Everything that touches the
//! clock, the terminal, files or the clipboard lives here, never in the engine.

use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use caretline::protocol::Change;
use caretline::view::Role;
use caretline::{keymap_for, Effect, Frame, Key, KeyCode, Mods, Msg, Session, State};

use crate::hub::{self, Hub, Input};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::cursor::SetCursorStyle;
use crossterm::{execute, queue};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, BeginSynchronizedUpdate,
    EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
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
        if let Ok(out) = Command::new(cmd).args(*args).stderr(Stdio::null()).output()
            && out.status.success() {
                return String::from_utf8(out.stdout).ok();
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
        // Notices only come with the status bar off; the rest (completed, restored,
        // block_left, refused and any later kinds) are for hosts that keep their own data.
        _ => None,
    }
}

fn style(role: Role) -> Style {
    match role {
        Role::Text => Style::default(),
        Role::Selection => Style::default().bg(Color::Blue).fg(Color::White),
        Role::Status => Style::default().add_modifier(Modifier::REVERSED),
        Role::StatusAccent => Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
        Role::Hang => Style::default().add_modifier(Modifier::DIM),
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
    /// The in-memory trace limit (see `Session::set_trace_limit`).
    pub trace_limit: usize,
    /// Repaint at most this many times a second (0: no cap).
    pub max_fps: u32,
    /// The frame clock to start with, in frames per second (0: off).
    pub frame_clock: u16,
    /// Print repaint statistics on exit.
    pub stats: bool,
    /// A built-in demo driving this editor (`caretline demo`).
    pub demo: Option<Box<dyn Demo>>,
}

/// What a demo does with a key.
pub enum KeyAction {
    /// Not the demo's: the keymap gets it.
    Pass,
    /// Handled by the demo.
    Consumed,
    /// Quit the editor.
    Quit,
}

/// A built-in demo (`caretline demo`): hooks into the event loop for keys of its own, a
/// status hint, timed work (a replay), a second pane and frames of its own. Everything it
/// changes goes through the session like any other input, so the trace records it.
pub trait Demo {
    /// A terminal key, before the keymap.
    fn key(&mut self, _hub: &mut Hub, _key: &Key) -> KeyAction {
        KeyAction::Pass
    }
    /// Runs after every batch of input (and once at the start).
    fn after(&mut self, _hub: &mut Hub) {}
    /// Advances anything timed. Returns when it next wants to run.
    fn poll(&mut self, _hub: &mut Hub, _now: Instant) -> Option<Instant> {
        None
    }
    /// Rows at the bottom of a `height`-row terminal for a pane showing the first other view
    /// (0: no pane).
    fn pane_rows(&self, _hub: &Hub, _height: u16) -> u16 {
        0
    }
    /// A frame to draw instead of the editor's own (a replay). Paired with `generation`.
    fn overlay(&self) -> Option<Frame> {
        None
    }
    /// Goes up whenever `overlay` would draw something new.
    fn generation(&self) -> u64 {
        0
    }
}

/// Applies messages from a demo, performing their effects (a save, a quit).
pub fn dispatch_demo(hub: &mut Hub, msgs: Vec<Msg>) -> bool {
    let mut quit = false;
    dispatch_local(hub, msgs, &mut quit, "runtime");
    quit
}

/// The editor's frame with the pane below it: the first other view, its caret drawn as a
/// selected cell (the terminal has one cursor, the person's).
pub fn compose(hub: &Hub, pane_rows: u16) -> Frame {
    compose_state(hub.session.state(), hub.session.views(), pane_rows)
}

/// [`compose`] for a state and its other views.
pub fn compose_state(state: &State, views: &[(u32, caretline::View)], pane_rows: u16) -> Frame {
    let top = caretline::view(state);
    let Some((_, v)) = views.first().filter(|_| pane_rows > 0) else { return top };
    let mut s = State::from_parts(state.doc.clone(), v.clone());
    if (s.view.viewport.width, s.view.viewport.height) != (top.width, pane_rows) {
        caretline::update(&mut s, Msg::Resize { width: top.width, height: pane_rows });
    }
    let mut pane = caretline::view(&s);
    if let Some((x, y)) = pane.cursor.take() {
        let i = y as usize * pane.width as usize + x as usize;
        if let Some(cell) = pane.cells.get_mut(i) {
            cell.role = Role::Selection;
        }
    }
    stack(top, pane)
}

/// One frame above another (the same width).
pub fn stack(mut top: Frame, bottom: Frame) -> Frame {
    top.height += bottom.height;
    top.cells.extend(bottom.cells);
    top.rows.extend(bottom.rows);
    top
}

/// What the event loop counts, for `--stats`.
#[derive(Default)]
struct Stats {
    paints: u64,
    paint_time: Duration,
    frames: u64,
    inputs: u64,
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
    let mut session = Session::new(state);
    session.set_trace_limit(opts.trace_limit);
    let mut hub = Hub::new(session, trace);
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
    let started = Instant::now();
    let mut stats = Stats::default();
    let pacing = Pacing { max_fps: opts.max_fps, frame_clock: opts.frame_clock };
    let result = event_loop(&mut hub, rx, status, pacing, &mut stats, opts.demo);
    restore_terminal(kitty, mouse);
    if opts.stats {
        let secs = started.elapsed().as_secs_f64();
        let mean = stats.paint_time.as_secs_f64() * 1e3 / stats.paints.max(1) as f64;
        eprintln!(
            "caretline: {} repaints in {secs:.1} s ({:.1}/s, {mean:.2} ms each), {} inputs, {} clock frames",
            stats.paints,
            stats.paints as f64 / secs,
            stats.inputs,
            stats.frames
        );
    }
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
            match to_key(&k).and_then(|key| keymap_for(state.doc.outline.is_some(), &key)) {
                // The keymap is pure, so a paste key carries no text; fill it in here from
                // the system clipboard so the trace records exactly what was pasted.
                Some(Msg::Paste { text: None }) => vec![Msg::Paste { text: read_system_clipboard() }],
                Some(Msg::PastePlain { text: None }) => vec![Msg::PastePlain { text: read_system_clipboard() }],
                Some(msg) => vec![msg],
                None => vec![],
            }
        }
        Event::Paste(text) => vec![Msg::Paste { text: Some(text) }],
        Event::Resize(width, height) => vec![Msg::Resize { width, height }],
        Event::Mouse(m) => {
            let text_rows = state.text_rows() as u16;
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
    let change = Change { rev: hub.session.rev(), msgs: applied, state_set: false, view: None };
    hub.changed(&change, source);
}

/// How the event loop paces itself.
#[derive(Clone, Copy)]
struct Pacing {
    max_fps: u32,
    frame_clock: u16,
}

type Term = Terminal<CrosstermBackend<BufWriter<io::Stdout>>>;

/// Sizes view 0 to the terminal less the demo's pane, and the pane's view to the pane.
fn fit_views(hub: &mut Hub, demo: &Option<Box<dyn Demo>>, term: Option<(u16, u16)>, quit: &mut bool) {
    let (Some(demo), Some((w, h))) = (demo, term) else { return };
    let rows = demo.pane_rows(hub, h).min(h.saturating_sub(2));
    let v = hub.session.state().view.viewport;
    let top = h - rows;
    if (v.width, v.height) != (w, top) {
        dispatch_local(hub, vec![Msg::Resize { width: w, height: top }], quit, "runtime");
    }
    if rows > 0
        && let Some((id, view)) = hub.session.views().first()
        && (view.viewport.width, view.viewport.height) != (w, rows)
    {
        let id = *id;
        hub.session.apply_on(id, Msg::Resize { width: w, height: rows });
    }
}

fn event_loop(
    hub: &mut Hub,
    rx: Receiver<Input>,
    status: Option<String>,
    pacing: Pacing,
    stats: &mut Stats,
    mut demo: Option<Box<dyn Demo>>,
) -> Result<(), String> {
    // One buffered write per repaint, wrapped in a synchronized update (below).
    let backend = CrosstermBackend::new(BufWriter::with_capacity(1 << 16, io::stdout()));
    let mut terminal = Terminal::new(backend).map_err(|e| format!("terminal: {e}"))?;
    let mut quit = false;
    let mut term = terminal.size().map(|s| (s.width, s.height)).ok();

    let mut start = vec![Msg::Tick { now_ms: now_ms() }];
    let v = hub.session.state().view.viewport;
    if let Some((w, h)) = term
        && (w, h) != (v.width, v.height) {
            start.push(Msg::Resize { width: w, height: h });
        }
    if let Some(text) = status {
        start.push(Msg::ShowStatus { text });
    }
    if pacing.frame_clock > 0 {
        start.push(Msg::FrameClock { fps: pacing.frame_clock });
    }
    dispatch_local(hub, start, &mut quit, "runtime");
    fit_views(hub, &demo, term, &mut quit);
    if let Some(d) = &mut demo {
        d.after(hub);
    }

    let process = |hub: &mut Hub, demo: &mut Option<Box<dyn Demo>>, term: &mut Option<(u16, u16)>, input: Input, quit: &mut bool| match input {
        Input::Connect { client, out } => hub.connect(client, out),
        Input::Disconnect { client } => hub.disconnect(client),
        Input::Line { client, line } => {
            // Pushed messages' effects are returned, not performed, unless the request
            // sets apply_effects.
            let change = hub.request(client, &line, Some(&mut |e| perform(e, quit)));
            // A replaced state keeps the terminal's size.
            if let (Some(c), Some((w, h))) = (change, *term) {
                let v = hub.session.state().view.viewport;
                if c.state_set && (w, h) != (v.width, v.height) {
                    dispatch_local(hub, vec![Msg::Resize { width: w, height: h }], quit, "runtime");
                }
            }
        }
        Input::Terminal(ev) => {
            if let Event::Resize(w, h) = ev {
                *term = Some((w, h));
                if demo.is_some() {
                    fit_views(hub, demo, *term, quit);
                    return;
                }
            }
            if let (Some(d), Event::Key(k)) = (demo.as_mut(), &ev)
                && matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                && let Some(key) = to_key(k)
            {
                match d.key(hub, &key) {
                    KeyAction::Pass => {}
                    KeyAction::Consumed => return,
                    KeyAction::Quit => {
                        *quit = true;
                        return;
                    }
                }
            }
            let msgs = terminal_msgs(hub.session.state(), ev);
            if !msgs.is_empty() {
                let mut all = vec![Msg::Tick { now_ms: now_ms() }];
                all.extend(msgs);
                dispatch_local(hub, all, quit, "terminal");
            }
        }
    };

    // Repaints are coalesced to one per refresh: time is cut into slots of `gap` on a fixed
    // grid (like a display's refresh), and a change paints at most once a slot, at once if
    // this slot hasn't painted yet, else at the next slot's start. Input is applied as it
    // arrives, so a fast client never waits for the terminal. A fixed grid, rather than a gap
    // after each paint, keeps frames that arrive at the display rate with a little jitter
    // from colliding.
    let gap = if pacing.max_fps > 0 { Duration::from_secs_f64(1.0 / pacing.max_fps as f64) } else { Duration::ZERO };
    let epoch = Instant::now();
    let slot = |t: Instant| if gap.is_zero() { 0 } else { (t - epoch).as_nanos() / gap.as_nanos() };
    let mut drawn: Option<(u64, u64)> = None;
    let mut painted_slot: Option<u128> = None;
    // The frame clock's next deadline, on an absolute schedule so it doesn't drift.
    let mut next_frame: Option<Instant> = None;
    while !quit {
        let now = Instant::now();
        match hub.session.state().view.frame_rate() {
            Some(fps) => {
                let period = Duration::from_secs_f64(1.0 / fps as f64);
                let due = *next_frame.get_or_insert(now);
                if now >= due {
                    stats.frames += 1;
                    dispatch_local(hub, vec![Msg::Frame { now_ms: now_ms() }], &mut quit, "runtime");
                    // Behind by more than a frame (a stall): skip ahead instead of bursting.
                    let next = due + period;
                    next_frame = Some(if next <= now { now + period } else { next });
                }
            }
            None => next_frame = None,
        }
        let demo_wake = demo.as_mut().and_then(|d| d.poll(hub, now));
        let key = |hub: &Hub, demo: &Option<Box<dyn Demo>>| (hub.session.rev(), demo.as_ref().map_or(0, |d| d.generation()));
        let dirty = drawn != Some(key(hub, &demo));
        let free = gap.is_zero() || painted_slot != Some(slot(now));
        if dirty && free {
            let t = Instant::now();
            let frame = match &demo {
                Some(d) => match d.overlay() {
                    Some(f) => f,
                    None => compose(hub, term.map_or(0, |(_, h)| d.pane_rows(hub, h).min(h.saturating_sub(2)))),
                },
                None => hub.session.frame(),
            };
            draw(&mut terminal, &frame)?;
            stats.paints += 1;
            stats.paint_time += t.elapsed();
            drawn = Some(key(hub, &demo));
            painted_slot = Some(slot(t));
        }
        let paint_at = painted_slot.map(|k| epoch + gap * (k + 1) as u32);
        // Sleep until input, the next repaint a pending change is waiting for, the next
        // clock frame or the demo's next step, whichever is first.
        let dirty = drawn != Some(key(hub, &demo));
        let wake = [dirty.then_some(paint_at).flatten(), next_frame, demo_wake].into_iter().flatten().min();
        let input = match wake {
            Some(t) => match rx.recv_timeout(t.saturating_duration_since(Instant::now())) {
                Ok(input) => Some(input),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(input) => Some(input),
                Err(_) => break,
            },
        };
        if let Some(input) = input {
            stats.inputs += 1;
            process(hub, &mut demo, &mut term, input, &mut quit);
            // Apply everything already queued before drawing again.
            while !quit {
                match rx.try_recv() {
                    Ok(input) => {
                        stats.inputs += 1;
                        process(hub, &mut demo, &mut term, input, &mut quit)
                    }
                    Err(_) => break,
                }
            }
            if demo.is_some() {
                fit_views(hub, &demo, term, &mut quit);
            }
            if let Some(d) = &mut demo {
                d.after(hub);
            }
        }
    }
    Ok(())
}

/// Draws a frame. ratatui diffs it against the last one and writes only the cells that
/// changed; the writes go out as one buffered flush inside a synchronized update (DEC mode
/// 2026), so a terminal that supports it shows the whole frame at once, never half of one.
fn draw(terminal: &mut Term, frame: &Frame) -> Result<(), String> {
    let _ = queue!(terminal.backend_mut(), BeginSynchronizedUpdate);
    let drawn = terminal
        .draw(|f| {
            let area = f.area();
            let buf = f.buffer_mut();
            for y in 0..frame.height.min(area.height) {
                for x in 0..frame.width.min(area.width) {
                    let cell = frame.cell(x, y);
                    if cell.symbol.is_empty() {
                        continue;
                    }
                    let w = caretline::view::display_width(&cell.symbol).max(1);
                    buf.set_stringn(x, y, &cell.symbol, w, style(cell.role));
                }
            }
            if let Some((x, y)) = frame.cursor
                && x < area.width && y < area.height {
                    f.set_cursor_position((x, y));
                }
        })
        .map(|_| ())
        .map_err(|e| format!("draw: {e}"));
    let _ = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    drawn
}
