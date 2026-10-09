//! The runtime: everything that isn't state. It owns the terminal, the control socket, child
//! processes and the watched file, turns what happens into `Msg`s for `update`, and carries out
//! the `Effect`s that come back.

use crate::model::{Source, Ui};
use crate::state::{Effect, Msg, State, Stats, narrow, update};
use crate::view;
use anyhow::{Context, Result, bail};
use crate::view::Hit;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
    MouseEventKind,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant, SystemTime};

/// One request line on the control socket.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Request {
    Push { ui: Ui },
    Patch { id: String, node: crate::model::Node },
    Key { key: String },
    /// Replace the layers only (callouts, rings, spotlights).
    Layers { layers: Vec<crate::model::LayerSpec> },
    /// The mouse at a cell, as the terminal would report it: `move`, `click`, `up` or `down`
    /// (the wheel).
    Mouse { kind: String, x: u16, y: u16 },
    /// The current frame, as text.
    Screen,
    /// Click a node as the mouse would: a row of it, or a tab.
    Click {
        id: String,
        #[serde(default)]
        row: Option<usize>,
        #[serde(default)]
        tab: Option<usize>,
    },
    Get,
    State,
    Stats,
}

/// Everything the loop reacts to arrives on one queue: terminal input, data, socket requests.
enum Event {
    Msg(Msg),
    Request(Request, Sender<Value>),
    Term(TermEvent),
}

pub fn default_socket() -> PathBuf {
    std::env::var_os("THC_SCENE_SOCKET").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("thc-scene.sock"))
}

pub fn load(path: &Path) -> Result<Ui> {
    let raw = if path == Path::new("-") {
        std::io::read_to_string(std::io::stdin())?
    } else {
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
    };
    serde_json::from_str(&raw).with_context(|| format!("{} is not a UI", path.display()))
}

/// Fetch a source now (blocking).
pub fn fetch(src: &Source) -> Result<Value, String> {
    if let Some(v) = &src.value {
        return narrow(&v.to_string(), src.path.as_deref());
    }
    if let Some(argv) = &src.stream {
        let mut child = spawn_stream(argv)?;
        let mut line = String::new();
        let read = BufReader::new(child.stdout.take().ok_or("no stdout")?).read_line(&mut line);
        let _ = child.kill();
        let _ = child.wait();
        read.map_err(|e| e.to_string())?;
        return narrow(&line, src.path.as_deref());
    }
    let mut cmd = match (&src.cmd, &src.shell) {
        (Some(argv), _) if !argv.is_empty() => {
            let mut c = Command::new(&argv[0]);
            c.args(&argv[1..]);
            c
        }
        (_, Some(sh)) => {
            let mut c = Command::new("sh");
            c.args(["-c", sh]);
            c
        }
        _ => return Err("a source needs cmd, shell or value".into()),
    };
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.lines().next().unwrap_or("failed").to_string());
    }
    narrow(&String::from_utf8_lossy(&out.stdout), src.path.as_deref())
}

fn spawn_stream(argv: &[String]) -> Result<Child, String> {
    let (bin, args) = argv.split_first().ok_or("stream: empty command")?;
    Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{bin}: {e}"))
}

fn exec(argv: &[String]) -> Result<(), String> {
    let (bin, args) = argv.split_first().ok_or("run: empty command")?;
    let out = Command::new(bin).args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("failed").to_string())
    }
}

/// Runs `update` and carries out its effects. Fetches and runs go to threads that report back as
/// messages; `blocking` (for headless renders) does them inline instead.
struct Driver {
    tx: Sender<Event>,
    in_flight: BTreeSet<String>,
    last_fetch: BTreeMap<String, Instant>,
    /// Running `stream` sources, with the definition each was started from.
    streams: BTreeMap<String, (Source, Child)>,
    quit: bool,
    blocking: bool,
}

