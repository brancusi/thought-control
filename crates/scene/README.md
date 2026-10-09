# thc-scene (proof of concept)

A terminal UI that is one serializable value. The whole screen (a tree of components, the data
sources they bind to and the keys) is plain JSON. A running screen holds no other state, so you can
read the UI out, push a whole new one in, or replace one node, and it re-renders.

```
world (thc, a market feed, any command that prints JSON)
   │  sources: {"cmd": [...], "path": "items", "every": 3}
   ▼
State { ui, data, focus, local (selection by node id), status }   ← serializable, `get --state`
   │  update(state, msg) -> effects        (pure)
   │  view(state) -> frame                 (pure)
   ▼
terminal
```

## Try it

```sh
cargo build -p thc-scene
thc-scene run crates/scene/examples/tasks.json          # in one terminal
thc-scene push crates/scene/examples/trading.json       # in another: the whole UI swaps
thc-scene patch header node.json                        # replace one node by id
thc-scene key j x                                       # type keys
thc-scene get --state                                   # everything on screen, as JSON
thc-scene run --watch my.json                           # hot reload while you edit
thc-scene render crates/scene/examples/tasks.json --size 100x30 --keys j   # no terminal
```

## The model

- **Components** (a closed set): `col`, `row`, `text`, `list`, `table`, `sparkline`. Every node can
  have `id`, `size` (`12`, `"30%"`, `"*"`, `"2*"`), `title`, `border`, `style`, `bind` and `keys`.
- **Sources** in `data`: `cmd` (argv), `shell`, or a literal `value`; `path` narrows the JSON, `every`
  re-fetches. Text and columns are templates over the bound value: `"{text} · {due}"`.
- **Actions** on keys: `quit`, `up`, `down`, `top`, `bottom`, `focus_next`, `focus_prev`, `refresh`,
  `{"run": [argv with {field} of the selected row]}`, `{"load": "other-ui.json"}`,
  `{"focus": "id"}`. A focused node's keys win over the UI's, which win over the defaults.
- **Identity**: view state (selection) and focus are keyed by node id, so they survive a push for
  any id that's still there (React's `key`).

## Not yet

An `editor` component on caretline, a `thc` subcommand instead of a separate binary, policy on what
a pushed UI may `run` (today: anything you could run yourself, over a socket only you can open),
the existing TUI's screens rebuilt as UIs.

## Stress demos

`crates/scene/examples/stress/`: a 500 updates/s ticker, a 160×50 text plasma at 120 Hz and a
50,000-row list replaced 20×/s (all `stream` sources fed by `feed.py`), plus `storm.py`, which
drives the screen over its socket with 1,000 patches/s or whole random UIs as fast as it can.
`play.sh` runs them all on the running screen and prints the counters. The status bar always
shows fps, the mean and worst draw time and messages/s; UIs can bind them as `$stats`.

Release build, a 170×52 terminal, measured on one machine:

| demo | fps | draw | msgs/s |
|---|---|---|---|
| idle | 2 | 0.4 ms | 0 |
| ticker (500 updates/s) | 114 | 0.23 ms | 500 |
| plasma (120 frames/s of 8,000 cells) | 110 | 0.24 ms | 122 |
| 50k-row list replaced 20×/s | 20 | 0.18 ms | 20 |
| patch storm, 1,000 patches/s | 116 | 0.22 ms | 1,000 |
| push storm, whole UIs as fast as a client can send | 108 | 0.84 ms | ~6,200 |
