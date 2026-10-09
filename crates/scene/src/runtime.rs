//! The runtime: everything that isn't state. It owns the terminal, the control socket, child
//! processes and the watched file, turns what happens into `Msg`s for `update`, and carries out
//! the `Effect`s that come back.

use crate::model::{Source, Ui};
use crate::state::{Effect, Msg, State, narrow, update};
use crate::view;
use anyhow::{Context, Result, bail};
use ratatui::crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

/// One request line on the control socket.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Request {
    Push { ui: Ui },
    Patch { id: String, node: crate::model::Node },
    Key { key: String },
    Get,
    State,
}

enum Event {
    Msg(Msg),
    Request(Request, Sender<Value>),
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
    quit: bool,
    blocking: bool,
}

impl Driver {
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
    }

    /// Sources with `every` that are due.
    fn tick(&mut self, st: &mut State) {
        let due: Vec<String> = st
            .ui
            .data
            .iter()
            .filter(|(k, s)| {
                s.every.is_some_and(|secs| {
                    self.last_fetch.get(*k).is_none_or(|t| t.elapsed() >= Duration::from_secs_f64(secs.max(0.2)))
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
            Request::Push { ui } => Msg::Push { ui },
            Request::Patch { id, node } => Msg::Patch { id, node },
            Request::Key { key } => Msg::Key { key },
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
    let mut d = Driver { tx, in_flight: BTreeSet::new(), last_fetch: BTreeMap::new(), quit: false, blocking: false };
    d.apply(&mut st, Msg::Push { ui });
    let mut seen = file.as_deref().and_then(mtime);

    let mut term = ratatui::init();
    let res = (|| -> Result<()> {
        while !d.quit {
            term.draw(|f| view::draw(&st, f))?;
            if event::poll(Duration::from_millis(50))?
                && let TermEvent::Key(k) = event::read()?
                && k.kind == KeyEventKind::Press
                && let Some(key) = key_name(k)
            {
                d.apply(&mut st, Msg::Key { key });
            }
            drain(&rx, &mut d, &mut st);
            d.tick(&mut st);
            if watch && let Some(p) = &file {
                let now = mtime(p);
                if now != seen {
                    seen = now;
                    match load(p) {
                        Ok(ui) => d.apply(&mut st, Msg::Push { ui }),
                        Err(e) => st.status = Some(format!("{e:#}")),
                    }
                }
            }
        }
        Ok(())
    })();
    ratatui::restore();
    let _ = std::fs::remove_file(&socket);
    res
}

fn drain(rx: &Receiver<Event>, d: &mut Driver, st: &mut State) {
    while let Ok(ev) = rx.try_recv() {
        match ev {
            Event::Msg(m) => d.apply(st, m),
            Event::Request(req, reply) => {
                let _ = reply.send(d.request(st, req));
            }
        }
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

/// Render one frame as text without a terminal, after fetching sources and replaying keys.
pub fn render(ui: Ui, width: u16, height: u16, keys: &[String]) -> Result<String> {
    use ratatui::{Terminal, backend::TestBackend};
    let (tx, _rx) = mpsc::channel();
    let mut st = State::default();
    let mut d = Driver { tx, in_flight: BTreeSet::new(), last_fetch: BTreeMap::new(), quit: false, blocking: true };
    d.apply(&mut st, Msg::Push { ui });
    for k in keys {
        d.apply(&mut st, Msg::Key { key: k.clone() });
    }
    let mut term = Terminal::new(TestBackend::new(width, height))?;
    term.draw(|f| view::draw(&st, f))?;
    let buf = term.backend().buffer();
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