impl Drop for Driver {
    fn drop(&mut self) {
        for (_, (_, mut c)) in std::mem::take(&mut self.streams) {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Driver {
    fn new(tx: Sender<Event>, blocking: bool) -> Driver {
        Driver { tx, in_flight: BTreeSet::new(), last_fetch: BTreeMap::new(), streams: BTreeMap::new(), quit: false, blocking }
    }

    fn apply(&mut self, st: &mut State, msg: Msg) {
        if let Msg::Data { name, .. } = &msg {
            self.in_flight.remove(name);
        }
        let mut queue: Vec<Effect> = update(st, msg);
        while let Some(fx) = queue.pop() {
            match fx {
                Effect::Quit => self.quit = true,
                Effect::Load(path) => match load(Path::new(&path)) {
                    Ok(ui) => queue.extend(update(st, Msg::Push { ui })),
                    Err(e) => st.status = Some(format!("{e:#}")),
                },
                Effect::Fetch(name) if self.blocking => {
                    let Some(src) = st.ui.data.get(&name) else { continue };
                    let result = fetch(src);
                    queue.extend(update(st, Msg::Data { name, result }));
                }
                Effect::Fetch(name) if st.ui.data.get(&name).is_some_and(|s| s.stream.is_some()) => {
                    self.start_stream(st, name)
                }
                Effect::Fetch(name) => self.spawn_fetch(st, name),
                Effect::Run(argv) => {
                    if self.blocking {
                        let result = exec(&argv);
                        queue.extend(update(st, Msg::Ran { argv, result }));
                    } else {
                        let tx = self.tx.clone();
                        std::thread::spawn(move || {
                            let _ = tx.send(Event::Msg(Msg::Ran { result: exec(&argv), argv }));
                        });
                    }
                }
            }
        }
        self.reap(st);
    }

    /// Start a stream source unless it's already running from the same definition.
    fn start_stream(&mut self, st: &mut State, name: String) {
        let Some(src) = st.ui.data.get(&name).cloned() else { return };
        if self.streams.get(&name).is_some_and(|(def, _)| *def == src) {
            return;
        }
        let argv = src.stream.clone().unwrap_or_default();
        let mut child = match spawn_stream(&argv) {
            Ok(c) => c,
            Err(e) => {
                update(st, Msg::Data { name, result: Err(e) });
                return;
            }
        };
        let out = child.stdout.take().expect("piped");
        let (tx, path, n) = (self.tx.clone(), src.path.clone(), name.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                let msg = Msg::Data { name: n.clone(), result: narrow(&line, path.as_deref()) };
                if tx.send(Event::Msg(msg)).is_err() {
                    return;
                }
            }
        });
        if let Some((_, mut old)) = self.streams.insert(name, (src, child)) {
            let _ = old.kill();
            let _ = old.wait();
        }
    }

    /// Stop streams the UI no longer has (or has changed).
    fn reap(&mut self, st: &State) {
        let gone: Vec<String> =
            self.streams.iter().filter(|(k, (def, _))| st.ui.data.get(*k) != Some(def)).map(|(k, _)| k.clone()).collect();
        for k in gone {
            if let Some((_, mut c)) = self.streams.remove(&k) {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }

    /// Sources with `every` that are due.
    fn tick(&mut self, st: &mut State) {
        let due: Vec<String> = st
            .ui
            .data
            .iter()
            .filter(|(k, s)| {
                s.stream.is_none()
                    && s.every.is_some_and(|secs| {
                        self.last_fetch.get(*k).is_none_or(|t| t.elapsed() >= Duration::from_secs_f64(secs.max(0.01)))
                    })
            })
            .map(|(k, _)| k.clone())
            .collect();
        for name in due {
            self.spawn_fetch(st, name);
        }
    }

    /// Fetch on a thread; the value comes back as `Msg::Data`. One fetch per source at a time.
    fn spawn_fetch(&mut self, st: &State, name: String) {
        let Some(src) = st.ui.data.get(&name).cloned() else { return };
        self.last_fetch.insert(name.clone(), Instant::now());
        if !self.in_flight.insert(name.clone()) {
            return;
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::Msg(Msg::Data { result: fetch(&src), name }));
        });
    }

    fn request(&mut self, st: &mut State, req: Request) -> Value {
        let msg = match req {
            Request::Get => return serde_json::to_value(&st.ui).unwrap_or_default(),
            Request::State => return serde_json::to_value(&*st).unwrap_or_default(),
            Request::Stats => return serde_json::to_value(&st.stats).unwrap_or_default(),
            Request::Push { ui } => Msg::Push { ui },
            Request::Patch { id, node } => Msg::Patch { id, node },
            Request::Key { key } => Msg::Key { key },
            Request::Layers { layers } => Msg::Layers { layers },
            Request::Click { id, row, tab } => Msg::Click { id, row, tab },
            // The loop answers these itself (they need the last frame).
            Request::Mouse { .. } | Request::Screen => return json!({"ok": false}),
        };
        self.apply(st, msg);
        json!({"ok": st.status.is_none(), "version": st.version, "status": st.status})
    }
}

/// Run a UI in the terminal until `quit`.
pub fn run(file: Option<PathBuf>, watch: bool, socket: PathBuf) -> Result<()> {
    let ui = match &file {
        Some(p) => load(p)?,
        None => serde_json::from_value(json!({"root": {"type": "text",
            "text": "Waiting for a UI.\n\nPush one:  thc-scene push ui.json"}}))?,
    };
    let (tx, rx) = mpsc::channel();
    serve(&socket, tx.clone())?;
    let mut st = State::default();
    let mut d = Driver::new(tx, false);
    d.apply(&mut st, Msg::Push { ui });
    let mut seen = file.as_deref().and_then(mtime);

    let mut term = ratatui::init();
    let _ = ratatui::crossterm::execute!(std::io::stdout(), EnableMouseCapture);
    let started = Instant::now();
    let res = (|| -> Result<()> {
        // Draw only when something changed, at most once per frame budget (120 fps).
        let budget = Duration::from_micros(8_333);
        // Terminal input joins the same queue as everything else.
        let input = d.tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = event::read() {
                if input.send(Event::Term(ev)).is_err() {
                    return;
                }
            }
        });
        let mut dirty = true;
        let mut last_draw = Instant::now() - budget;
        let mut window = Window::new();
        // What's under each cell in the last frame: the view's, for turning the mouse into messages.
        let mut hits: Vec<Hit> = Vec::new();
        let mut screens: Vec<Sender<Value>> = Vec::new();
        while !d.quit {
            // Animations (a pulsing ring) need time to move: tick every frame while one shows.
            let animating = st.ui.layers.iter().any(|l| l.pulse.is_some());
            if animating && last_draw.elapsed() >= budget {
                d.apply(&mut st, Msg::Tick { now_ms: started.elapsed().as_millis() as u64 });
                dirty = true;
            }
            dirty |= !screens.is_empty();
            if dirty && last_draw.elapsed() >= budget {
                let t = Instant::now();
                let mut text = None;
                term.draw(|f| {
                    hits = view::draw(&st, f);
                    if !screens.is_empty() {
                        text = Some(buffer_text(f.buffer_mut()));
                    }
                })?;
                if let Some(text) = text {
                    for s in screens.drain(..) {
                        let _ = s.send(json!({"ok": true, "screen": text}));
                    }
                }
                window.frame(t.elapsed());
                last_draw = Instant::now();
                dirty = false;
            }
            // Sleep until the next frame is due (when there's something to draw) or a while.
            let wait = if dirty || animating { budget.saturating_sub(last_draw.elapsed()) } else { Duration::from_millis(50) };
            let mut batch = Vec::new();
            if let Ok(ev) = rx.recv_timeout(wait) {
                batch.push(ev);
                batch.extend(rx.try_iter().take(10_000));
            }
            let n = apply_batch(batch, &mut d, &mut st, &hits, &mut screens);
            window.msgs += n;
            dirty |= n > 0;
            d.tick(&mut st);
            if watch && let Some(p) = &file {
                let now = mtime(p);
                if now != seen {
                    seen = now;
                    match load(p) {
                        Ok(ui) => d.apply(&mut st, Msg::Push { ui }),
                        Err(e) => st.status = Some(format!("{e:#}")),
                    }
                    dirty = true;
                }
            }
            if let Some(stats) = window.report(st.stats.frames) {
                d.apply(&mut st, Msg::Stats { stats });
                dirty = true;
            }
        }
        Ok(())
    })();
    let _ = ratatui::crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    let _ = std::fs::remove_file(&socket);
    res
}

