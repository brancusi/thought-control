# Dossier spec (shared by every research agent)

Write ONE JSON file per tool to `RESEARCH/dossiers/<id>.json` (id = lowercase-kebab, e.g. `bubble-tea`). Exactly this shape (omit nothing; use null or [] when unknown, never guess):

```json
{
  "id": "bubble-tea",
  "name": "Bubble Tea",
  "group": "terminal-frameworks | agent-ui-specs | agent-canvases | time-travel | teaching-and-live-data",
  "tagline": "≤ 12 words, plain",
  "links": {"site": "", "repo": "", "docs": "", "other": [{"label": "", "url": ""}]},
  "license": {"spdx": "MIT", "notes": "e.g. dual-licensed, source-available, proprietary SaaS, CLA"},
  "language": "Go",
  "backing": "company / foundation / individual(s); funding if known",
  "first_release": "YYYY-MM or null",
  "latest_release": {"tag": "", "date": "YYYY-MM-DD"},
  "adoption": {
    "github_stars": 0, "stars_fetched": "2026-10-09",
    "downloads": "e.g. '1.2M/week npm (ink)' with source, or null",
    "notable_users": ["Crush", "…"],
    "summary": "1–2 sentences"
  },
  "maintainability": {
    "release_cadence": "e.g. monthly; last 5 releases dates",
    "contributors": "number from GitHub if available",
    "recent_activity": "commits / PRs merged in last 90 days if you can get it",
    "bus_factor": "low/medium/high + why",
    "summary": "1–2 sentences"
  },
  "architecture": {"summary": "3–5 sentences: model, rendering, state, extension points", "key_points": ["…"]},
  "performance": {"claims": [{"text": "", "source": ""}], "summary": "what is measured vs claimed; 'no published numbers' if so"},
  "agent_usability": {
    "how": "how an agent drives/integrates it today: MCP server? API? CLI? code generation only? screen scraping?",
    "integration_effort": "what it takes to wire an agent in",
    "readback": "can the agent see the current UI/state? how",
    "granular_edits": "can the agent make small targeted changes? addressing (ids, JSON pointer, refs)",
    "history": "time travel / undo / replay / fork available to the agent?",
    "summary": "1–2 sentences"
  },
  "scores": {
    "ui_as_data": 0, "agent_drivable": 0, "readback": 0, "granular_edits": 0, "time_travel": 0,
    "live_data_rate": 0, "terminal_native": 0, "pixel_graphics": 0, "teaching": 0, "maturity": 0, "openness": 0
  },
  "score_notes": {"<axis>": "one line justifying each non-obvious score"},
  "steelman": "One strong paragraph: the best honest case FOR this tool, as its biggest fan would make it.",
  "weaknesses": ["honest, sourced where possible"],
  "relation_to_scene": {
    "overlap": "what it shares with thc-scene",
    "scene_reinvents": "what thc-scene would be re-doing that this already does well (be blunt)",
    "borrow": ["concrete idea/API/format to take from it"],
    "gap_left": "the specific thing this tool does NOT do that thc-scene does — or 'none'",
    "verdict": "use | adopt | borrow | complement | ignore",
    "verdict_why": "one sentence"
  },
  "sources": [{"title": "", "url": ""}]
}
```

## Scoring rubric (0–5, integers, same meaning for every tool)
- **ui_as_data**: 0 UI only exists as code · 3 declarative but in a host language (JSX, Python classes) · 5 the whole UI is a serializable value (JSON) you can store, diff, send.
- **agent_drivable**: 0 nothing · 1 agent can only generate source code · 3 a protocol/MCP/API an agent can call for some operations · 5 first-class agent control of a live instance (create, change, operate).
- **readback**: 0 agent can't see the result · 2 screenshots/screen scrape · 4 structured state/tree readable · 5 both structured state and rendered view on demand.
- **granular_edits**: 0 regenerate everything · 3 patches exist but coarse or one-way · 5 small id/pointer-addressed edits to a live instance, cheap and frequent.
- **time_travel**: 0 none · 2 undo/redo · 3 human-facing time travel (devtools) · 4 agent can read history/seek · 5 agent can read, seek, fork and replay.
- **live_data_rate**: 0 static · 2 updates in seconds · 3 tens per second · 4 hundreds/second with numbers · 5 thousands/second published and data bypasses the model.
- **terminal_native**: 0 web/desktop only · 3 has a terminal renderer · 5 built for the terminal.
- **pixel_graphics**: 0 none · 2 via add-on · 4 built-in images/graphics · 5 rich pixel rendering (anti-aliased drawing, compositing) natively.
- **teaching**: 0 none · 2 could be used for it · 4 has tours/walkthroughs/narration features · 5 built around teaching (record, narrate, take over, rewind).
- **maturity**: 0 idea · 1 prototype · 2 early (<1 yr, small use) · 3 used in production by some · 4 widely used · 5 ecosystem standard.
- **openness**: 0 proprietary/closed · 2 source-available/restrictive · 4 permissive OSS with company control/CLA · 5 permissive OSS, community governed.

## Rules
- Run every `firecrawl` command from `RESEARCH/<your-group>/` (create it). It writes a `.firecrawl/` cache into the working directory; that must never reach any project repo. Write nothing outside `RESEARCH/`.
- Prefer primary sources: repos (GitHub API `api.github.com/repos/<o>/<r>` for stars, license, pushed_at; `/releases?per_page=5`; `/contributors?per_page=1&anon=true` with the Link header for counts if feasible), npm (`api.npmjs.org/downloads/point/last-week/<pkg>`), crates.io (`crates.io/api/v1/crates/<name>`), PyPI stats, official docs, maintainers' posts.
- Never invent numbers, quotes, users or dates. If unknown: null and say so in notes.
- Steelman genuinely. The point is to find the specific gap thc-scene fills, not to argue it's best. If the tool already covers what scene does, say so.

## thc-scene in one paragraph (context for relation_to_scene)
A Rust terminal UI that is one serializable JSON value (components, data sources bound to shell commands / JSON-line streams / literals, keys, theme, callout layers). Elm architecture (serializable State, pure update, pure view). An agent drives a live instance over a local socket: push the whole UI, patch a node by id, send keys/mouse/clicks, read the screen as text, read full state, add callouts/spotlights, and hot-swap the binary keeping state (`upgrade`). Data streams bypass the model (~6,000 whole-UI pushes/s, 1,000 patches/s, 120 fps cap measured). Kitty-graphics pixel layer (cards beneath text, anti-aliased plots, SVG via resvg). Rebuilt thc's hand-written Today screen cell-for-cell (78,000 cells). Planned: agent-addressable time travel (history, state_at, screen_at, diff, seek, fork, replay, marks, export/import) and teaching tours (agent narrates, learner takes over = fork, rewind). Sits on caretline (Helix-model editor engine). Early prototype, single author, not yet released; license of thc is MIT.
