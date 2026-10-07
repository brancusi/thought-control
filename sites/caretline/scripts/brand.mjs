// Writes the caretline logo files to public/brand/ from the Geist Mono outlines, so the SVGs
// need no font. Run: npm run brand
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import opentype from 'opentype.js';

const require = createRequire(import.meta.url);
const fontFile = (w) => require.resolve(`@fontsource/geist-mono/files/geist-mono-latin-${w}-normal.woff`);
const out = path.resolve('public/brand');
fs.mkdirSync(out, { recursive: true });

// The palette, as in src/styles/tokens.css.
const C = {
  dark: { void: '#08090D', ink: '#DCDFE8', dim: '#7D8396', lit: '#A396FF' },
  light: { void: '#F5F6F9', ink: '#141720', dim: '#5D6377', lit: '#5338F0' },
};

const fbuf = fs.readFileSync(fontFile(500));
const font = opentype.parse(fbuf.buffer.slice(fbuf.byteOffset, fbuf.byteOffset + fbuf.length));
const SIZE = 100; // font size in SVG units
const adv = (font.charToGlyph('M').advanceWidth / font.unitsPerEm) * SIZE; // one cell
const CELL_H = adv * 2; // a terminal cell is about 1:2
const r = (n) => Math.round(n * 100) / 100;

// Text as a path, one glyph per cell, vertically centred in a cell row at y.
function textPath(str, x0, y) {
  const capH = (font.tables.os2.sCapHeight / font.unitsPerEm) * SIZE;
  const baseline = y + CELL_H / 2 + capH / 2;
  let d = '';
  [...str].forEach((ch, i) => {
    if (ch === ' ') return;
    d += font.getPath(ch, x0 + i * adv, baseline, SIZE).toPathData(2);
  });
  return d;
}

const WORD = 'C A R E T L I N E';
const PAD = 1; // cells of selection around the word
const bandW = (WORD.length + PAD * 2) * adv;
const caretW = adv * 0.22;
const caretX = bandW + adv * 0.9;
const totalW = caretX + caretW;

function wordmark(theme, { selected = true, bg = false } = {}) {
  const c = C[theme];
  const m = adv * 0.6; // margin when a background is drawn
  const W = r(totalW + (bg ? m * 2 : 0)), H = r(CELL_H + (bg ? m * 2 : 0));
  const ox = bg ? m : 0, oy = bg ? m : 0;
  const letters = textPath(WORD, ox + PAD * adv, oy);
  const id = `cut-${theme}`;
  const body = selected
    ? `<mask id="${id}"><rect x="${r(ox)}" y="${r(oy)}" width="${r(bandW)}" height="${r(CELL_H)}" fill="#fff"/><path d="${letters}" fill="#000"/></mask>
  <rect x="${r(ox)}" y="${r(oy)}" width="${r(bandW)}" height="${r(CELL_H)}" fill="${c.lit}" mask="url(#${id})"/>`
    : `<path d="${letters}" fill="${c.ink}"/>`;
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" width="${r(W / 4)}" height="${r(H / 4)}" role="img" aria-label="caretline">
  <title>caretline</title>${bg ? `\n  <rect width="${W}" height="${H}" fill="${c.void}"/>` : ''}
  ${body}
  <rect x="${r(ox + caretX)}" y="${r(oy + CELL_H * 0.08)}" width="${r(caretW)}" height="${r(CELL_H * 0.84)}" fill="${c.lit}"/>
</svg>
`;
}

// The mark: a caret at the centre of a warp field. Streaks taper toward the centre, the way
// the stars in the hero stream outward; a few stars are still dots. None cross the caret.
const STREAKS = [
  // angle (deg, 0 = right, clockwise), inner radius, outer radius, outer width
  [-24, 12, 29, 4.2], [16, 15, 26, 3.2], [150, 13, 29, 4.2], [196, 15, 25, 3.2],
  [56, 16, 27, 3.4], [236, 16, 27, 3.4],
];
const DOTS = [[-62, 19, 1.7], [112, 21, 1.7], [-118, 24, 1.4], [84, 26, 1.3]];
const polar = (a, rr) => [32 + Math.cos((a * Math.PI) / 180) * rr, 32 + Math.sin((a * Math.PI) / 180) * rr];
function streak(a, r0, r1, w) {
  const t = (a * Math.PI) / 180, nx = -Math.sin(t), ny = Math.cos(t);
  const [x0, y0] = polar(a, r0), [x1, y1] = polar(a, r1);
  const p = [[x0 + nx * 0.3, y0 + ny * 0.3], [x1 + nx * w / 2, y1 + ny * w / 2], [x1 - nx * w / 2, y1 - ny * w / 2], [x0 - nx * 0.3, y0 - ny * 0.3]];
  return p.map(([x, y]) => `${r(x)},${r(y)}`).join(' ');
}
function mark(theme, { tile = true, small = false } = {}) {
  const c = C[theme];
  const ss = small ? [[-24, 11, 30, 7], [150, 11, 30, 7], [40, 15, 28, 5.5], [220, 15, 28, 5.5]] : STREAKS;
  const parts = ss.map(([a, r0, r1, w]) => `<polygon points="${streak(a, r0, r1, w)}" fill="${c.ink}"/>`);
  if (!small) for (const [a, rr, d] of DOTS) { const [x, y] = polar(a, rr); parts.push(`<circle cx="${r(x)}" cy="${r(y)}" r="${d}" fill="${c.ink}" fill-opacity="0.7"/>`); }
  const caret = small
    ? `<rect x="27" y="11" width="10" height="42" fill="${c.lit}"/>`
    : `<rect x="29" y="13" width="6" height="38" fill="${c.lit}"/>`;
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64" role="img" aria-label="caretline">
  <title>caretline</title>${tile ? `\n  <rect width="64" height="64" rx="${small ? 12 : 14}" fill="${c.void}"/>` : ''}
  ${parts.join('\n  ')}
  ${caret}
</svg>
`;
}

const files = {
  'caretline-wordmark-dark.svg': wordmark('dark'),
  'caretline-wordmark-light.svg': wordmark('light'),
  'caretline-wordmark-plain-dark.svg': wordmark('dark', { selected: false }),
  'caretline-wordmark-plain-light.svg': wordmark('light', { selected: false }),
  'caretline-lockup-dark.svg': wordmark('dark', { bg: true }),
  'caretline-lockup-light.svg': wordmark('light', { bg: true }),
  'caretline-mark-dark.svg': mark('dark'),
  'caretline-mark-light.svg': mark('light'),
  'caretline-mark-bare-dark.svg': mark('dark', { tile: false }),
  'caretline-mark-bare-light.svg': mark('light', { tile: false }),
  'favicon.svg': mark('dark', { small: true }),
};
for (const [name, svg] of Object.entries(files)) fs.writeFileSync(path.join(out, name), svg);
fs.copyFileSync(path.join(out, 'favicon.svg'), path.resolve('public/favicon.svg'));
console.log(`wrote ${Object.keys(files).length} files to ${out} (cell ${r(adv)} × ${r(CELL_H)})`);
