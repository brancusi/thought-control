// The landscape board's vocabulary: the axes every tool is scored on (0–5, one rubric for all),
// the groups, the verdicts, and small helpers the pages share. The dossiers themselves are data
// in src/content/landscape/.

export interface Axis {
  key: string;
  label: string;
  short: string;
  /** What 0, 3 and 5 mean, as written in the research rubric. */
  rubric: string;
}

export const AXES: Axis[] = [
  { key: 'ui_as_data', label: 'UI as data', short: 'data', rubric: '0 the UI exists only as code · 3 declarative, but in a host language (JSX, classes) · 5 the whole UI is a serializable value you can store, diff and send' },
  { key: 'agent_drivable', label: 'Agent can drive it', short: 'drive', rubric: '0 nothing · 1 an agent can only write source code · 3 a protocol, MCP server or API for some operations · 5 first-class agent control of a live instance' },
  { key: 'readback', label: 'Agent can see it', short: 'see', rubric: '0 the agent can’t see the result · 2 screenshots or screen scraping · 4 structured state readable · 5 structured state and the rendered view, on demand' },
  { key: 'granular_edits', label: 'Small, targeted edits', short: 'edit', rubric: '0 regenerate everything · 3 patches exist, but coarse or one-way · 5 small id- or pointer-addressed edits to a live instance, cheap and frequent' },
  { key: 'time_travel', label: 'Time travel', short: 'time', rubric: '0 none · 2 undo/redo · 3 time travel for humans (devtools) · 4 an agent can read history and seek · 5 an agent can read, seek, fork and replay' },
  { key: 'live_data_rate', label: 'Live data rate', short: 'rate', rubric: '0 static · 2 updates every few seconds · 3 tens a second · 4 hundreds a second, measured · 5 thousands a second, published, with data that bypasses the model' },
  { key: 'terminal_native', label: 'Terminal native', short: 'term', rubric: '0 web or desktop only · 3 has a terminal renderer · 5 built for the terminal' },
  { key: 'pixel_graphics', label: 'Pixel graphics', short: 'px', rubric: '0 none · 2 through an add-on · 4 built-in images and graphics · 5 rich pixel rendering (anti-aliasing, compositing) natively' },
  { key: 'teaching', label: 'Teaching', short: 'teach', rubric: '0 none · 2 could be used for it · 4 has tours, walkthroughs or narration · 5 built around teaching (record, narrate, take over, rewind)' },
  { key: 'maturity', label: 'Maturity', short: 'mature', rubric: '0 an idea · 1 prototype · 2 early, small use · 3 in production somewhere · 4 widely used · 5 an ecosystem standard' },
  { key: 'openness', label: 'Openness', short: 'open', rubric: '0 proprietary · 2 source-available or restrictive · 4 permissive open source under company control · 5 permissive and community-governed' },
];

/** Composite axes for the map: averages of related scores. */
export const COMPOSITES: { key: string; label: string; of: string[] }[] = [
  { key: 'agent_loop', label: 'Agent loop (drive + see + edit + time)', of: ['agent_drivable', 'readback', 'granular_edits', 'time_travel'] },
  { key: 'live_ui', label: 'Live UI (data + rate + edits)', of: ['ui_as_data', 'live_data_rate', 'granular_edits'] },
  { key: 'terminal_craft', label: 'Terminal craft (native + pixels)', of: ['terminal_native', 'pixel_graphics'] },
];

export const GROUPS: Record<string, { label: string; blurb: string }> = {
  self: { label: 'thc-scene', blurb: 'The thing being positioned.' },
  'terminal-frameworks': { label: 'Terminal UI frameworks', blurb: 'Libraries people write terminal apps with.' },
  'agent-ui-specs': { label: 'Agent UI specs', blurb: 'Formats and protocols for UIs an agent generates.' },
  'agent-canvases': { label: 'Agent canvases & live UIs', blurb: 'Surfaces an agent operates directly, or drives from outside.' },
  'time-travel': { label: 'Time travel & replay', blurb: 'Tools that record, rewind and replay state.' },
  'teaching-and-live-data': { label: 'Teaching & live data', blurb: 'Tools for explaining things, and for fast-moving data.' },
};

export const GROUP_ORDER = ['self', 'terminal-frameworks', 'agent-ui-specs', 'agent-canvases', 'time-travel', 'teaching-and-live-data'];

export const VERDICTS: Record<string, string> = {
  use: 'Use it: it already does the job; point people to it.',
  adopt: 'Adopt it: build on it or speak its format.',
  borrow: 'Borrow from it: take a specific idea, API or format.',
  complement: 'Complement it: it does a neighbouring job; interoperate.',
  ignore: 'Ignore it for this purpose.',
};

export type Dossier = Record<string, any> & { id: string; name: string; group: string; tagline: string; scores: Record<string, number> };

export function score(d: Dossier, key: string, planned = false): number {
  const s = planned && d.planned_scores?.[key] != null ? d.planned_scores : d.scores;
  const c = COMPOSITES.find((c) => c.key === key);
  if (c) return c.of.reduce((a, k) => a + (s[k] ?? 0), 0) / c.of.length;
  return s[key] ?? 0;
}

export function axisLabel(key: string): string {
  return AXES.find((a) => a.key === key)?.label ?? COMPOSITES.find((c) => c.key === key)?.label ?? key;
}

export function stars(n: number | null | undefined): string {
  if (n == null) return '—';
  return n >= 1000 ? `${(n / 1000).toFixed(n >= 10000 ? 0 : 1)}k` : String(n);
}

/** Points of a radar polygon (centre cx,cy, radius r) for the scores, axis 0 at the top. */
export function radarPoints(scores: Record<string, number>, cx: number, cy: number, r: number): string {
  return AXES.map((a, i) => {
    const t = (Math.PI * 2 * i) / AXES.length - Math.PI / 2;
    const v = Math.max(0, Math.min(5, scores[a.key] ?? 0)) / 5;
    return `${(cx + Math.cos(t) * r * v).toFixed(1)},${(cy + Math.sin(t) * r * v).toFixed(1)}`;
  }).join(' ');
}

export function host(url: string): string {
  try { return new URL(url).host.replace(/^www\./, ''); } catch { return url; }
}
