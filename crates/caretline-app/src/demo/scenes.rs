//! `caretline demo scenes`: ASCII animations (a warp field with the CARETLINE logo, a
//! donut, a cube, a tunnel, plasma, fire) pushed into the real editor with the protocol's
//! `frame` op, from a client on the editor's own socket, paced against absolute deadlines.
//!
//! `--bench` plays them into an editor that is already running instead, at several target
//! rates, and reports the frames per second achieved and how late frames left:
//!
//! ```sh
//! caretline notes.md --listen                                  # in one terminal
//! caretline demo scenes --bench                                # in another
//! caretline demo scenes --bench --fps 120 --scene donut,plasma --seconds 5
//! ```
//!
//! Pacing: frame `k` is due at `start + k / fps`; the client sleeps until about 1 ms before
//! that and spins the rest, so it neither drifts nor oversleeps.

use std::f64::consts::PI;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use caretline_next::{Key, KeyCode, Session, State, Viewport};
use serde_json::{json, Value};

use super::agent::Conn;
use super::DemoArgs;
use crate::hub::Hub;
use crate::runtime::{self, Demo, KeyAction};

/// A frame: rows of chars and the cells to highlight.
struct Grid {
    w: usize,
    h: usize,
    cells: Vec<char>,
    hot: Vec<bool>,
}

impl Grid {
    fn new(w: usize, h: usize) -> Grid {
        Grid { w, h, cells: vec![' '; w * h], hot: vec![false; w * h] }
    }

    fn put(&mut self, x: i64, y: i64, c: char, hot: bool) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            let i = y as usize * self.w + x as usize;
            self.cells[i] = c;
            self.hot[i] |= hot;
        }
    }

    /// The `frame` request's body after `{"op":"frame",`: the text and the highlights as
    /// char ranges (rows are `w` chars and a line break).
    fn body(&self) -> String {
        let (text, ranges) = self.parts();
        format!(r#""text":{},"highlights":{}}}"#, Value::from(text), json!(ranges))
    }

    /// The frame's text and its highlights as char ranges.
    fn parts(&self) -> (String, Vec<[usize; 2]>) {
        let mut text = String::with_capacity((self.w + 1) * self.h);
        let mut ranges = Vec::new();
        for y in 0..self.h {
            let row = &self.cells[y * self.w..(y + 1) * self.w];
            text.extend(row.iter());
            if y + 1 < self.h {
                text.push('\n');
            }
            let mut run = None;
            for x in 0..=self.w {
                let on = x < self.w && self.hot[y * self.w + x];
                match (on, run) {
                    (true, None) => run = Some(x),
                    (false, Some(a)) => {
                        let base = y * (self.w + 1);
                        ranges.push([base + a, base + x]);
                        run = None;
                    }
                    _ => {}
                }
            }
        }
        (text, ranges)
    }
}

/// A small deterministic random source (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.unit()
    }
}

/// A scene: a function from time (seconds) to a frame.
trait Scene {
    fn frame(&mut self, t: f64, w: usize, h: usize) -> Grid;
}

/// The spinning torus: a z-buffered, shaded donut; the brightest cells highlighted.
struct Donut;

impl Scene for Donut {
    fn frame(&mut self, t: f64, w: usize, h: usize) -> Grid {
        let (wf, hf) = (w as f64, h as f64);
        let mut g = Grid::new(w, h);
        let mut zb = vec![0.0f64; w * h];
        let (a, b) = (t * 1.7, t * 0.9);
        let (ca, sa, cb, sb) = (a.cos(), a.sin(), b.cos(), b.sin());
        let ramp: Vec<char> = ".,-~:;=!*#$@".chars().collect();
        for j in (0..628).step_by(9) {
            let (ct, st) = ((j as f64 / 100.0).cos(), (j as f64 / 100.0).sin());
            for i in (0..628).step_by(3) {
                let (sp, cp) = ((i as f64 / 100.0).sin(), (i as f64 / 100.0).cos());
                let hh = ct + 2.0;
                let d = 1.0 / (sp * hh * sa + st * ca + 5.0);
                let tt = sp * hh * ca - st * sa;
                let x = (wf / 2.0 + wf * 0.57 * d * (cp * hh * cb - tt * sb)) as i64;
                let y = (hf / 2.0 + hf * 0.5 * d * (cp * hh * sb + tt * cb)) as i64;
                let l = (8.0 * ((st * sa - sp * ct * ca) * cb - sp * ct * sa - st * ca - cp * ct * sb)) as i64;
                if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                    let k = y as usize * w + x as usize;
                    if d > zb[k] {
                        zb[k] = d;
                        g.cells[k] = ramp[l.clamp(0, 11) as usize];
                        g.hot[k] = l >= 8;
                    }
                }
            }
        }
        g
    }
}