/// Apply a batch of queued events. A source's value replaces the last one, so of several queued
/// values for one source only the newest is applied (the rest still count as received). Returns
/// how many events arrived.
fn apply_batch(batch: Vec<Event>, d: &mut Driver, st: &mut State, hits: &[Hit], screens: &mut Vec<Sender<Value>>) -> usize {
    let n = batch.len();
    let mut newest: BTreeMap<String, usize> = BTreeMap::new();
    for (i, ev) in batch.iter().enumerate() {
        if let Event::Msg(Msg::Data { name, .. }) = ev {
            newest.insert(name.clone(), i);
        }
    }
    for (i, ev) in batch.into_iter().enumerate() {
        match ev {
            Event::Msg(Msg::Data { name, .. }) if newest.get(&name) != Some(&i) => d.in_flight.remove(&name),
            Event::Msg(m) => {
                d.apply(st, m);
                true
            }
            Event::Request(Request::Screen, reply) => {
                screens.push(reply);
                true
            }
            Event::Request(Request::Mouse { kind, x, y }, reply) => {
                let kind = match kind.as_str() {
                    "click" => MouseEventKind::Down(MouseButton::Left),
                    "up" => MouseEventKind::ScrollUp,
                    "down" => MouseEventKind::ScrollDown,
                    _ => MouseEventKind::Moved,
                };
                let msg = mouse(kind, x, y, hits, st);
                let what = msg.as_ref().map(|m| serde_json::to_value(m).unwrap_or_default());
                if let Some(m) = msg {
                    d.apply(st, m);
                }
                let _ = reply.send(json!({"ok": true, "msg": what}));
                true
            }
            Event::Request(req, reply) => {
                let _ = reply.send(d.request(st, req));
                true
            }
            Event::Term(TermEvent::Key(k)) if k.kind == KeyEventKind::Press => {
                if let Some(key) = key_name(k) {
                    d.apply(st, Msg::Key { key });
                }
                true
            }
            Event::Term(TermEvent::Mouse(m)) => {
                if let Some(msg) = mouse(m.kind, m.column, m.row, hits, st) {
                    d.apply(st, msg);
                }
                true
            }
            Event::Term(_) => true,
        };
    }
    n
}

