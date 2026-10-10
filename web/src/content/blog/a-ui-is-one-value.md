---
title: A terminal UI that is one value
description: An experiment, thc-scene, makes a whole terminal screen one JSON value that an agent can push, patch, drive and read back while it runs, fed by live data at thousands of updates a second.
date: 2026-10-09T12:00:00Z
kicker: experiment
---

thc's screens are Rust. That's fine for the screens we ship, but two things we want don't fit it:
a vault with its own, simpler screen (a writing vault doesn't need a task board), and a screen an
agent puts up for you: "here's your week," "here's what changed in the build." Both need a screen
that is data, not code.

So we made a branch, stripped everything down, and tried the most direct version of the idea: the
whole UI is **one serializable value**. It's an experiment called thc-scene. This post is what we
built in a day of agents driving it, what held up, and what's missing.

![thc's Today screen rebuilt as a scene UI, with a callout an agent placed on a row, pixel cards and a live plot](/blog/scene-callout.jpg)

*thc's Today, rebuilt as one JSON value. An agent placed the callout; the plot on the left is pixels, the one beside it is the same data in cells.*

## The shape

It's the Elm architecture, which thc already uses: a serializable `State`, messages, a pure
`update` that returns effects and a pure `view`. A runtime owns the terminal, a local socket, child
processes and the pixel layer. The UI is a field of the state, so it can be swapped like any
other field.

A UI is a tree of components, the data sources they bind to, and keys:

```json
{
  "root": {"type": "row", "children": [
    {"type": "table", "id": "tasks", "title": "Open tasks", "bind": "tasks",
     "columns": [{"title": "task", "value": "{text}"}, {"title": "due", "value": "{due}", "size": 11}],
     "keys": {"x": {"run": ["thc", "done", "{id}"]}}},
    {"type": "list", "id": "pages", "title": "Pages", "bind": "pages", "item": "¶ {title}"}
  ]},
  "data": {
    "tasks": {"cmd": ["thc", "q", "status:open sort:due", "--json"], "path": "items", "every": 3},
    "pages": {"cmd": ["thc", "q", "is:page sort:title", "--json"], "path": "items", "every": 10}
  }
}
```

Data comes from commands, JSON-line streams or literal values. The model never sees the data:
an agent writes the screen once and the numbers flow straight from the source to the terminal.

## Push, patch, read back

Everything goes over a local socket, with a CLI for each operation:

```sh
thc-scene push trading.json          # the whole UI swaps, in one frame
thc-scene patch header node.json     # replace one node by id
thc-scene key j x                    # type, like a person
thc-scene mouse click 40 12          # or click
thc-scene screen                     # the frame, as text: see what you drew
thc-scene get --state                # everything, as JSON
thc-scene upgrade                    # exec a new binary; same screen, same state
```

View state (the selected row, the focused pane) is keyed by node id, like React's keys, so a push
keeps your place wherever the ids still match. `upgrade` saves the state, execs the new binary and
restores it: you rebuild the engine and the screen carries on, same process, same selection.

Reading back matters as much as writing. An agent that can't see what it drew is guessing;
`screen` gives it the frame as text, and `state` gives it everything behind the frame.

## How fast

We wrote a stress suite and left a frame counter on screen. Release build, a 170×52 terminal, one
machine:

| demo | fps | draw | messages/s |
|---|---|---|---|
| ticker, 500 updates a second | 114 | 0.23 ms | 500 |
| text plasma, 8,000 cells at 120 Hz | 110 | 0.24 ms | 122 |
| a 50,000-row list replaced 20 times a second | 20 | 0.18 ms | 20 |
| 1,000 patches a second | 116 | 0.22 ms | 1,000 |
| whole new UIs, as fast as a client can send | 108 | 0.84 ms | ~6,200 |

Three things got it there: one event queue for input, data and requests; streams coalesced so
the newest value wins each frame; and drawing only what's visible (the 50,000-row list borrows
rows and draws the 50 you can see).

## Does generated have to look generated?

The test we cared about most: can a scene UI match a screen we built by hand? We rebuilt thc's
Today screen as JSON (sections, rows, badges, the detail pane, ember-dark's colours) and compared
it with the real TUI **cell by cell**: character, colours and attributes, after the same keys.
All 13 states, 78,000 cells, are identical.

That took a small template language: theme-token styles (`<accent+b>…</>`), value maps
(`{status|todo=[ ];done=[x]}`), conditionals, right-aligned tails and lines that drop out when
their fields are empty. Data values can never inject styles, so a task titled `<red>` stays text.

## Pixels, where the terminal has them

Terminals that speak the kitty graphics protocol (Ghostty, kitty, WezTerm) can show pictures, so
scene uses them where they help and falls back to cells everywhere else:

- cards (rounded, shadowed, gradient panels) drawn **under** the text, so selections still paint
  over them
- anti-aliased plots with gradient fills
- SVG, rendered with resvg and filled from data (the progress ring below)
- PNGs by path, fitted to their aspect
- spotlights: a translucent sheet that dims everything but one pane

![The same screen in a light theme, rearranged by a single push: KPI cards on top, a progress ring drawn from SVG](/blog/scene-light.jpg)

*One push rearranged the screen and switched the theme. The ring is SVG with its arc filled from data.*

The view stays pure: it asks for a picture in cells, colours and proportions, and the runtime
rasterises it at the terminal's real cell size, sends it once (cached by content) and moves only
what moved. Larger text (OSC 66) is probed at startup; Ghostty doesn't draw it yet, so scene falls
back to block digits.

What we tell agents, in order: **use components first** (they're themed, cached and degrade
gracefully), **SVG for anything custom**, **PNG only for real bitmaps**, and **animate the data, not
the pictures**.

## What's missing

This is a prototype by one author, and it shows:

- **No history yet.** State is serializable and every input is a message, so time travel (read,
  seek, fork, replay) is cheap to build. It isn't built.
- **No MCP server.** Agents drive it over the socket or the CLI.
- **Sources run shell commands.** That's convenient and the opposite of the field's "data, not
  code" safety stance. It needs a command policy before anyone pushes a UI they didn't write.
- **Its own format.** No A2UI or MCP Apps input yet.
- **Pixels cost.** A 15 Hz plot takes frames from under 1 ms to about 7 ms, because rasterising
  runs on the main thread. A worker thread is next.
- **Fullscreen only,** so it gives up the terminal's own scrollback and search.

Before building more, we wanted to know whether we were reinventing something. That's
[the next post](/blog/mapping-the-landscape/).

The code is on the [`poc/declarative-ui` branch](https://github.com/brancusi/thought-control/tree/poc/declarative-ui/crates/scene),
with every demo in `crates/scene/examples/`.