/// A rotating wireframe cube; near edges and the corners highlighted.
struct Cube;

impl Scene for Cube {
    fn frame(&mut self, t: f64, w: usize, h: usize) -> Grid {
        let (wf, hf) = (w as f64, h as f64);
        let mut g = Grid::new(w, h);
        let mut v = Vec::new();
        for x in [-1.0, 1.0] {
            for y in [-1.0, 1.0] {
                for z in [-1.0, 1.0] {
                    v.push([x, y, z]);
                }
            }
        }
        let (ax, ay, az) = (t * 0.9, t * 1.3, t * 0.4);
        let rot = |[x, y, z]: [f64; 3]| {
            let (y, z) = (y * ax.cos() - z * ax.sin(), y * ax.sin() + z * ax.cos());
            let (x, z) = (x * ay.cos() + z * ay.sin(), -x * ay.sin() + z * ay.cos());
            let (x, y) = (x * az.cos() - y * az.sin(), x * az.sin() + y * az.cos());
            let f = 3.2 / (z + 4.0);
            [wf / 2.0 + x * f * wf * 0.28, hf / 2.0 + y * f * hf * 0.25, z]
        };
        let p: Vec<[f64; 3]> = v.iter().map(|&q| rot(q)).collect();
        for a in 0..8 {
            for b in a + 1..8 {
                if (0..3).filter(|&k| v[a][k] != v[b][k]).count() != 1 {
                    continue;
                }
                let ([x0, y0, z0], [x1, y1, z1]) = (p[a], p[b]);
                let n = (x1 - x0).abs().max((y1 - y0).abs()) as usize + 1;
                for k in 0..=n {
                    let u = k as f64 / n as f64;
                    let z = z0 + (z1 - z0) * u;
                    let c = if z < -0.3 { '@' } else if z < 0.4 { '#' } else { '+' };
                    g.put((x0 + (x1 - x0) * u) as i64, (y0 + (y1 - y0) * u) as i64, c, z < -0.3);
                }
            }
        }
        for [x, y, _] in p {
            g.put(x as i64, y as i64, 'O', true);
        }
        g
    }
}

/// A texture tunnel (XOR pattern in polar coordinates) flying forward.
struct Tunnel;

impl Scene for Tunnel {
    fn frame(&mut self, t: f64, w: usize, h: usize) -> Grid {
        let (wf, hf) = (w as f64, h as f64);
        let mut g = Grid::new(w, h);
        let ramp: Vec<char> = " .:-=+*#%@".chars().collect();
        for y in 0..h {
            for x in 0..w {
                let dx = (x as f64 - wf / 2.0) / wf * 2.0;
                let dy = (y as f64 - hf / 2.0) / hf * 2.0 * 0.55;
                let r = dx.hypot(dy) + 1e-6;
                let a = dy.atan2(dx);
                let u = 0.6 / r + t * 3.0;
                let v = a * 4.0 / PI + t * 0.7;
                let c = ((u * 4.0) as i64 ^ (v * 4.0) as i64) & 7;
                let k = y * w + x;
                if r > 0.04 {
                    g.cells[k] = ramp[(c + (4.0 * (1.0 - r.min(1.0))) as i64).min(9) as usize];
                }
                g.hot[k] = c == 7 && r > 0.08;
            }
        }
        g
    }
}

/// The classic plasma: summed sines over the plane and time.
struct Plasma;

impl Scene for Plasma {
    fn frame(&mut self, t: f64, w: usize, h: usize) -> Grid {
        let mut g = Grid::new(w, h);
        let ramp: Vec<char> = " .-:=+*%#@".chars().collect();
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f64 / 8.0, y as f64 / 4.0);
                let cx = fx + 0.5 * (t * 0.7).sin() * 10.0;
                let cy = fy + 0.5 * (t * 0.5).cos() * 10.0;
                let v = (fx + t).sin()
                    + ((fy + t) * 0.5).sin()
                    + ((fx + fy + t) * 0.5).sin()
                    + ((cx * cx + cy * cy).sqrt() + t).sin();
                let n = (v + 4.0) / 8.0; // 0..1
                let k = y * w + x;
                g.cells[k] = ramp[((n * 10.0) as usize).min(9)];
                g.hot[k] = n > 0.78;
            }
        }
        g
    }
}

