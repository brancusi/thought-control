/* The caretline playground: the real engine (crates/caretline-next), compiled to
 * WebAssembly, driven over its own state protocol in the page.
 *
 * Session 0 is the live editor. Keys become key-script tokens (`<s-left>`, `<d-z>`, plain
 * characters) and go in as `keys` requests with the page's clock as `now_ms`. Every response
 * is shown as it comes back: the messages the engine applied, its state, and its frame.
 * Session 1 is for time travel: the scrubber replays the live trace into it, message by
 * message, and renders what the editor looked like at that rev.
 */
(function () {
  'use strict';
  const enc = new TextEncoder(), dec = new TextDecoder();

  async function load(url) {
    const res = await fetch(url);
    if (!res.ok) throw new Error(`could not load the engine (${res.status})`);
    const bytes = await res.arrayBuffer();
    const { instance } = await WebAssembly.instantiate(bytes, {});
    const e = instance.exports;
    return function call(sid, req) {
      const b = enc.encode(JSON.stringify(req));
      const p = e.cl_alloc(b.length);
      new Uint8Array(e.memory.buffer, p, b.length).set(b);
      const n = e.cl_handle(sid, p, b.length);
      const line = dec.decode(new Uint8Array(e.memory.buffer, e.cl_out_ptr(), n));
      return JSON.parse(line);
    };
  }

  // ── keys → key-script tokens ──────────────────────────────────────────
  const NAMED = {
    Enter: 'cr', Backspace: 'bs', Delete: 'del', Tab: 'tab', Escape: 'esc',
    ArrowLeft: 'left', ArrowRight: 'right', ArrowUp: 'up', ArrowDown: 'down',
    Home: 'home', End: 'end', PageUp: 'pgup', PageDown: 'pgdn',
  };
  const isMac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
  // Command chords the engine binds; anything else stays with the browser (reload, tabs…).
  const CHORDS = new Set(['a', 'c', 'x', 'z', 'y', 's']);

  function token(e) {
    const named = NAMED[e.key];
    const mods = (e.shiftKey ? 's-' : '') + (e.ctrlKey ? 'c-' : '') + (e.altKey ? 'a-' : '') + (e.metaKey ? 'd-' : '');
    if (named) return mods ? `<${mods}${named}>` : `<${named}>`;
    const cmd = isMac ? e.metaKey : e.ctrlKey;
    if (cmd && !e.altKey) {
      const k = (e.code && e.code.startsWith('Key') ? e.code.slice(3) : e.key).toLowerCase();
      if (!CHORDS.has(k)) return null;
      return `<${e.shiftKey ? 's-' : ''}${isMac ? 'd-' : 'c-'}${k}>`;
    }
    return null; // printable text arrives through the input event
  }
  const literal = (s) => s.replace(/</g, '<lt>').replace(/>/g, '<gt>').replace(/\r\n?/g, '\n');

  // ── drawing a frame from `cells` rows ─────────────────────────────────
  const WIDE = /\p{Extended_Pictographic}|[ᄀ-ᅟ⺀-꓏가-힣豈-﫿︰-﹏＀-｠￠-￦]/u;
  const seg = window.Intl && Intl.Segmenter ? new Intl.Segmenter() : null;
  const graphemes = (s) => (seg ? Array.from(seg.segment(s), (x) => x.segment) : Array.from(s));
  const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

  function frameHTML(result, showCaret) {
    const cur = result.cursor;
    return result.rows.map((row, y) => {
      const roles = [];
      for (const [x, len, role] of row.spans || []) for (let k = x; k < x + len; k++) roles[k] = role;
      let html = '', x = 0, open = null, buf = '';
      const flush = () => { if (buf) html += open ? `<span class="r-${open}">${esc(buf)}</span>` : esc(buf); buf = ''; };
      for (const g of graphemes(row.text)) {
        const role = roles[x] || null;
        const caret = showCaret && cur && cur[1] === y && cur[0] === x;
        if (role !== open || caret) { flush(); open = role; }
        if (caret) html += `<span class="caret${role ? ' r-' + role : ''}">${esc(g)}</span>`;
        else buf += g;
        x += WIDE.test(g) ? 2 : 1;
      }
      flush();
      if (showCaret && cur && cur[1] === y && cur[0] >= x) html += '<span class="caret"> </span>';
      return `<span class="row${row.info && row.info.kind === 'status' ? ' row--status' : ''}">${html}</span>`;
    }).join('\n');
  }

  // The state, as the protocol returns it, with the bulky parts summarised.
  function stateHTML(state) {
    const s = Object.assign({}, state);
    const hist = s.history && s.history.revisions ? s.history.revisions.length : 0;
    if (s.history) s.history = `‹${hist} revision${hist === 1 ? '' : 's'}, current ${state.history.current}›`;
    if (s.mark_log) s.mark_log = '‹…›';
    if (s.selection && s.selection.ranges) {
      s.selection = Object.assign({}, s.selection, { ranges: s.selection.ranges.map((r) => ({ anchor: r.anchor, head: r.head })) });
    }
    const json = JSON.stringify(s, (k, v) => (v === null ? undefined : v), 2);
    return esc(json)
      .replace(/^(\s*)&quot;([a-z_]+)&quot;:/gm, '$1<span class="j-k">"$2"</span>:')
      .replace(/: (&quot;.*?&quot;)(,?)$/gm, ': <span class="j-s">$1</span>$2')
      .replace(/: (-?\d+(?:\.\d+)?|true|false)(,?)$/gm, ': <span class="j-n">$1</span>$2');
  }

  function msgHTML(m) {
    const json = JSON.stringify(m);
    return esc(json).replace(/&quot;msg&quot;:&quot;([a-z_]+)&quot;/, '<span class="j-k">"msg"</span>:<span class="j-m">"$1"</span>');
  }

  // ── the playground ────────────────────────────────────────────────────
  const SAMPLE = '# caretline\n\nThe whole editor is one state.\nEvery keystroke is a message.\n\nClick here and type. Undo with ⌘Z or Ctrl-Z.\n';

  async function mount(root) {
    const $ = (s) => root.querySelector(s);
    const els = {
      frame: $('[data-frame]'), input: $('[data-input]'), msgs: $('[data-msgs]'), state: $('[data-state]'),
      rev: $('[data-rev]'), scrub: $('[data-scrub]'), mode: $('[data-mode]'), status: $('[data-status]'),
      replay: $('[data-replay]'), reset: $('[data-reset]'), copy: $('[data-copy]'), stats: $('[data-stats]'),
    };
    let call;
    try {
      call = await load(root.dataset.wasm);
    } catch (err) {
      root.classList.add('is-offline');
      els.frame.textContent = `The engine didn't load: ${err.message}. The rest of the page works without it.`;
      return;
    }
    root.classList.add('is-live');

    const LIVE = 0, REPLAY = 1;
    const size = () => {
      const cs = getComputedStyle(els.frame);
      const w = els.frame.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight);
      const probe = document.createElement('span'); probe.textContent = 'M'.repeat(20);
      probe.style.font = cs.font; probe.style.position = 'absolute'; probe.style.visibility = 'hidden';
      document.body.appendChild(probe); const cw = probe.getBoundingClientRect().width / 20; probe.remove();
      metrics = { cw, lh: parseFloat(cs.lineHeight), pl: parseFloat(cs.paddingLeft), pt: parseFloat(cs.paddingTop) };
      return { w: Math.max(24, Math.min(80, Math.floor((w - 2) / cw))), h: 12 };
    };
    let metrics = null;
    let vp = size();
    const t0 = performance.now();
    const now = () => Math.round(performance.now() - t0) + 1000;
    let log = []; // [{rev, msgs}] newest last, from responses
    let scrubAt = null; // null = live
    let demo = null;

    function reset(text) {
      call(LIVE, { op: 'state.set', state: { text, viewport: { width: vp.w, height: vp.h }, selection: { ranges: [{ anchor: text.length, head: text.length }] } } });
      log = [];
      scrubAt = null;
      paint();
    }

    function send(req) {
      const r = call(LIVE, req);
      if (r.error) { flash(`${r.error.kind}: ${r.error.message}`); return r; }
      const res = r.result;
      if (res.msgs && res.msgs.length) log.push({ rev: res.rev, msgs: res.msgs });
      if (log.length > 200) log = log.slice(-200);
      for (const fx of res.effects || []) effect(fx);
      paint();
      return r;
    }

    function effect(fx) {
      if (fx.effect === 'clipboard_set' && navigator.clipboard) navigator.clipboard.writeText(fx.text).catch(() => {});
      const names = { clipboard_set: 'the page put it on your clipboard', write_file: 'a runtime would write the file; this page has none', quit: 'a runtime would quit here' };
      flash(`effect ${fx.effect}: ${names[fx.effect] || 'handed to the runtime'}`);
    }

    let flashTimer = 0;
    function flash(text) {
      els.status.textContent = text; els.status.hidden = false;
      clearTimeout(flashTimer); flashTimer = setTimeout(() => { els.status.hidden = true; }, 2600);
    }

    function paint() {
      const sid = scrubAt == null ? LIVE : REPLAY;
      const r = call(sid, { op: 'render', w: vp.w, h: vp.h, format: 'cells' }).result;
      const st = call(sid, { op: 'state.get' }).result;
      const liveRev = scrubAt == null ? r.rev : call(LIVE, { op: 'hello' }).result.rev;
      els.frame.innerHTML = frameHTML(r, true);
      els.state.innerHTML = stateHTML(st.state);
      const items = [];
      for (const { rev, msgs } of log.slice(-40)) {
        msgs.forEach((m, i) => items.push(`<li${scrubAt != null && rev > scrubAt ? ' class="is-future"' : ''}><span class="rev">${i === msgs.length - 1 ? rev : ''}</span><code>${msgHTML(m)}</code></li>`));
      }
      els.msgs.innerHTML = items.slice(-28).join('') || '<li class="empty">Messages appear here as you type.</li>';
      els.msgs.scrollTop = els.msgs.scrollHeight;
      els.rev.textContent = scrubAt == null ? `rev ${liveRev}` : `rev ${scrubAt} of ${liveRev}`;
      els.mode.textContent = scrubAt == null ? 'live' : 'replay';
      root.classList.toggle('is-replay', scrubAt != null);
      const base = traceBase();
      els.scrub.min = base; els.scrub.max = liveRev; els.scrub.value = scrubAt == null ? liveRev : scrubAt;
      els.scrub.disabled = liveRev <= base;
    }

    // The live trace's current segment: a state line, then one message per rev.
    let traceCache = null;
    function trace() {
      const r = call(LIVE, { op: 'trace.get' }).result;
      if (!traceCache || traceCache.rev !== r.rev) traceCache = r;
      return traceCache;
    }
    function traceBase() { return trace().from_rev; }

    // Replays the live trace into the replay session up to `rev`, one message per rev.
    function scrubTo(rev) {
      const tr = trace();
      if (rev >= tr.rev) { scrubAt = null; paint(); return; }
      const lines = tr.trace;
      const first = lines[0];
      call(REPLAY, { op: 'state.set', state: first.state });
      const msgs = [];
      for (const line of lines.slice(1, 1 + (rev - tr.from_rev))) if (line.msg) msgs.push(line.msg);
      if (msgs.length) call(REPLAY, { op: 'msgs', msgs });
      scrubAt = rev;
      paint();
    }

    // Typing while scrubbed back branches from there: the replayed state becomes live.
    function branch() {
      if (scrubAt == null) return;
      const st = call(REPLAY, { op: 'state.get' }).result.state;
      call(LIVE, { op: 'state.set', state: st });
      log = log.filter((e) => e.rev <= scrubAt);
      scrubAt = null;
      flash('branched: the replayed state is live again, and the trace starts from it');
    }

    function keys(script) { stopDemo(); branch(); send({ op: 'keys', keys: script, now_ms: now() }); }

    els.input.addEventListener('keydown', (e) => {
      if (e.isComposing) return;
      const t = token(e);
      if (!t) return;
      e.preventDefault();
      keys(t);
    });
    els.input.addEventListener('input', () => {
      const v = els.input.value; els.input.value = '';
      if (v) keys(literal(v));
    });
    els.input.addEventListener('paste', (e) => {
      const text = e.clipboardData && e.clipboardData.getData('text/plain');
      if (text == null) return;
      e.preventDefault(); stopDemo(); branch();
      send({ op: 'msgs', msgs: [{ msg: 'paste', text }], now_ms: now() });
    });
    const focus = () => { els.input.focus({ preventScroll: true }); };
    els.input.addEventListener('mousedown', (e) => {
      // A click is a message too: the cell under the pointer.
      const rect = els.frame.getBoundingClientRect();
      const col = Math.max(0, Math.floor((e.clientX - rect.left - metrics.pl) / metrics.cw));
      const row = Math.max(0, Math.floor((e.clientY - rect.top - metrics.pt) / metrics.lh));
      if (row < vp.h - 1) { stopDemo(); branch(); send({ op: 'msgs', msgs: [{ msg: 'click', col, row, extend: e.shiftKey }], now_ms: now() }); }
      e.preventDefault(); focus();
    });
    els.input.addEventListener('focus', () => root.classList.add('is-focused'));
    els.input.addEventListener('blur', () => root.classList.remove('is-focused'));

    els.scrub.addEventListener('input', () => { stopDemo(); scrubTo(+els.scrub.value); });
    els.replay.addEventListener('click', () => {
      stopDemo();
      const tr = trace(); let k = tr.from_rev;
      const step = () => { if (k > tr.rev) { scrubAt = null; paint(); return; } scrubTo(k++); demo = setTimeout(step, 45); };
      step();
    });
    els.reset.addEventListener('click', () => { stopDemo(); reset(SAMPLE); flash('a fresh state: state.set with only text, a selection and a viewport'); });
    els.copy.addEventListener('click', () => {
      const tr = trace();
      const text = tr.trace.map((l) => JSON.stringify(l)).join('\n') + '\n';
      const done = () => flash(`copied ${tr.trace.length} trace lines: replay them with caretline --replay`);
      if (navigator.clipboard) navigator.clipboard.writeText(text).then(done, () => flash('the browser refused the clipboard'));
    });

    window.addEventListener('resize', () => {
      const n = size(); if (n.w === vp.w) return; vp = n;
      call(LIVE, { op: 'msgs', msgs: [{ msg: 'resize', width: vp.w, height: vp.h }] });
      paint();
    });

    // ── the demo: typed through the same `keys` requests, until you take over ──
    const SCRIPT = [
      ['<d-down>', 300], ['Agents type here too.', 40], ['<wait:0>', 600],
      ['<s-home>', 700], ['Scripts, tests and agents type here too.', 32],
      ['<wait:0>', 700], ['<d-z>', 800], ['<d-s-z>', 900], ['<cr>', 300], ['Drag the scrubber to go back in time.', 35],
    ];
    function stopDemo() { if (demo) { clearTimeout(demo); demo = null; } }
    function runDemo() {
      let i = 0, j = 0;
      const step = () => {
        if (i >= SCRIPT.length) { demo = null; return; }
        const [s, gap] = SCRIPT[i];
        if (s.startsWith('<')) { send({ op: 'keys', keys: s, now_ms: now() }); i++; demo = setTimeout(step, gap); return; }
        const ch = [...s][j++];
        send({ op: 'keys', keys: literal(ch), now_ms: now() });
        if (j >= [...s].length) { i++; j = 0; }
        demo = setTimeout(step, gap);
      };
      step();
    }

    reset(SAMPLE);
    const reduce = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    if (!reduce && window.IntersectionObserver) {
      const io = new IntersectionObserver((es) => {
        if (es[0].isIntersecting) { io.disconnect(); demo = setTimeout(runDemo, 600); }
      }, { threshold: 0.4 });
      io.observe(root);
    }
  }

  window.CaretlinePlayground = { mount, load };
  document.querySelectorAll('[data-playground]').forEach((el) => mount(el));
})();
