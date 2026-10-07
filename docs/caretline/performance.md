# Performance

How fast caretline is, where the limits are, and how to go faster. The numbers come from release
builds on an Apple-silicon Mac. `caretline bench` reproduces the engine and protocol numbers; the
stress and animation figures came from driving a live `caretline FILE --listen` editor over its
socket from a script.

## Summary

The engine is not the bottleneck for anything a person or a terminal does. An edit costs 1–2 µs,
a caret move 10–30 µs, a socket round trip about 13 µs and a typical frame about 0.13 ms.
Document size barely matters: a million-line document edits and scrolls as fast as a
hundred-line one. The practical ceilings are outside caretline: how fast the terminal paints and
how fast a client can produce input.

## Engine and protocol (`caretline bench`)

| Operation | 1,000-line doc | 100,000-line doc |
|---|---|---|
| `msgs`, one edit, in process | 1.6 µs | 1.7 µs |
| `msgs`, one edit, socket round trip | 13.4 µs | 14.2 µs |
| `hello`, socket round trip | 11.4 µs | 11.5 µs |
| `keys "<down>"`, socket round trip | 42.9 µs | 34.8 µs |
| `render` 100×40, in process | 127 µs | 128 µs |
| `render` 100×40, socket round trip | 144 µs | 145 µs |
| `state.get`, in process | 22 µs | 2.55 ms |
| `state.get`, socket round trip | 2.86 ms (2.2 MB) | 10.9 ms (9.0 MB) |

| Per message, in process | p50 |
|---|---|
| `tick` | 0.8 µs |
| move by grapheme | 9.5 µs |
| move by visual line | 28.4 µs |
| a realistic mix | ≈ 32,000 messages/s |

**Typing runs** (one `insert_text` per character, no clock) cost about 10 µs per character at
any length: 16,000 characters take 0.15 s and 64,000 take 0.6 s. Before the wrap cache and undo-run
caps this was quadratic (8.1 s for 16,000 characters).

## Stress test (live editor, driven over the socket)

| Stage | Result |
|---|---|
| Typing firehose, one message per character, live redraw | 72,000 characters/s |
| Load 100,000 lines (8 MB) with `state.set` | 19 ms |
| Load 1,000,000 lines (82 MB) with `state.set` | 202 ms |
| Page down through 100,000 / 1,000,000 lines | ≈ 6,800 pages/s for both |
| Jump to the end and edit, 1,000,000 lines | 1.1 ms |
| One 1 MB line with no line breaks (≈ 8,500 wrapped rows): load / page down / insert mid-line | 2 ms / 11,200 pages/s / 0.02 ms |
| 100 / 1,000 / 10,000 carets typing at once | 0.3 / 0.4 / 1.4 ms per keystroke |
| Undo 3,000 separate edits | 32 ms (11 µs each) |
| Render 80×24 / 200×60 / 400×150 | 0.13 / 0.58 / 1.8 ms per frame |
| An outline of 5,000 blocks, typing with a render per key | ≈ 1 ms per key |

## Animation throughput

Full-screen frames (122×64 cells, 8–14 KB of JSON each, with multiple selections used as a second
colour) were precomputed and pushed into a live editor as `state.set` requests.

| Scene | Unthrottled states applied per second |
|---|---|
| Shaded 3D torus | 923 |
| Rotating wireframe cube | 1,346 |
| Full-screen texture tunnel | 835 |
| Cellular fire | 1,194 |
| Star field | 1,674 |
| All scenes interleaved | 1,155 |

Busier frames (more selection ranges) are slower. Every state is applied in order and is in the
trace; when states arrive faster than the terminal paints, the screen shows the newest.

## Where the limits really are

- **The terminal's frame rate.** WezTerm paints at most `max_fps` frames per second, 60 by
  default; raise it with `config.max_fps = 120`. The display limits it too: a 60 Hz monitor can't
  look smoother than 60 fps, a 120 Hz panel can show 120.
- **Multiplexers.** A multiplexer between the editor and the terminal re-reads and repaints the
  pane at its own pace, adding a cap and a copy. Measure in a plain terminal window.
- **Clients.** A paced client that sleeps `1/fps − work` drifts and lands below its target. Schedule
  against absolute deadlines (`start + k/fps`), sleeping until about 1 ms before each and spinning
  the rest.
- **Frame shape.** A full `state.set` per frame parses the whole text and rebuilds the history.
  Fine at 1,000 frames/s, but the wrong shape for animation; see below.

## Known gaps

| Gap | Impact | Plan |
|---|---|---|
| `state.get` includes the whole undo history | 1.5 MB after 3,000 small edits; 9 MB on a 100,000-line document | `state.get {history: false}` and a separate `history.get` |
| `keys` costs about 3× an equivalent `msgs` | 35–43 µs against 13 µs per round trip | cache parsed key scripts and keymap tables |
| Outline blocks are re-derived on every edit | ≈ 1 ms per key at 5,000 blocks | make derivation incremental if pages approach 50,000 blocks |
| No synchronized output | frames can tear in the terminal | wrap each repaint in synchronized-update mode (DEC 2026) |
| Repaints follow input, not the display | wasted work past the display rate | coalesce to one paint per refresh; redraw changed cells only |

## Toward 120 fps and beyond

Smooth animation should not stream frames from outside. In the Elm model, animation is state that
advances with time:

1. The runtime sends a frame message (for example `Msg::Frame { now_ms }`) at the display rate.
   Time stays an input, so animation stays pure and replayable.
2. `update` advances animation state from `now_ms`; `view` draws the frame for that time, so a
   dropped frame never causes stutter.
3. The runtime repaints once per refresh inside a synchronized update, writing only changed cells.

With that, a 120 Hz display at `max_fps = 120` shows 120 fps. Rates above the display's refresh
are only useful headless (replay, tests, recording), which already runs at more than 1,000 states
per second.

## Sub-cell graphics

A terminal cell can show several "pixels" using Unicode block characters, with 24-bit foreground
and background colours per cell:

| Technique | Dots per cell | Notes |
|---|---|---|
| Half blocks `▀ ▄` | 1×2, two colours | The most common: two full-colour pixels per cell |
| Quadrants `▖ ▗ ▘ ▝ ▙ …` | 2×2 | |
| Sextants (Unicode 13) | 2×3 | |
| Octants (Unicode 16) | 2×4 | Needs font support |
| Braille `⠁ ⡇ ⣿` | 2×4 dots, one colour | Good for lines and plots |
| Eighth blocks `▏▎▍▌ ▁▂▃▄` | ⅛-cell edges | Smooth bars, scrollbars and sliding edges |
| Kitty graphics, iTerm2 images, Sixel | real pixels | Bitmaps placed in the grid; WezTerm supports them |

notcurses, ratatui's `Canvas`, chafa and timg use these. caretline's frame has three roles today
(text, selection, status); pixel graphics would be a separate canvas surface with true colour and
a choice of these "blitters", driven by the same frame clock.

## Reproducing

```console
$ caretline bench                       # engine and protocol (release build)
$ caretline notes.md --listen           # a live editor to drive from another process
```

Drive the live editor with `caretline send` or any JSON-lines client; see
[protocol.md](protocol.md).