/// A mouse event as a message, by what was drawn under it last frame (the innermost hit).
fn mouse(kind: MouseEventKind, x: u16, y: u16, hits: &[Hit], st: &State) -> Option<Msg> {
    let at = |want_row: bool| {
        hits.iter().rev().find(|h| h.rect.contains(ratatui::layout::Position { x, y }) && (!want_row || h.row.is_some()))
    };
    match kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let h = at(false)?;
            Some(Msg::Click { id: h.id.clone(), row: h.row.filter(|r| *r < 1000), tab: h.tab })
        }
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
            let by = if kind == MouseEventKind::ScrollDown { 1 } else { -1 };
            let h = hits.iter().rev().find(|h| h.row.is_none() && h.rect.contains(ratatui::layout::Position { x, y }))?;
            Some(Msg::Scroll { id: h.id.clone(), by })
        }
        MouseEventKind::Moved | MouseEventKind::Drag(_) => {
            let hover = at(true).map(|h| crate::state::Hover { id: h.id.clone(), row: h.row });
            (hover != st.hover).then_some(Msg::Hover { hover })
        }
        _ => None,
    }
}

/// Counts frames and messages over half-second windows.
struct Window {
    start: Instant,
    frames: u64,
    draw: Duration,
    max: Duration,
    msgs: usize,
}

impl Window {
    fn new() -> Window {
        Window { start: Instant::now(), frames: 0, draw: Duration::ZERO, max: Duration::ZERO, msgs: 0 }
    }

    fn frame(&mut self, took: Duration) {
        self.frames += 1;
        self.draw += took;
        self.max = self.max.max(took);
    }

