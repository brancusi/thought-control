//! Pixels: pictures the view asks for (cards, plots, image files) rasterised at the terminal's
//! real cell size and placed with the kitty graphics protocol (Ghostty, kitty, WezTerm).
//!
//! The view stays pure: it returns `Placed` pictures in cells, with colours and proportions, never
//! pixels. The runtime knows the cell size, rasterises each picture once (they're cached by
//! content), sends it, and re-places only what moved.

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::io::Write;

pub type Rgb = (u8, u8, u8);

/// A picture over a rectangle of cells.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    /// Below zero draws under the text (a backdrop); zero or more above it.
    pub z: i32,
    pub pic: Pic,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pic {
    /// A rounded panel: a fill (a vertical gradient when `fill2` is set), an optional 1px border,
    /// a soft shadow. Sizes are fractions of a cell's height.
    Card { fill: Rgb, fill2: Option<Rgb>, border: Option<Rgb>, radius: f32, shadow: f32, glow: Option<Rgb> },
    /// Lines over shared bounds, anti-aliased, each with a gradient fill down to the bottom.
    Plot { series: Vec<(Vec<f64>, Rgb)>, fill: bool, width: f32 },
    /// An image file (PNG), scaled into the cells.
    File(String),
    /// A glowing dot (the brand mark's `•`), centred.
    Dot { color: Rgb, glow: f32 },
}

impl Pic {
    fn key(&self, px: (u32, u32)) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{self:?}").hash(&mut h);
        px.hash(&mut h);
        h.finish()
    }
}

// ---- rasterising ----------------------------------------------------------------------------

/// Straight RGBA, row-major.
struct Canvas {
    w: u32,
    h: u32,
    px: Vec<[f32; 4]>,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Canvas {
        Canvas { w, h, px: vec![[0.0; 4]; (w * h) as usize] }
    }

    /// Paint `c` with coverage `a` over what's there (source-over).
    fn blend(&mut self, x: u32, y: u32, c: Rgb, a: f32) {
        if x >= self.w || y >= self.h || a <= 0.0 {
            return;
        }
        let p = &mut self.px[(y * self.w + x) as usize];
        let a = a.min(1.0);
        let out_a = a + p[3] * (1.0 - a);
        if out_a <= 0.0 {
            return;
        }
        for (k, v) in [c.0, c.1, c.2].into_iter().enumerate() {
            p[k] = (v as f32 / 255.0 * a + p[k] * p[3] * (1.0 - a)) / out_a;
        }
        p[3] = out_a;
    }

