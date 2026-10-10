---
title: Are we reinventing something? Mapping 33 tools
description: Before building more of thc-scene, we had research agents make the best case for every nearby tool, scored them all on one rubric, and wrote down the gap that's actually left, and what to borrow instead of rebuild.
date: 2026-10-09T18:00:00Z
kicker: research
---

[The last post](/blog/a-ui-is-one-value/) described thc-scene: a terminal UI that is one JSON value,
which an agent can push, patch, drive and read back while live data streams through it. It works
and it's fast. The next question is the uncomfortable one: **does this already exist?** If it
does, we should use it, adopt it, or at least take its best ideas, not build a worse copy.

So we mapped the neighbourhood. The result is a public board,
**[the landscape](/landscape/)**, with a dossier for each of 33 tools. This post is how we made it
and what it told us.

![The landscape board's positioning map: tools placed by their agent loop and live-UI scores](/blog/landscape-map.jpg)

*The map on the board. Pick any two axes; scene's planned position is shown next to its current one.*

## How we built it

Five research agents each took a group of tools:

- **Terminal UI frameworks:** ratatui, Textual, Bubble Tea, Ink, OpenTUI, Notcurses, iocraft
- **Agent UI specs:** A2UI, MCP Apps, AG-UI, json-render, the Vercel AI SDK, Thesys C1
- **Agent canvases and live UIs:** raxol, tldraw's agent, Excalidraw's MCP, Wave, Phoenix
  LiveView, terminal drivers for agents, and others
- **Time travel and replay:** Redux DevTools, Elm's debugger, LangGraph, Rerun, Replay.io, rrweb
- **Teaching and live data:** Scrimba, marimo, Perspective, Streamlit, Motion Canvas, driver.js,
  terminal recorders

The brief for each was to **steelman** the tool: write its best case as its own maintainers would,
then gather the facts (license, stars, release cadence, contributors, architecture, published
performance numbers), with sources. Then score it from 0 to 5 on eleven axes, using one rubric for
every tool:

| axis | what a 5 means |
|---|---|
| UI as data | the whole UI is a serializable value you can store, diff and send |
| Agent can drive it | first-class agent control of a live instance |
| Agent can see it | structured state and the rendered view, on demand |
| Small, targeted edits | id-addressed edits to a live instance, cheap and frequent |
| Time travel | an agent can read, seek, fork and replay |
| Live data rate | thousands of updates a second, published, bypassing the model |
| Terminal native | built for the terminal |
| Pixel graphics | rich pixel rendering, natively |
| Teaching | built around teaching: record, narrate, take over, rewind |
| Maturity | an ecosystem standard |
| Openness | permissive and community-governed |

We scored scene the same way, honestly: **0 for time travel** (designed, not built) and **1 for
maturity** (a prototype by one author). Its dossier lists its weaknesses like everyone else's.

Each tool gets a verdict: **use** it, **adopt** it (build on it or speak its format), **borrow**
from it, **complement** it, or **ignore** it for this purpose. Of the 33: use 1 (ratatui, which
scene already sits on), adopt 1 (A2UI), borrow 20, complement 7, ignore 4.

## What we found

**Terminal frameworks have no agent surface.** ratatui, Textual, Bubble Tea and Ink are excellent
at what they do, which is helping a person write an app in code. None lets an agent drive a running
app from outside, and none treats the UI as a value. Textual comes closest: its test driver,
Pilot, has the right verbs (press, click, hover, wait for idle), and we'll take them.

**The agent UI specs have no terminal host.** A2UI is the closest thing agents have to HTML for
native UI, and it has momentum; MCP Apps is now the shared standard for UIs inside chat clients.
Neither has a maintained terminal renderer. That's open ground: scene could be the terminal host
for both, rather than a third format.

**One project took the same bet: raxol.** It's an Elixir framework with one Elm-style source of
truth, MCP tools derived automatically from the live component tree, a "focus lens" that shows the
model only the tools near what's focused, allow/ask/deny permissions and a time-travel API. In
places it's ahead of us. It's code-first rather than data-first, and so far small in use. We're
borrowing its focus lens, its permission model and the shape of its history entries.

**Time travel an agent can use barely exists for UIs.** Redux DevTools has the verbs, for people.
Replay.io gives agents a recording to read, after the fact. Rerun lets an agent move a time cursor
over logged data, and its layout undo is the layout's own timeline. LangGraph is the only tool
whose full history an agent can list, read, fork and replay, but it's agent state, not UI.

**"UI as data, edited by id" is a bet others are winning with.** marimo lets an agent edit
notebook cells by id in validated batches, and rejects a bad batch whole. Perspective ships an
in-page agent that drives only its public JSON config. Both are web tools; both say the model is
right.

![A dossier page: the steelman, the radar against scene, scores with notes, and what to borrow](/blog/landscape-dossier.jpg)

*A dossier: the steelman first, then the facts, the scores against scene, its weaknesses and what we'd take from it.*

## The gap, narrowly

Everything on our wish list is done better somewhere: formats (A2UI, json-render), terminal
rendering (ratatui), time travel for people (Redux DevTools), teaching (Scrimba), dense time
series (Rerun). What we couldn't find is the combination:

> A UI an agent writes as one value, rewrites live, piece by piece, and reads back exactly, fed
> by streams that never pass through the model, rendered natively in the terminal, with pixels
> where the terminal has them, and (once built) a history the agent itself can rewind, fork and
> replay.

That's narrower than "a new UI framework," and it should be. It means scene's job is mostly
glue and loop: speak the standards, run in the terminal, and make the agent's loop (write, see,
edit, rewind) fast.

## What we're taking

- **A2UI and MCP Apps as inputs,** not a competing format; JSON Patch for edits.
- **An MCP server,** with raxol's focus lens and Rerun's split: semantic tools first, raw keys
  and mouse as the fallback, recent logs on every result.
- **A command policy** before anyone pushes a UI they didn't write: data only by default,
  commands from an allowlist, raxol's allow/ask/deny.
- **Batches** that validate whole and fail whole, as in marimo.
- **History, after LangGraph and Redux DevTools:** record inputs, never pixels; snapshot
  sparsely; give agents `history`, `state_at`, `screen_at`, `diff`, `seek`, `fork` and `replay`.
  Every write names the step it was based on, so nothing drifts silently.
- **Tours as branches,** after Scrimba: a tour is a recording with marks and narration; when the
  learner touches anything, it forks, and the agent can diff what they did against the tour.

Two warnings from the research shape the order. Elm hoped its debugger would teach people, but
what they actually used was the exported bug report. Vue removed time travel when nobody used it.
So the first piece of history we'll build is **reproducible agent sessions**: record, export,
import, replay, diff. Tours come after, on the same log.

## Use it yourself

The [board](/landscape/) is meant to be used, by us and by anyone choosing a tool: sort the
matrix, filter by group, compare any tool with scene on the radar, and open a dossier to click out
to its site, repo and sources. Each dossier says what scene would only be reinventing.

The data is plain JSON, one file per tool, with the research brief and the import script in the
[site's source](https://github.com/brancusi/thought-control/tree/main/web/scripts/landscape). If
we've scored your tool unfairly, or missed one, that's where to tell us.