    /// The window's numbers once it's half a second old (and a fresh window).
    fn report(&mut self, total: u64) -> Option<Stats> {
        let secs = self.start.elapsed().as_secs_f64();
        if secs < 0.5 {
            return None;
        }
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let stats = Stats {
            fps: self.frames as f64 / secs,
            draw_ms: if self.frames > 0 { ms(self.draw) / self.frames as f64 } else { 0.0 },
            max_ms: ms(self.max),
            msgs: self.msgs as f64 / secs,
            frames: total + self.frames,
        };
        *self = Window::new();
        Some(stats)
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Listen for request lines; each gets one reply line.
fn serve(path: &Path, tx: Sender<Event>) -> Result<()> {
    if UnixStream::connect(path).is_ok() {
        bail!("a screen is already running on {} (set THC_SCENE_SOCKET for another)", path.display());
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut w = match conn.try_clone() {
                    Ok(w) => w,
                    Err(_) => return,
                };
                for line in BufReader::new(conn).lines().map_while(Result::ok) {
                    let reply = match serde_json::from_str::<Request>(&line) {
                        Ok(req) => {
                            let (rtx, rrx) = mpsc::channel();
                            if tx.send(Event::Request(req, rtx)).is_err() {
                                return;
                            }
                            rrx.recv_timeout(Duration::from_secs(5)).unwrap_or(json!({"ok": false, "status": "timed out"}))
                        }
                        Err(e) => json!({"ok": false, "status": format!("bad request: {e}")}),
                    };
                    if writeln!(w, "{reply}").is_err() {
                        return;
                    }
                }
            });
        }
    });
    Ok(())
}

/// Send one request to a running screen.
pub fn send(socket: &Path, req: Value) -> Result<Value> {
    let mut s = UnixStream::connect(socket)
        .with_context(|| format!("no screen running on {} (start one: thc-scene run)", socket.display()))?;
    writeln!(s, "{req}")?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line)?;
    Ok(serde_json::from_str(&line)?)
}

/// A frame as ANSI text: an SGR (truecolour) wherever the style changes.
fn buffer_ansi(buf: &ratatui::buffer::Buffer) -> String {
    use ratatui::style::{Color, Modifier};
    let sgr = |c: &ratatui::buffer::Cell| {
        let mut p = vec!["0".to_string()];
        for (m, code) in [(Modifier::BOLD, "1"), (Modifier::DIM, "2"), (Modifier::ITALIC, "3"), (Modifier::UNDERLINED, "4"), (Modifier::REVERSED, "7"), (Modifier::CROSSED_OUT, "9")] {
            if c.modifier.contains(m) {
                p.push(code.into());
            }
        }
        if let Color::Rgb(r, g, b) = c.fg {
            p.push(format!("38;2;{r};{g};{b}"));
        }
        if let Color::Rgb(r, g, b) = c.bg {
            p.push(format!("48;2;{r};{g};{b}"));
        }
        format!("\x1b[{}m", p.join(";"))
    };
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut last = String::new();
        for x in 0..buf.area.width {
            let c = &buf[(x, y)];
            let s = sgr(c);
            if s != last {
                out.push_str(&s);
                last = s;
            }
            out.push_str(c.symbol());
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Render one frame as text without a terminal, after fetching sources and replaying keys.
pub fn render(ui: Ui, width: u16, height: u16, keys: &[String], ansi: bool) -> Result<String> {
    use ratatui::{Terminal, backend::TestBackend};
    let (tx, _rx) = mpsc::channel();
    let mut st = State::default();
    let mut d = Driver::new(tx, true);
    d.apply(&mut st, Msg::Push { ui });
    for k in keys {
        d.apply(&mut st, Msg::Key { key: k.clone() });
    }
    let mut term = Terminal::new(TestBackend::new(width, height))?;
    term.draw(|f| {
        view::draw(&st, f);
    })?;
    let buf = term.backend().buffer();
    if ansi {
        return Ok(buffer_ansi(buf));
    }
    let mut out = String::new();
    for y in 0..height {
        let line: String = (0..width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        out.push_str(line.trim_end());
        out.push('\n');
    }
    Ok(out)
}

pub fn key_name(k: KeyEvent) -> Option<String> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    Some(match k.code {
        KeyCode::Char(c) if ctrl => format!("c-{c}"),
        KeyCode::Char(' ') => "space".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Tab => "tab".into(),
        KeyCode::BackTab => "s-tab".into(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Backspace => "bs".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        _ => return None,
    })
}