    fn rgba(&self) -> Vec<u8> {
        self.px.iter().flat_map(|p| [p[0], p[1], p[2], p[3]].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect()
    }
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

/// Signed distance from a point to a rounded rectangle (negative inside).
fn rounded(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> f32 {
    let cx = (x0 + x1) / 2.0;
    let cy = (y0 + y1) / 2.0;
    let hx = (x1 - x0) / 2.0 - r;
    let hy = (y1 - y0) / 2.0 - r;
    let qx = (px - cx).abs() - hx;
    let qy = (py - cy).abs() - hy;
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

fn card(w: u32, h: u32, cell_h: f32, pic: &Pic) -> Canvas {
    let mut c = Canvas::new(w, h);
    let Pic::Card { fill, fill2, border, radius, shadow, glow } = *pic else { return c };
    let r = radius * cell_h;
    let s = shadow * cell_h;
    // The panel sits inside the shadow's margin; the shadow falls a little down.
    let (x0, y0, x1, y1) = (s * 0.6, s * 0.35, w as f32 - s * 0.6, h as f32 - s * 0.85);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            if s > 0.0 {
                let d = rounded(fx, fy - s * 0.25, x0, y0, x1, y1, r);
                let a = (1.0 - (d / s).clamp(0.0, 1.0)).powf(2.2) * 0.55;
                let tint = glow.unwrap_or((0, 0, 0));
                c.blend(x, y, tint, a);
            }
            let d = rounded(fx, fy, x0, y0, x1, y1, r);
            let cover = (0.5 - d).clamp(0.0, 1.0);
            if cover > 0.0 {
                let t = ((fy - y0) / (y1 - y0)).clamp(0.0, 1.0);
                c.blend(x, y, fill2.map_or(fill, |f2| mix(fill, f2, t)), cover);
            }
            if let Some(b) = border {
                let edge = (1.0 - (d + 0.75).abs()).clamp(0.0, 1.0);
                // A lighter rim on top fading down: light from above.
                let t = ((fy - y0) / (y1 - y0)).clamp(0.0, 1.0);
                c.blend(x, y, b, edge * (1.0 - t * 0.7));
            }
        }
    }
    c
}

/// Distance from a point to a segment.
fn seg_dist(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx * dx + dy * dy;
    let t = if len > 0.0 { (((px - a.0) * dx + (py - a.1) * dy) / len).clamp(0.0, 1.0) } else { 0.0 };
    ((px - a.0 - t * dx).powi(2) + (py - a.1 - t * dy).powi(2)).sqrt()
}

fn plot(w: u32, h: u32, cell_h: f32, series: &[(Vec<f64>, Rgb)], fill: bool, width: f32) -> Canvas {
    let mut c = Canvas::new(w, h);
    let all = series.iter().flat_map(|s| s.0.iter().copied());
    let (lo, hi) = all.fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
    if lo > hi {
        return c;
    }
    let pad = cell_h * 0.4;
    let span = if hi > lo { hi - lo } else { 1.0 };
    let lw = (width * cell_h).max(1.0);
    for (vals, col) in series {
        if vals.len() < 2 {
            continue;
        }
        let n = vals.len() - 1;
        let pts: Vec<(f32, f32)> = vals
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let x = pad + (w as f32 - 2.0 * pad) * i as f32 / n as f32;
                let y = pad + (h as f32 - 2.0 * pad) * (1.0 - ((v - lo) / span) as f32);
                (x, y)
            })
            .collect();
        if fill {
            // The area under the line, fading out toward the bottom.
            for x in 0..w {
                let fx = x as f32 + 0.5;
                let k = pts.windows(2).position(|p| fx >= p[0].0 && fx <= p[1].0);
                let Some(k) = k else { continue };
                let (a, b) = (pts[k], pts[k + 1]);
                let t = (fx - a.0) / (b.0 - a.0).max(1e-3);
                let top = a.1 + (b.1 - a.1) * t;
                for y in (top.max(0.0) as u32)..h {
                    let depth = (y as f32 - top) / (h as f32 - top).max(1.0);
                    c.blend(x, y, *col, 0.32 * (1.0 - depth).powf(1.6));
                }
            }
        }
        // The line: each pixel's coverage is its distance to the nearest segment (so joints
        // don't double up), then it's painted once.
        let mut cover = vec![0.0f32; (w * h) as usize];
        for p in pts.windows(2) {
            let (a, b) = (p[0], p[1]);
            let (minx, maxx) = (a.0.min(b.0) - lw - 1.0, a.0.max(b.0) + lw + 1.0);
            let (miny, maxy) = (a.1.min(b.1) - lw - 1.0, a.1.max(b.1) + lw + 1.0);
            for y in (miny.max(0.0) as u32)..(maxy.min(h as f32) as u32) {
                for x in (minx.max(0.0) as u32)..(maxx.min(w as f32) as u32) {
                    let d = seg_dist(x as f32 + 0.5, y as f32 + 0.5, a, b);
                    let i = (y * w + x) as usize;
                    cover[i] = cover[i].max((lw / 2.0 + 0.5 - d).clamp(0.0, 1.0));
                }
            }
        }
        for (i, a) in cover.into_iter().enumerate() {
            if a > 0.0 {
                c.blend(i as u32 % w, i as u32 / w, *col, a);
            }
        }
        // The last point, ringed.
        if let Some(&(lx, ly)) = pts.last() {
            let r = lw * 2.2;
            for y in (ly - r - 2.0).max(0.0) as u32..((ly + r + 2.0).min(h as f32) as u32) {
                for x in (lx - r - 2.0).max(0.0) as u32..((lx + r + 2.0).min(w as f32) as u32) {
                    let d = ((x as f32 + 0.5 - lx).powi(2) + (y as f32 + 0.5 - ly).powi(2)).sqrt();
                    c.blend(x, y, *col, (r + 0.5 - d).clamp(0.0, 1.0));
                    c.blend(x, y, *col, (1.0 - ((d - r) / (r * 2.5)).clamp(0.0, 1.0)) * 0.25);
                }
            }
        }
    }
    c
}