/// Cellular fire rising from a random bottom row.
struct Fire {
    buf: Vec<u32>,
    rng: Rng,
}

impl Scene for Fire {
    fn frame(&mut self, _t: f64, w: usize, h: usize) -> Grid {
        let rows = h + 2;
        if self.buf.len() != w * rows {
            self.buf = vec![0; w * rows];
        }
        for x in 0..w {
            self.buf[(rows - 1) * w + x] = [0, 36, 36, 36][(self.rng.next() % 4) as usize];
        }
        for y in 0..rows - 1 {
            for x in 0..w {
                let below = (y + 1) * w;
                let l = self.buf[below + (x + w - 1) % w];
                let r = self.buf[below + (x + 1) % w];
                let c = self.buf[below + x];
                let v = (l + c + r + c) / 4;
                self.buf[y * w + x] = v.saturating_sub([0, 0, 1][(self.rng.next() % 3) as usize]);
            }
        }
        let ramp: Vec<char> = " .,:;+*%#@".chars().collect();
        let mut g = Grid::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = self.buf[y * w + x];
                g.cells[y * w + x] = ramp[((v / 4) as usize).min(9)];
                g.hot[y * w + x] = v > 30;
            }
        }
        g
    }
}

/// A star field rushing at the viewer, with a title in the middle.
struct Warp {
    stars: Vec<[f64; 3]>,
}

impl Scene for Warp {
    fn frame(&mut self, t: f64, w: usize, h: usize) -> Grid {
        let (wf, hf) = (w as f64, h as f64);
        let mut g = Grid::new(w, h);
        for &[sx, sy, sz] in &self.stars {
            let z = (sz - t * 0.35).rem_euclid(1.0) + 0.02;
            let x = (wf / 2.0 + sx / z * wf * 0.25) as i64;
            let y = (hf / 2.0 + sy / z * hf * 0.25) as i64;
            let c = if z < 0.15 { '@' } else if z < 0.35 { '*' } else if z < 0.6 { '+' } else { '.' };
            g.put(x, y, c, z < 0.2);
        }
        let title = "C A R E T L I N E";
        let x0 = (w / 2).saturating_sub(title.len() / 2) as i64;
        for (k, c) in title.chars().enumerate() {
            g.put(x0 + k as i64, (h / 2) as i64, c, true);
        }
        g
    }
}

fn scene(name: &str) -> Option<Box<dyn Scene>> {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    Some(match name {
        "donut" => Box::new(Donut),
        "cube" => Box::new(Cube),
        "tunnel" => Box::new(Tunnel),
        "plasma" => Box::new(Plasma),
        "fire" => Box::new(Fire { buf: Vec::new(), rng }),
        "warp" => Box::new(Warp {
            stars: (0..900).map(|_| [rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(0.05, 1.0)]).collect(),
        }),
        _ => return None,
    })
}

const ALL: [&str; 6] = ["warp", "donut", "cube", "tunnel", "plasma", "fire"];

/// One playback's result.
struct Run {
    frames: u64,
    /// From the first frame sent to the last (paced), or to the end (unthrottled).
    secs: f64,
    paced: bool,
    /// How late frames left against their deadlines, sorted (paced runs only).
    late: Vec<Duration>,
}

impl Run {
    /// Frames per second: frame intervals over their time when paced (the first frame
    /// starts the clock), frames over the run when unthrottled.
    fn fps(&self) -> f64 {
        let n = if self.paced { self.frames.saturating_sub(1) } else { self.frames };
        n as f64 / self.secs
    }
    fn late_pct(&self, p: usize) -> Duration {
        if self.late.is_empty() {
            return Duration::ZERO;
        }
        self.late[(self.late.len() * p / 100).min(self.late.len() - 1)]
    }
}

