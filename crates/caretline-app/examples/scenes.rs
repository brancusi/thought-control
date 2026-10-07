//! ASCII animation scenes pushed into a live caretline editor with the `frame` op, paced
//! against absolute deadlines. A demo of the frame path and a measurement of it.
//!
//! ```sh
//! caretline notes.md --listen                                # in one terminal
//! cargo run --release -p caretline-app --example scenes      # in another
//! cargo run --release -p caretline-app --example scenes -- --fps 120 --scene donut,plasma
//! ```
//!
//! Each scene is precomputed (text plus highlight ranges), then played at every target rate
//! for `--seconds` (`--fps 0` is unthrottled: the next frame goes as soon as the editor
//! answers the last). It prints the frames per second achieved and how late each frame left
//! against its deadline. Pacing: frame `k` is due at `start + k / fps`; the client sleeps
//! until about 1 ms before that and spins the rest, so it neither drifts nor oversleeps.

use std::f64::consts::PI;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const USAGE: &str = "scenes [--socket PATH] [--fps 60,120,0] [--seconds S] [--scene all|donut,cube,tunnel,plasma,fire,warp] [--frames N] [--size WxH] [--spin-ms MS]";

struct Opts {
    socket: Option<PathBuf>,
    fps: Vec<u32>,
    seconds: f64,
    scenes: Vec<String>,
    frames: usize,
    size: Option<(usize, usize)>,
    /// How long before a deadline to stop sleeping and spin.
    spin: Duration,
}

fn opts() -> Result<Opts, String> {
    let mut o = Opts { socket: None, fps: vec![60, 120, 0], seconds: 3.0, scenes: Vec::new(), frames: 180, size: None, spin: Duration::from_millis(1) };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().ok_or_else(|| format!("{a} needs a value\nusage: {USAGE}"));
        match a.as_str() {
            "--socket" => o.socket = Some(val()?.into()),
            "--fps" => o.fps = val()?.split(',').map(|f| f.trim().parse().map_err(|_| format!("bad fps {f:?}"))).collect::<Result<_, _>>()?,
            "--seconds" => o.seconds = val()?.parse().map_err(|_| "bad --seconds")?,
            "--spin-ms" => o.spin = Duration::from_secs_f64(val()?.parse::<f64>().map_err(|_| "bad --spin-ms")? / 1e3),
            "--frames" => o.frames = val()?.parse().map_err(|_| "bad --frames")?,
            "--scene" => o.scenes = val()?.split(',').filter(|s| *s != "all").map(str::to_string).collect(),
            "--size" => {
                let v = val()?;
                let (w, h) = v.split_once('x').ok_or("--size is WxH")?;
                o.size = Some((w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?));
            }
            "-h" | "--help" => return Err(format!("usage: {USAGE}")),
            other => return Err(format!("unknown argument {other:?}\nusage: {USAGE}")),
        }
    }
    Ok(o)
}

/// The newest live editor in `$TMPDIR/caretline/*.json` that answers.
fn discover() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("caretline");
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
            && UnixStream::connect(sock).is_ok()
        {
            return Ok(sock.into());
        }
    }
    Err("no live caretline: start one with `caretline FILE --listen`".into())
}

struct Conn {
    w: UnixStream,
    r: BufReader<UnixStream>,
    line: String,
}

impl Conn {
    fn send(&mut self, req: &str) -> Result<(), String> {
        self.w.write_all(req.as_bytes()).and_then(|_| self.w.write_all(b"\n")).map_err(|e| format!("send: {e}"))?;
        self.line.clear();
        self.r.read_line(&mut self.line).map_err(|e| format!("read: {e}"))?;
        if self.line.is_empty() {
            return Err("the editor closed the connection".into());
        }
        Ok(())
    }

    fn ask(&mut self, req: Value) -> Result<Value, String> {
        self.send(&req.to_string())?;
        let v: Value = serde_json::from_str(&self.line).map_err(|e| format!("reply: {e}"))?;
        if v.get("error").is_some() {
            return Err(format!("{}", v["error"]));
        }
        Ok(v["result"].clone())
    }
}

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
        format!(r#""text":{},"highlights":{}}}"#, Value::from(text), json!(ranges))
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

const ALL: [&str; 6] = ["donut", "cube", "tunnel", "plasma", "fire", "warp"];

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

fn main() {
    if let Err(e) = run() {
        eprintln!("scenes: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let o = opts()?;
    let path = match &o.socket {
        Some(p) => p.clone(),
        None => discover()?,
    };
    let s = UnixStream::connect(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut c = Conn { w: s.try_clone().map_err(|e| e.to_string())?, r: BufReader::new(s), line: String::new() };
    let views = c.ask(json!({"op": "view.list"}))?;
    let (w, h) = match o.size {
        Some(s) => s,
        // The text area: the view, less the status bar.
        None => (views["views"][0]["w"].as_u64().unwrap_or(80) as usize, views["views"][0]["h"].as_u64().unwrap_or(24).saturating_sub(1).max(1) as usize),
    };
    let names: Vec<String> = if o.scenes.is_empty() { ALL.iter().map(|s| s.to_string()).collect() } else { o.scenes.clone() };
    println!("caretline at {}: {w}x{h} text cells, {} frames per scene", path.display(), o.frames);

    let mut rows = Vec::new();
    for name in &names {
        let mut sc = scene(name).ok_or_else(|| format!("unknown scene {name:?}; scenes: {}", ALL.join(", ")))?;
        c.ask(json!({"op": "frame", "text": format!("precomputing {name}…"), "status": format!("precomputing {name}")}))?;
        let t0 = Instant::now();
        let bodies: Vec<String> = (0..o.frames).map(|k| sc.frame(k as f64 / 60.0, w, h).body()).collect();
        let avg = bodies.iter().map(String::len).sum::<usize>() / bodies.len().max(1);
        println!("{name}: {} frames in {:.2} s, {:.1} KB per request", bodies.len(), t0.elapsed().as_secs_f64(), avg as f64 / 1024.0);
        for &fps in &o.fps {
            let tag = if fps == 0 { format!("{name} · unthrottled") } else { format!("{name} · {fps} fps target") };
            let r = play(&mut c, &bodies, o.seconds, fps, o.spin, &tag)?;
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