fn dot(w: u32, h: u32, color: Rgb, glow: f32) -> Canvas {
    let mut c = Canvas::new(w, h);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let r = w.min(h) as f32 * 0.22;
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            if glow > 0.0 {
                c.blend(x, y, color, (1.0 - ((d - r) / (r * 2.2 * glow)).clamp(0.0, 1.0)).powf(2.0) * 0.45);
            }
            // A little highlight up and to the left: a lit sphere, not a flat disc.
            let hl = ((x as f32 + 0.5 - (cx - r * 0.35)).powi(2) + (y as f32 + 0.5 - (cy - r * 0.35)).powi(2)).sqrt();
            let shade = mix((255, 236, 220), color, (hl / (r * 1.3)).clamp(0.0, 1.0));
            c.blend(x, y, shade, (r + 0.5 - d).clamp(0.0, 1.0));
        }
    }
    c
}

// ---- the protocol ---------------------------------------------------------------------------

/// What the terminal can do, found once at start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Caps {
    pub graphics: bool,
    /// Pixels per cell (0 when unknown).
    pub cell_w: f32,
    pub cell_h: f32,
}

impl Caps {
    /// Kitty graphics by terminal (Ghostty, kitty, WezTerm), `THC_SCENE_GRAPHICS=0|1` to force,
    /// and the cell size from the window's pixel size.
    pub fn detect() -> Caps {
        let env = |k: &str| std::env::var(k).unwrap_or_default();
        let term = format!("{} {}", env("TERM_PROGRAM"), env("TERM")).to_lowercase();
        let mut graphics = term.contains("ghostty") || term.contains("kitty") || term.contains("wezterm");
        match env("THC_SCENE_GRAPHICS").as_str() {
            "0" => graphics = false,
            "1" => graphics = true,
            _ => {}
        }
        let (mut cell_w, mut cell_h) = (0.0, 0.0);
        if let Ok(ws) = ratatui::crossterm::terminal::window_size()
            && ws.columns > 0
            && ws.rows > 0
            && ws.width > 0
        {
            cell_w = ws.width as f32 / ws.columns as f32;
            cell_h = ws.height as f32 / ws.rows as f32;
        }
        Caps { graphics: graphics && cell_h > 0.0, cell_w, cell_h }
    }
}

/// Whether the terminal draws kitty's larger text (OSC 66): print two characters at twice the
/// size and ask where the cursor went. Four columns on means it does; two means it printed them
/// plainly; none means it dropped them. Call before anything else reads the terminal's input.
pub fn probe_text_sizing() -> bool {
    use ratatui::crossterm::{cursor, terminal};
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b[1;1H\x1b]66;s=2;ab\x07");
    let _ = out.flush();
    let col = cursor::position().map(|(c, _)| c).unwrap_or(0);
    let _ = ratatui::crossterm::execute!(out, terminal::Clear(terminal::ClearType::All));
    col == 4
}

/// Sends pictures and keeps track of what's on screen.
#[derive(Default)]
pub struct Kitty {
    /// Content key → image id, for everything sent and not yet freed.
    sent: BTreeMap<u64, u32>,
    /// What was placed last frame: (image id, placement id, x, y, w, h, z).
    placed: Vec<(u32, u32, u16, u16, u16, u16, i32)>,
    next: u32,
    pub bytes: usize,
}