/// Waits until `deadline`: sleeps until `spin` before it (about 1 ms), then spins. The
/// sleep goes in steps of at most 2 ms: an OS may let a timer fire late by a share of its
/// length (macOS's timer leeway), and a long sleep would overshoot the spin.
fn wait_until(deadline: Instant, spin: Duration) {
    loop {
        let now = Instant::now();
        if deadline <= now + spin {
            break;
        }
        std::thread::sleep((deadline - now - spin).min(Duration::from_millis(2)));
    }
    while Instant::now() < deadline {
        std::hint::spin_loop();
    }
}

/// Plays `bodies` round and round for `seconds` at `fps` (0: unthrottled).
fn play(c: &mut Conn, bodies: &[String], seconds: f64, fps: u32, spin: Duration, tag: &str) -> Result<Run, String> {
    let start = Instant::now();
    let end = start + Duration::from_secs_f64(seconds);
    let period = (fps > 0).then(|| Duration::from_secs_f64(1.0 / fps as f64));
    let mut late = Vec::new();
    let mut k: u64 = 0;
    let mut first: Option<Instant> = None;
    let mut last = start;
    let mut status = Value::from(tag.to_string());
    loop {
        if let Some(p) = period {
            let due = start + p * k as u32;
            if due >= end {
                break;
            }
            wait_until(due, spin);
            late.push(Instant::now() - due);
        } else if Instant::now() >= end {
            break;
        }
        if k % 30 == 29 {
            let el = start.elapsed().as_secs_f64();
            status = Value::from(format!("{tag} · {:.1} frames/s · frame {}", (k + 1) as f64 / el, k + 1));
        }
        let req = format!(r#"{{"op":"frame","status":{status},{}"#, bodies[k as usize % bodies.len()]);
        last = Instant::now();
        first.get_or_insert(last);
        c.send(&req)?;
        k += 1;
    }
    late.sort();
    let secs = match period {
        Some(_) => (last - first.unwrap_or(last)).as_secs_f64(),
        None => start.elapsed().as_secs_f64(),
    };
    Ok(Run { frames: k, secs: secs.max(1e-9), paced: period.is_some(), late })
}

fn ms(d: Duration) -> String {
    format!("{:.0} µs", d.as_secs_f64() * 1e6)
}


/// The scenes to play: `--scene`, or all of them, warp first.
fn names(args: &DemoArgs) -> Result<Vec<&'static str>, String> {
    let Some(list) = &args.scene else { return Ok(ALL.to_vec()) };
    let mut out = Vec::new();
    for n in list.split(',').map(str::trim).filter(|n| !n.is_empty() && *n != "all") {
        out.push(*ALL.iter().find(|a| **a == n).ok_or_else(|| format!("unknown scene {n:?}; scenes: {}", ALL.join(", ")))?);
    }
    Ok(if out.is_empty() { ALL.to_vec() } else { out })
}

/// The status line under a scene.
fn status(name: &str, i: usize, n: usize, fps: f64) -> String {
    format!("{name} · {}/{n} · {fps:.0} fps · ←/→ scene · q quits", i + 1)
}

/// Which scene the person picked, shared with the player.
struct Control {
    index: AtomicUsize,
    /// Set when the person picks one, so the player restarts its clock.
    picked: AtomicBool,
}

struct ScenesDemo {
    control: Arc<Control>,
    count: usize,
}

impl Demo for ScenesDemo {
    fn key(&mut self, _hub: &mut Hub, key: &Key) -> KeyAction {
        let m = key.mods;
        let step = |d: isize| {
            let i = self.control.index.load(Ordering::Relaxed) as isize;
            self.control.index.store((i + d).rem_euclid(self.count as isize) as usize, Ordering::Relaxed);
            self.control.picked.store(true, Ordering::Relaxed);
        };
        match key.code {
            KeyCode::Esc => return KeyAction::Quit,
            KeyCode::Char(c) if m.ctrl && matches!(c.to_ascii_lowercase(), 'c' | 'q') => return KeyAction::Quit,
            KeyCode::Char('q') if !m.ctrl && !m.alt && !m.cmd => return KeyAction::Quit,
            KeyCode::Right | KeyCode::Tab | KeyCode::Char(' ') | KeyCode::Char('n') => step(1),
            KeyCode::Left | KeyCode::BackTab | KeyCode::Char('p') => step(-1),
            _ => {}
        }
        KeyAction::Consumed
    }
}

