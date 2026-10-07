// Renders the tour preview on the landing page: `caretline demo --snapshot` after a growing
// key script, one frame per step, converted from ANSI to HTML spans. Writes
// public/tour/frames.json (committed, so the site builds without the binary).
//
//   node scripts/tour-frames.mjs            # uses ../../target/release/caretline, else `caretline`
//   CARETLINE=/path/to/caretline node scripts/tour-frames.mjs
import { execFileSync } from 'node:child_process';
import { existsSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const local = join(here, '../../../target/release/caretline');
const bin = process.env.CARETLINE || (existsSync(local) ? local : 'caretline');
const SIZE = '72x16';

// [keys added in this step, how long the frame shows (ms)]
const typed = ' Lines wrap at word boundaries, never mid-word.';
const steps = [['', 1600]];
for (let i = 0; i < typed.length; i += 3) steps.push([typed.slice(i, i + 3), 70]);
const down = (n) => '<down>'.repeat(n);
steps.push(
  ['', 700],
  [down(1), 900],
  [down(17) + '<home>', 900],
  ['<c-n>', 500],
  ['<c-n>', 900],
);
for (const ch of ['tw', 'o ']) steps.push([ch, 200]);
steps.push(
  ['', 1100],
  ['<c-z>', 1200],
  ['<esc>', 500],
  [down(10), 900],
  ['<tab>', 900],
  ['<s-tab>', 700],
  [down(5), 700],
  ['<c-o>', 1300],
  ['<c-o>', 800],
  ['<end><c-g>', 1200],
  [' (and here)', 1400],
  ['<c-g>', 800],
  ['<c-d>', 2600],
);

const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
const CLASS = { '': '', '7;4': 'c', '30;46': 's', '30;47': 'b', '1;30;47': 'ba', '2': 'h' };

function html(ansi) {
  const rows = ansi.replace(/\n$/, '').split('\n');
  return rows
    .map((row) => {
      let out = '';
      for (const part of row.split('\x1b[')) {
        const m = part.match(/^([0-9;]*)m([\s\S]*)$/);
        if (!m) { out += esc(part); continue; }
        const [, code, text] = m;
        if (!text) continue;
        const cls = CLASS[code === '0' ? '' : code] ?? '';
        out += cls ? `<span class="${cls}">${esc(text)}</span>` : esc(text);
      }
      return out;
    })
    .join('\n');
}

let keys = '';
const frames = [];
for (const [k, ms] of steps) {
  keys += k;
  const ansi = execFileSync(bin, ['demo', '--snapshot', SIZE, '--format', 'ansi', '--keys', keys], { encoding: 'utf8' });
  frames.push({ ms, html: html(ansi) });
}
const version = execFileSync(bin, ['--version'], { encoding: 'utf8' }).trim();
writeFileSync(join(here, '../public/tour/frames.json'), JSON.stringify({ size: SIZE, version, frames }) + '\n');
console.log(`tour-frames: ${frames.length} frames from ${version}`);
