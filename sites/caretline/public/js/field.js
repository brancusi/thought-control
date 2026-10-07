/* caretline field: ASCII scenes on a frame clock, drawn two-tone.
 *
 * Every scene is a function of (time, grid). It writes one glyph per cell into `g` and marks
 * the bright cells in `hot`. Bright cells are drawn the way caretline draws a selection: a lit
 * block with the glyph knocked out of it. That is the whole visual language: ink on the void,
 * and the selection colour for the hottest parts.
 *
 * The maths is ported from the stress-test scripts that pushed these frames into a live
 * caretline editor over its socket. Time is an input (seconds on the frame clock), so a frame
 * for time t is always the same frame; only `fire` keeps a buffer, advanced in fixed steps.
 *
 * Plain script, no dependencies. Exposes window.CaretlineField.
 */
(function () {
  'use strict';

  // A small seeded generator, so star fields and fire are the same on every load.
  function rng(seed) {
    let a = seed >>> 0;
    return function () {
      a = (a + 0x6d2b79f5) >>> 0;
      let t = a;
      t = Math.imul(t ^ (t >>> 15), t | 1);
      t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }

  function Grid(W, H) {
    this.W = W; this.H = H;
    this.g = new Array(W * H).fill(' ');
    this.hot = new Uint8Array(W * H);
    this.z = new Float32Array(W * H);
  }
  Grid.prototype.clear = function () { this.g.fill(' '); this.hot.fill(0); this.z.fill(0); this.caret = null; };
  Grid.prototype.put = function (x, y, ch, hot) {
    x |= 0; y |= 0;
    if (x < 0 || y < 0 || x >= this.W || y >= this.H) return;
    const i = y * this.W + x; this.g[i] = ch; if (hot) this.hot[i] = 1;
  };
  Grid.prototype.text = function () {
    const rows = [];
    for (let y = 0; y < this.H; y++) rows.push(this.g.slice(y * this.W, (y + 1) * this.W).join(''));
    return rows.join('\n');
  };

  // ── scenes ──────────────────────────────────────────────────────────────
  // The originals ran on a 122×64 grid; sx and sy scale their constants to any grid.

  function donut(G, t) {
    const { W, H } = G, sx = W / 122, sy = H / 64;
    const A = t * 1.7, B = t * 0.9;
    const cA = Math.cos(A), sA = Math.sin(A), cB = Math.cos(B), sB = Math.sin(B);
    const ramp = '.,-~:;=!*#$@';
    const dj = W < 70 ? 12 : 7, di = W < 70 ? 4 : 2;
    for (let j = 0; j < 628; j += dj) {
      const ct = Math.cos(j / 100), st = Math.sin(j / 100);
      for (let i = 0; i < 628; i += di) {
        const sp = Math.sin(i / 100), cp = Math.cos(i / 100), h = ct + 2;
        const D = 1 / (sp * h * sA + st * cA + 5), tt = sp * h * cA - st * sA;
        const x = (W / 2 + 70 * sx * D * (cp * h * cB - tt * sB)) | 0;
        const y = (H / 2 + 32 * sy * D * (cp * h * sB + tt * cB)) | 0;
        const L = (8 * ((st * sA - sp * ct * cA) * cB - sp * ct * sA - st * cA - cp * ct * sB)) | 0;
        if (y >= 0 && y < H && x >= 0 && x < W) {
          const k = y * W + x;
          if (D > G.z[k]) {
            G.z[k] = D; G.g[k] = ramp[Math.max(L, 0)];
            G.hot[k] = L >= 8 ? 1 : 0;
          }
        }
      }
    }
  }

  const CUBE_V = [];
  for (const x of [-1, 1]) for (const y of [-1, 1]) for (const z of [-1, 1]) CUBE_V.push([x, y, z]);
  const CUBE_E = [];
  for (let a = 0; a < 8; a++) for (let b = a + 1; b < 8; b++) {
    let d = 0; for (let k = 0; k < 3; k++) if (CUBE_V[a][k] !== CUBE_V[b][k]) d++;
    if (d === 1) CUBE_E.push([a, b]);
  }
  function cube(G, t) {
    const { W, H } = G, sx = W / 122, sy = H / 64;
    const ax = t * 0.9, ay = t * 1.3, az = t * 0.4;
    const P = CUBE_V.map(([x, y, z]) => {
      let y1 = y * Math.cos(ax) - z * Math.sin(ax), z1 = y * Math.sin(ax) + z * Math.cos(ax);
      let x2 = x * Math.cos(ay) + z1 * Math.sin(ay), z2 = -x * Math.sin(ay) + z1 * Math.cos(ay);
      let x3 = x2 * Math.cos(az) - y1 * Math.sin(az), y3 = x2 * Math.sin(az) + y1 * Math.cos(az);
      const f = 3.2 / (z2 + 4);
      return [W / 2 + x3 * f * 34 * sx, H / 2 + y3 * f * 16 * sy, z2];
    });
    for (const [a, b] of CUBE_E) {
      const [x0, y0, z0] = P[a], [x1, y1, z1] = P[b];
      const n = Math.max(Math.abs(x1 - x0), Math.abs(y1 - y0)) | 0;
      for (let k = 0; k <= n + 1; k++) {
        const u = k / (n + 1), z = z0 + (z1 - z0) * u;
        G.put(x0 + (x1 - x0) * u, y0 + (y1 - y0) * u, z < -0.3 ? '@' : z < 0.4 ? '#' : '+', z < -0.3);
      }
    }
    for (const [x, y] of P) G.put(x, y, 'O', true);
  }

  function tunnel(G, t) {
    const { W, H } = G, ramp = ' .:-=+*#%@';
    for (let y = 0; y < H; y++) for (let x = 0; x < W; x++) {
      const dx = (x - W / 2) / W * 2, dy = (y - H / 2) / H * 2 * 0.55;
      const r = Math.hypot(dx, dy) + 1e-6, a = Math.atan2(dy, dx);
      const u = 0.6 / r + t * 3, v = a * 4 / Math.PI + t * 0.7;
      const c = ((u * 4) ^ (v * 4 + 64)) & 7;
      const k = y * W + x;
      G.g[k] = r > 0.04 ? ramp[Math.min(9, c + ((4 * (1 - Math.min(r, 1))) | 0))] : ' ';
      G.hot[k] = c === 7 && r > 0.08 ? 1 : 0;
    }
  }

  function plasma(G, t) {
    const { W, H } = G, ramp = ' .:-=+*#%@';
    const k = 2 * H / W; // cells are 1:2, so rows count double
    const cx = 4 + Math.sin(t * 0.5) * 2.5, cy = 4 * k + Math.cos(t * 0.37) * 2;
    for (let y = 0; y < H; y++) for (let x = 0; x < W; x++) {
      const u = x / W * 8, v = y / H * 8 * k;
      let p = Math.sin(u * 0.9 + t * 1.1) + Math.sin(v * 1.2 - t * 0.8)
        + Math.sin((u * 0.7 + v * 0.9) + t * 0.6)
        + Math.sin(Math.hypot(u - cx, v - cy) * 1.7 - t * 1.9);
      // Contour bands: the field folded three times, so it ripples instead of blurring.
      const f = (((p + 4) / 8) * 3) % 1;
      const i = y * W + x;
      G.g[i] = ramp[Math.min(9, (f * 10) | 0)];
      G.hot[i] = f > 0.86 ? 1 : 0;
    }
  }

  // Fire keeps a heat buffer; it advances in fixed 1/30 s steps of the frame clock.
  function makeFire() {
    let buf = null, W0 = 0, H0 = 0, step = -1, rand = rng(7);
    return function fire(G, t) {
      const { W, H } = G;
      if (!buf || W0 !== W || H0 !== H) {
        buf = []; for (let y = 0; y < H + 2; y++) buf.push(new Int16Array(W));
        W0 = W; H0 = H; step = -1;
      }
      const target = (t * 30) | 0;
      if (target < step || target - step > 90) { for (const r of buf) r.fill(0); step = target - 60; }
      // A short grid burns out sooner, so the flames keep the same shape at any height.
      const decay = Math.min(0.95, 0.3 * 64 / H);
      while (step < target) {
        step++;
        const pick = [0, 36, 36, 36];
        for (let x = 0; x < W; x++) buf[H + 1][x] = pick[(rand() * 4) | 0];
        for (let y = 0; y < H + 1; y++) {
          const row = buf[y], below = buf[y + 1];
          for (let x = 0; x < W; x++) {
            const v = ((below[(x - 1 + W) % W] + below[x] + below[(x + 1) % W] + below[x]) >> 2) - (rand() < decay ? 1 : 0);
            row[x] = v > 0 ? v : 0;
          }
        }
      }
      const ramp = ' .,:;+*%#@';
      for (let y = 0; y < H; y++) {
        const row = buf[y];
        for (let x = 0; x < W; x++) {
          const v = row[x];
          const k = y * W + x;
          G.g[k] = ramp[Math.min(9, (v / 4) | 0)];
          G.hot[k] = v > 30 ? 1 : 0;
        }
      }
    };
  }

  // Rain: columns of glyphs falling at their own speeds. The head of each drop is lit; the
  // trail thins to dots. Glyphs flicker on the clock, from a hash, so time t is one frame.
  const RAIN = 'abcdefghijklmnopqrstuvwxyz0123456789{}[]<>/\\|=+*:;$#%&';
  function hash(a, b, c) { let h = (a * 374761393 + b * 668265263 + c * 2147483647) | 0; h = Math.imul(h ^ (h >>> 13), 1274126177); return (h ^ (h >>> 16)) >>> 0; }
  function rain(G, t) {
    const { W, H } = G;
    for (let x = 0; x < W; x++) {
      for (let lane = 0; lane < 1; lane++) {
        const seed = hash(x, lane, 7);
        if (seed % 4 === 0) continue; // some columns stay dark
        const speed = 6 + (seed % 1000) / 1000 * 16;           // rows a second
        const len = 6 + (seed >>> 10) % Math.max(4, (H * 0.6) | 0);
        const period = H + len + (seed >>> 20) % H;
        const head = ((t * speed + (seed % period)) % period) - len * lane * 0.5;
        for (let k = 0; k < len; k++) {
          const y = Math.floor(head) - k;
          if (y < 0 || y >= H) continue;
          const i = y * W + x;
          const flick = hash(x, y, Math.floor(t * (k === 0 ? 14 : 5)));
          G.g[i] = k > len * 0.7 ? (flick % 3 ? '.' : ':') : RAIN[flick % RAIN.length];
          if (k === 0) G.hot[i] = 1;
        }
      }
    }
  }

  // A 5×5 block face for the big wordmark.
  const BLOCK = {
    C: [' ####', '#    ', '#    ', '#    ', ' ####'],
    A: [' ### ', '#   #', '#####', '#   #', '#   #'],
    R: ['#### ', '#   #', '#### ', '#  # ', '#   #'],
    E: ['#####', '#    ', '#### ', '#    ', '#####'],
    T: ['#####', '  #  ', '  #  ', '  #  ', '  #  '],
    L: ['#    ', '#    ', '#    ', '#    ', '#####'],
    I: ['#####', '  #  ', '  #  ', '  #  ', '#####'],
    N: ['#   #', '##  #', '# # #', '#  ##', '#   #'],
  };

  // The warp field: stars streaming out of the centre, past the wordmark.
  function makeWarp(opts) {
    opts = opts || {};
    const r = rng(11), stars = [];
    const n = opts.stars || 900;
    for (let i = 0; i < n; i++) stars.push([r() * 2 - 1, r() * 2 - 1, 0.05 + r() * 0.95]);
    const word = opts.word === undefined ? 'C A R E T L I N E' : opts.word;
    return function warp(G, t) {
      const { W, H } = G;
      const cx = W / 2, cy = H / 2, kx = W * 0.25, ky = H * 0.25;
      const speed = opts.speed || 0.35;
      // The same density at any size: the originals had 900 stars on 122×64 cells.
      const count = Math.min(stars.length, Math.round(n * (W * H) / (122 * 64) * (opts.density || 1)));
      for (let s = 0; s < count; s++) {
        const [sx, sy, sz] = stars[s];
        const z = ((sz - t * speed) % 1 + 1) % 1 + 0.02;
        const x = cx + sx / z * kx, y = cy + sy / z * ky;
        if (x < 0 || x >= W || y < 0 || y >= H) continue;
        // A short streak behind the near stars, toward the centre: where it was a moment ago.
        if (z < 0.22) {
          const zb = z + 0.035, xb = cx + sx / zb * kx, yb = cy + sy / zb * ky;
          const n = Math.max(Math.abs(x - xb), Math.abs(y - yb)) | 0;
          for (let k = 1; k <= n && k < 8; k++) {
            const u = k / (n + 1);
            const px = (xb + (x - xb) * u) | 0, py = (yb + (y - yb) * u) | 0;
            const i = py * W + px;
            if (px >= 0 && px < W && py >= 0 && py < H && G.g[i] === ' ') G.g[i] = Math.abs(x - xb) > Math.abs(y - yb) * 2 ? '-' : '.';
          }
        }
        G.put(x, y, z < 0.15 ? '@' : z < 0.35 ? '*' : z < 0.6 ? '+' : '.', z < 0.2);
      }
      if (!word) return;
      const big = opts.big !== false && W >= 116 && H >= 18 && word === 'C A R E T L I N E';
      if (big) {
        // The word in a 5-row block face, each pixel two cells wide (a cell is 1:2, so square).
        // Pixels are lit; stars passing behind them show through as knocked-out glyphs.
        const letters = 'CARETLINE', pw = 2, gap = 2, lw = 5 * pw;
        const width = letters.length * lw + (letters.length - 1) * gap;
        const x0 = ((W - width - 4) / 2) | 0, y0 = ((H - 5) / 2) | 0;
        for (let yy = y0 - 1; yy <= y0 + 5; yy++) for (let xx = x0 - 3; xx < x0 + width + 6; xx++) {
          if (xx >= 0 && xx < W && yy >= 0 && yy < H && (yy === y0 - 1 || yy === y0 + 5 || xx < x0 - 1 || xx >= x0 + width + 4)) { G.g[yy * W + xx] = ' '; G.hot[yy * W + xx] = 0; }
        }
        for (let k = 0; k < letters.length; k++) {
          const rows = BLOCK[letters[k]], lx = x0 + k * (lw + gap);
          for (let ry = 0; ry < 5; ry++) for (let rx = 0; rx < 5; rx++) {
            const on = rows[ry][rx] === '#';
            for (let d = 0; d < pw; d++) {
              const xx = lx + rx * pw + d, yy = y0 + ry, i = yy * W + xx;
              if (xx < 0 || xx >= W || yy < 0 || yy >= H) continue;
              if (on) G.hot[i] = 1;
              else { G.hot[i] = 0; G.g[i] = ' '; }
            }
          }
          // keep the gaps between letters dark
          for (let yy = y0; yy < y0 + 5; yy++) for (let d = 0; d < gap && k < letters.length - 1; d++) { const i = yy * W + lx + lw + d; G.g[i] = ' '; G.hot[i] = 0; }
        }
        if (opts.caret !== false && ((t * 1000 / 530) | 0) % 2 === 0) G.caret = [x0 + width + 3, y0, 5];
        return;
      }
      const y = (H / 2) | 0, x0 = ((W - word.length) / 2) | 0;
      // A quiet margin around the word, so it reads at any density.
      for (let yy = y - 1; yy <= y + 1; yy++) for (let xx = x0 - 3; xx < x0 + word.length + 4; xx++) {
        if (xx >= 0 && xx < W && yy >= 0 && yy < H) { G.g[yy * W + xx] = ' '; G.hot[yy * W + xx] = 0; }
      }
      for (let k = 0; k < word.length; k++) G.put(x0 + k, y, word[k], true);
      // The caret after the word, blinking on the frame clock (530 ms, a terminal's rate).
      if (opts.caret !== false && ((t * 1000 / 530) | 0) % 2 === 0) G.caret = [x0 + word.length + 1, y, 1];
    };
  }

  const SCENES = {
    warp: { name: 'warp field', make: () => makeWarp() },
    donut: { name: 'shaded torus', make: () => donut },
    cube: { name: 'wireframe cube', make: () => cube },
    tunnel: { name: 'XOR tunnel', make: () => tunnel },
    plasma: { name: 'plasma', make: () => plasma },
    fire: { name: 'cellular fire', make: () => makeFire() },
    rain: { name: 'matrix rain', make: () => rain },
  };

  // ── the renderer ────────────────────────────────────────────────────────

  function token(el, name, fallback) {
    const v = getComputedStyle(el).getPropertyValue(name).trim();
    return v || fallback;
  }

  const reduced = () => window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  /**
   * new Field(canvas, { scene: 'warp', cols: 120, rows: 36, speed: 1, still: 2.4, onStats })
   * Draws on a frame clock with requestAnimationFrame, only while visible. Under
   * prefers-reduced-motion it draws one still frame (at `still` seconds) until play() is called.
   */
  function Field(canvas, opts) {
    this.c = canvas; this.ctx = canvas.getContext('2d');
    this.o = Object.assign({ scene: 'warp', cols: 96, rows: 32, speed: 1, still: 2.4, minCols: 40 }, opts || {});
    this.fn = typeof this.o.fn === 'function' ? this.o.fn : SCENES[this.o.scene].make();
    this.t0 = null; this.paused = reduced(); this.visible = true; this.raf = 0;
    this.stats = { fps: 0, ms: 0, frames: 0 };
    this.time = this.o.still;
    this.resize();
    const ro = window.ResizeObserver ? new ResizeObserver(() => this.resize()) : null;
    if (ro) ro.observe(canvas.parentElement || canvas);
    if (window.IntersectionObserver) {
      new IntersectionObserver((es) => { this.visible = es[0].isIntersecting; this.kick(); }, { rootMargin: '80px' }).observe(canvas);
    }
    document.addEventListener('visibilitychange', () => this.kick());
    const mq = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)');
    if (mq && mq.addEventListener) mq.addEventListener('change', () => { this.paused = mq.matches; this.kick(); });
    if (window.matchMedia) {
      const cs = window.matchMedia('(prefers-color-scheme: dark)');
      if (cs.addEventListener) cs.addEventListener('change', () => this.draw());
    }
    new MutationObserver(() => this.draw()).observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    if (document.fonts && document.fonts.ready) document.fonts.ready.then(() => this.resize());
    this.kick();
  }

  Field.prototype.resize = function () {
    const box = this.c.parentElement || this.c;
    const width = Math.max(200, box.clientWidth);
    let cols = this.o.cols;
    // On a narrow screen keep the cells legible: fewer columns, not smaller glyphs.
    const minCell = this.o.minCell || 5.2;
    if (width / cols < minCell) cols = Math.max(this.o.minCols, Math.floor(width / minCell));
    const rows = this.o.fixedRows ? this.o.rows : Math.max(10, Math.round(this.o.rows * (cols / this.o.cols) ** 0.35));
    const family = token(this.c, '--font-mono', 'monospace');
    const ctx = this.ctx;
    ctx.font = `100px ${family}`;
    const adv = ctx.measureText('M').width / 100 || 0.6; // advance per px of font size
    const cw = width / cols, size = cw / adv, ch = cw * 2; // a terminal cell is about 1:2
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    this.c.width = Math.round(width * dpr); this.c.height = Math.round(rows * ch * dpr);
    this.c.style.width = width + 'px'; this.c.style.height = rows * ch + 'px';
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    this.m = { cols, rows, cw, ch, size, family };
    if (!this.G || this.G.W !== cols || this.G.H !== rows) this.G = new Grid(cols, rows);
    this.draw();
  };

  Field.prototype.kick = function () {
    const run = !this.paused && this.visible && document.visibilityState !== 'hidden';
    if (run && !this.raf) {
      this.last = null;
      this.raf = requestAnimationFrame((ts) => this.tick(ts));
    }
    if (!run && this.paused) this.draw();
  };

  Field.prototype.tick = function (ts) {
    this.raf = 0;
    if (this.paused || !this.visible || document.visibilityState === 'hidden') return;
    if (this.last != null) {
      const dt = Math.min(0.1, (ts - this.last) / 1000);
      this.time += dt * this.o.speed;
      const fps = dt > 0 ? 1 / dt : 0;
      this.stats.fps = this.stats.fps ? this.stats.fps * 0.92 + fps * 0.08 : fps;
    }
    this.last = ts;
    this.draw();
    this.raf = requestAnimationFrame((t) => this.tick(t));
  };

  Field.prototype.play = function () { this.paused = false; this.kick(); };
  Field.prototype.pause = function () { this.paused = true; this.draw(); };
  Field.prototype.toggle = function () { this.paused ? this.play() : this.pause(); return !this.paused; };

  Field.prototype.draw = function () {
    if (!this.m) return;
    const { cols, rows, cw, ch, size, family } = this.m, G = this.G, ctx = this.ctx;
    const t0 = performance.now();
    G.clear(); this.fn(G, this.time);
    const bg = token(this.c, '--field-bg', '#07080c');
    const ink = token(this.c, '--field-ink', '#c9cdd8');
    const lit = token(this.c, '--field-lit', '#9d8cff');
    const litInk = token(this.c, '--field-lit-ink', bg);
    ctx.fillStyle = bg; ctx.fillRect(0, 0, cols * cw, rows * ch);
    ctx.font = `${size}px ${family}`; ctx.textBaseline = 'middle';
    // Ink: one fillText per row, with lit cells blanked out of it.
    ctx.fillStyle = ink;
    for (let y = 0; y < rows; y++) {
      let s = '';
      for (let x = 0; x < cols; x++) { const i = y * cols + x; s += G.hot[i] ? ' ' : G.g[i]; }
      // The font size is chosen so one advance is exactly one cell, so a row is one call.
      if (s.trim()) ctx.fillText(s, 0, y * ch + ch / 2);
    }
    // Lit: the selection block, then the glyph knocked out in the background colour.
    for (let y = 0; y < rows; y++) {
      let x = 0;
      while (x < cols) {
        if (!G.hot[y * cols + x]) { x++; continue; }
        const a = x; while (x < cols && G.hot[y * cols + x]) x++;
        ctx.fillStyle = lit; ctx.fillRect(a * cw, y * ch, (x - a) * cw + 0.5, ch + 0.5);
        ctx.fillStyle = litInk;
        ctx.fillText(G.g.slice(y * cols + a, y * cols + x).join(''), a * cw, y * ch + ch / 2);
      }
    }
    // The caret is drawn, not typed: a thin lit bar at the left of its cell.
    if (G.caret) { const n = G.caret[2] || 1; ctx.fillStyle = lit; ctx.fillRect(G.caret[0] * cw, G.caret[1] * ch + (n > 1 ? 0 : ch * 0.12), Math.max(1.5, cw * (n > 1 ? 0.55 : 0.2)), n > 1 ? n * ch : ch * 0.76); }
    // A frame's cost here: the scene's maths and the canvas paint together.
    const ms = performance.now() - t0;
    this.stats.ms = this.stats.ms ? this.stats.ms * 0.9 + ms * 0.1 : ms;
    this.stats.frames++;
    if (this.o.onStats) this.o.onStats(this.stats, this);
  };

  window.CaretlineField = { Field, Grid, SCENES, makeWarp, rng, reduced };
})();