impl Kitty {
    /// Bring the screen's pictures in line with `want`. Returns the escape bytes to write.
    pub fn sync(&mut self, want: &[Placed], caps: Caps) -> Vec<u8> {
        let mut out = Vec::new();
        if !caps.graphics {
            return out;
        }
        let mut now = Vec::new();
        let mut live = BTreeSet::new();
        for (k, p) in want.iter().enumerate() {
            let px = ((p.w as f32 * caps.cell_w).round() as u32, (p.h as f32 * caps.cell_h).round() as u32);
            if px.0 == 0 || px.1 == 0 {
                continue;
            }
            let key = p.pic.key(px);
            let id = match self.sent.get(&key) {
                Some(id) => *id,
                None => {
                    self.next += 1;
                    let id = self.next;
                    send(&mut out, id, &p.pic, px, caps.cell_h);
                    self.sent.insert(key, id);
                    id
                }
            };
            live.insert(key);
            now.push((id, k as u32 + 1, p.x, p.y, p.w, p.h, p.z));
        }
        if now != self.placed {
            // Placing with an id that's on screen moves it; anything gone is deleted.
            for old in &self.placed {
                if !now.iter().any(|n| n.0 == old.0 && n.1 == old.1) {
                    let _ = write!(out, "\x1b_Ga=d,d=i,i={},p={},q=2\x1b\\", old.0, old.1);
                }
            }
            for &(id, pid, x, y, w, h, z) in &now {
                if !self.placed.contains(&(id, pid, x, y, w, h, z)) {
                    let _ = write!(out, "\x1b[{};{}H\x1b_Ga=p,i={id},p={pid},c={w},r={h},z={z},C=1,q=2\x1b\\", y + 1, x + 1);
                }
            }
            self.placed = now;
        }
        // Free images nothing shows any more (plots that moved on).
        let gone: Vec<u64> = self.sent.keys().filter(|k| !live.contains(k)).copied().collect();
        for k in gone {
            if let Some(id) = self.sent.remove(&k) {
                let _ = write!(out, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\");
            }
        }
        self.bytes += out.len();
        out
    }

    /// Delete everything this screen placed (on exit or upgrade).
    pub fn clear(&mut self) -> Vec<u8> {
        self.sent.clear();
        self.placed.clear();
        b"\x1b_Ga=d,d=A,q=2\x1b\\".to_vec()
    }
}

fn send(out: &mut Vec<u8>, id: u32, pic: &Pic, px: (u32, u32), cell_h: f32) {
    let b64 = base64::engine::general_purpose::STANDARD;
    if let Pic::File(path) = pic {
        // The terminal reads the file itself, from its own working directory: send it absolute.
        let abs = std::fs::canonicalize(path).map(|p| p.display().to_string()).unwrap_or_else(|_| path.clone());
        let _ = write!(out, "\x1b_Ga=t,t=f,f=100,i={id},q=2;{}\x1b\\", b64.encode(abs));
        return;
    }
    let (w, h) = px;
    let canvas = match pic {
        Pic::Card { .. } => card(w, h, cell_h, pic),
        Pic::Plot { series, fill, width } => plot(w, h, cell_h, series, *fill, *width),
        Pic::Dot { color, glow } => dot(w, h, *color, *glow),
        Pic::File(_) => unreachable!(),
    };
    let data = miniz_oxide::deflate::compress_to_vec_zlib(&canvas.rgba(), 4);
    let enc = b64.encode(&data);
    let chunks: Vec<&str> = enc.as_bytes().chunks(4096).map(|c| std::str::from_utf8(c).unwrap_or("")).collect();
    for (k, chunk) in chunks.iter().enumerate() {
        let more = (k + 1 < chunks.len()) as u8;
        if k == 0 {
            let _ = write!(out, "\x1b_Ga=t,f=32,o=z,s={w},v={h},i={id},q=2,m={more};{chunk}\x1b\\");
        } else {
            let _ = write!(out, "\x1b_Gm={more};{chunk}\x1b\\");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_is_opaque_inside_and_clear_at_the_corners() {
        let c = card(100, 50, 20.0, &Pic::Card { fill: (40, 40, 40), fill2: None, border: None, radius: 0.5, shadow: 0.0, glow: None });
        assert_eq!(c.px[(25 * 100 + 50) as usize][3], 1.0, "middle");
        assert_eq!(c.px[0][3], 0.0, "corner, outside the radius");
    }

    #[test]
    fn placing_the_same_picture_twice_sends_it_once() {
        let caps = Caps { graphics: true, cell_w: 8.0, cell_h: 16.0 };
        let pic = Pic::Card { fill: (1, 2, 3), fill2: None, border: None, radius: 0.3, shadow: 0.0, glow: None };
        let want = vec![Placed { x: 0, y: 0, w: 4, h: 2, z: -1, pic: pic.clone() }, Placed { x: 5, y: 0, w: 4, h: 2, z: -1, pic }];
        let mut k = Kitty::default();
        let first = String::from_utf8(k.sync(&want, caps)).unwrap();
        assert_eq!(first.matches("a=t").count(), 1);
        assert_eq!(first.matches("a=p").count(), 2);
        assert!(k.sync(&want, caps).is_empty(), "nothing changed, nothing sent");
    }
}