pub fn main(args: &DemoArgs) -> Result<(), String> {
    if args.bench {
        return bench(args);
    }
    let names = names(args)?;
    let fps: u32 = match &args.fps {
        Some(f) => f.trim().parse().map_err(|_| format!("bad --fps {f:?}: one rate, like 60"))?,
        None => 60,
    };
    let fps = fps.clamp(1, 240);
    let seconds = args.seconds.unwrap_or(8.0).max(0.5);
    if let Some(size) = &args.snapshot {
        let (w, h) = crate::parse_size(size)?;
        let frame = first_frame(names[0], names.len(), fps, w, h);
        print!("{}", if args.format == "ansi" { frame.to_ansi() } else { frame.to_text() });
        return Ok(());
    }
    let socket = crate::hub::default_socket_path()?;
    let control = Arc::new(Control { index: AtomicUsize::new(0), picked: AtomicBool::new(false) });
    {
        let (socket, control, names) = (socket.clone(), control.clone(), names.clone());
        std::thread::spawn(move || {
            let _ = play_live(&socket, &names, fps, seconds, &control);
        });
    }
    let (width, height) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut state = State::new("", None, Viewport { width, height });
    state.view.status = Some("warming up…".into());
    let demo = ScenesDemo { control, count: names.len() };
    runtime::run_interactive(
        state,
        runtime::Interactive {
            trace: None,
            mouse: false,
            listen: Some(socket),
            file: None,
            // Every frame is a new trace segment; keep only a little.
            trace_limit: 64,
            max_fps: 120,
            frame_clock: 0,
            stats: false,
            demo: Some(Box::new(demo)),
        },
    )
}

/// The first scene a moment in, as the editor draws it at `w`x`h`.
fn first_frame(name: &str, count: usize, fps: u32, w: u16, h: u16) -> caretline_next::Frame {
    let mut session = Session::new(State::new("", None, Viewport { width: w, height: h }));
    let rows = (h as usize).saturating_sub(1).max(1);
    let mut sc = scene(name).expect("a known scene");
    let (text, ranges) = sc.frame(2.0, (w as usize).saturating_sub(1).max(1), rows).parts();
    let ranges: Vec<(usize, usize)> = ranges.iter().map(|r| (r[0], r[1])).collect();
    session.push_frame(&text, &ranges, None, Some(status(name, 0, count, fps as f64)));
    session.frame()
}

/// Plays the scenes into the editor at `socket` until it goes away: each for `seconds`, or
/// until the person picks another.
fn play_live(socket: &std::path::Path, names: &[&str], fps: u32, seconds: f64, control: &Control) -> Result<(), String> {
    let mut c = Conn::connect_within(socket, Duration::from_secs(5))?;
    let period = Duration::from_secs_f64(1.0 / fps as f64);
    let size = |c: &mut Conn| -> Result<(usize, usize), String> {
        let v = c.ask(json!({"op": "view.list"}))?;
        // One column short of the view: a row as wide as the view would soft-wrap.
        let w = (v["views"][0]["w"].as_u64().unwrap_or(80) as usize).saturating_sub(1).max(1);
        let h = v["views"][0]["h"].as_u64().unwrap_or(24).saturating_sub(1).max(1) as usize;
        Ok((w, h))
    };
    let (mut w, mut h) = size(&mut c)?;
    let mut current = usize::MAX;
    let mut sc: Option<Box<dyn Scene>> = None;
    let mut started = Instant::now();
    let mut k: u64 = 0;
    let mut clock = Instant::now();
    let mut rate = fps as f64;
    let mut window = (Instant::now(), 0u32);
    loop {
        let i = control.index.load(Ordering::Relaxed) % names.len();
        if i != current || control.picked.swap(false, Ordering::Relaxed) {
            current = i;
            sc = scene(names[i]);
            started = Instant::now();
        }
        if k.is_multiple_of(30) {
            (w, h) = size(&mut c)?;
        }
        let t = started.elapsed().as_secs_f64();
        if t > seconds {
            control.index.store((i + 1) % names.len(), Ordering::Relaxed);
            continue;
        }
        let Some(s) = sc.as_mut() else { return Err("no scene".into()) };
        let body = s.frame(t, w, h).body();
        let st = Value::from(status(names[i], i, names.len(), rate));
        let req = format!(r#"{{"op":"frame","status":{st},{body}"#);
        let due = clock + period * (k as u32);
        if Instant::now() > due + period * 4 {
            // Fell behind (a stall): start the schedule again rather than burst.
            clock = Instant::now();
            k = 0;
        } else {
            wait_until(due, Duration::from_millis(1));
        }
        c.send(&req)?;
        k += 1;
        window.1 += 1;
        let el = window.0.elapsed().as_secs_f64();
        if el >= 1.0 {
            rate = window.1 as f64 / el;
            window = (Instant::now(), 0);
        }
    }
}

/// The newest live editor in `$TMPDIR/caretline/*.json` that answers.
fn discover() -> Result<PathBuf, String> {
    let dir = crate::hub::discovery_dir();
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&dir)
        .map_err(|_| "no live caretline: start one with `caretline FILE --listen`".to_string())?
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    found.sort();
    for (_, f) in found.iter().rev() {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let Ok(info) = serde_json::from_str::<Value>(&text) else { continue };
        if let Some(sock) = info["socket"].as_str()
            && std::os::unix::net::UnixStream::connect(sock).is_ok()
        {
            return Ok(sock.into());
        }
    }
    Err("no live caretline: start one with `caretline FILE --listen`".into())
}

/// `--bench`: every scene at every target rate into a running editor, with the numbers.
fn bench(args: &DemoArgs) -> Result<(), String> {
    let path = match &args.socket {
        Some(p) => p.clone(),
        None => discover()?,
    };
    let rates: Vec<u32> = match &args.fps {
        Some(f) => f.split(',').map(|f| f.trim().parse().map_err(|_| format!("bad fps {f:?}"))).collect::<Result<_, _>>()?,
        None => vec![60, 120, 0],
    };
    let seconds = args.seconds.unwrap_or(3.0);
    let spin = Duration::from_millis(1);
    let mut c = Conn::connect(&path)?;
    let views = c.ask(json!({"op": "view.list"}))?;
    let (w, h) = match &args.size {
        Some(s) => {
            let (w, h) = crate::parse_size(s)?;
            (w as usize, h as usize)
        }
        // The text area: the view, less the status bar.
        None => (views["views"][0]["w"].as_u64().unwrap_or(80) as usize, views["views"][0]["h"].as_u64().unwrap_or(24).saturating_sub(1).max(1) as usize),
    };
    let names = names(args)?;
    println!("caretline at {}: {w}x{h} text cells, {} frames per scene", path.display(), args.frames);

    let mut rows = Vec::new();
    for name in &names {
        let mut sc = scene(name).ok_or_else(|| format!("unknown scene {name:?}"))?;
        c.ask(json!({"op": "frame", "text": format!("precomputing {name}…"), "status": format!("precomputing {name}")}))?;
        let t0 = Instant::now();
        let bodies: Vec<String> = (0..args.frames.max(1)).map(|k| sc.frame(k as f64 / 60.0, w, h).body()).collect();
        let avg = bodies.iter().map(String::len).sum::<usize>() / bodies.len().max(1);
        println!("{name}: {} frames in {:.2} s, {:.1} KB per request", bodies.len(), t0.elapsed().as_secs_f64(), avg as f64 / 1024.0);
        for &fps in &rates {
            let tag = if fps == 0 { format!("{name} · unthrottled") } else { format!("{name} · {fps} fps target") };
            let r = play(&mut c, &bodies, seconds, fps, spin, &tag)?;
            let line = if fps == 0 {
                format!("{name:<8} unthrottled   {:>8.1} frames/s", r.fps())
            } else {
                format!("{name:<8} target {fps:>4}   {:>8.1} frames/s   late p50 {} p99 {}", r.fps(), ms(r.late_pct(50)), ms(r.late_pct(99)))
            };
            println!("  {line}");
            rows.push(line);
        }
    }
    let summary = format!("\n  caretline frame demo, {w}x{h}\n\n  {}\n", rows.join("\n  "));
    c.ask(json!({"op": "frame", "text": summary, "status": "scenes: done"}))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_draws_a_frame_with_text() {
        for name in ALL {
            let mut sc = scene(name).unwrap();
            // Fire builds up from its first frames.
            for k in 0..30 {
                sc.frame(k as f64 / 60.0, 40, 12);
            }
            let (text, _) = sc.frame(0.5, 40, 12).parts();
            assert_eq!(text.lines().count(), 12, "{name}");
            assert!(text.chars().any(|c| !c.is_whitespace()), "{name} is blank");
        }
    }
}
