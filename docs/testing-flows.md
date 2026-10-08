# User-flow tests

`crates/thc-tui/src/flows/` drives the TUI the way a person does: a key, a click, a paste at a
time, with every frame checked. A flow reads as a user story:

```rust
#[test]
fn jump_to_a_page_with_ctrl_o_and_type() {
    flow("jump to a page with ⌃O and type")
        .keys("<c-o>Q4 Plan<cr>")
        .expect_page("Q4 Plan")
        .keys("<c-end>")
        .type_text(" and more")
        .expect_caret_after("and more")
        .expect_saved_contains("and more")
        .done();
}
```

Run them with `cargo test -p thc-tui flows::` (about 20 s in a debug build). A failing flow
prints its name, the step, what broke and the frames before and after, with row numbers.

Scratch vaults and their replay copies go under the system temp dir. If it has grown huge
(tens of thousands of entries make every `mkdir` there slow, and the suite takes minutes),
point the run elsewhere: `TMPDIR=/private/tmp/thc-flows cargo test -p thc-tui flows::`.

## How a flow runs

`flow(name)` makes a scratch vault (`fixture.rs`: the pages Q4 Plan, Garden and Lisbon flat, a
journal day and yesterday, inbox tasks), opens a headless `Session` on it at 140×36 and pins
the logical clock to today 10:00. `flow_with(name, Size::Long | Size::Huge, (w, h))` adds a
300- or 5,000-line page.

Each step:

1. moves the logical clock on (`pace`, 60 ms by default; `pace(0)` types in a burst; a `Tick`
   goes out once a second's worth has built up);
2. applies its messages through `Session::apply`, the one door every input takes;
3. draws a frame into one long-lived emulated terminal (`emu.rs`), as the live loop draws
   frame after frame, then runs the runtime's after-frame work (the save of a line just left)
   and draws again if it did something;
4. checks the invariants below.

`done()` ends a flow: the trace replays, then the state is restored on a fresh session. A flow
dropped without `done()` fails.

## Steps

| Step | What it sends |
|---|---|
| `keys("…")` | a key script (`script.rs`): `j`, `<cr>`, `<c-o>`, `<d-[>` (⌘[), `<m-cr>` (⌥Enter)… each token its own step; anything may change |
| `moves("…")` | caret keys: the chrome may not change and the view scrolls only to keep the caret on screen |
| `keys_as(Motion::Typing, "…")` | keys held to the typing rules |
| `type_text("…")` | one key per character (`\n` is Enter), held to the typing rules |
| `paste("…")` | a bracketed paste; `paste_clipboard()` pastes what ⌃C / ⌃X copied, as ⌘V in a terminal would |
| `click(at)`, `click_caret(at)` | a click; `click_caret` also holds the caret rules |
| `shift_click`, `cmd_click`, `ctrl_click`, `alt_click` | a click with ⇧, ⌘ (`<d-click:x,y>`), ⌃, ⌥ held |
| `double_click`, `triple_click`, `drag(from, to)`, `wheel(down, n, at)`, `hover` | the rest of the mouse |
| `resize(w, h)`, `idle()`, `save()` | the terminal's size; time passing (the idle point: an undo step, the idle save); focus lost (saves) |
| `agent_add("…")`, `ui_patch(json, actor)`, `remote_edit(line, text)`, `msg(desc, motion, Msg)` | another writer: `thc add`, `thc ui patch`, another device; any `Msg` |
| `timed("page_open", \|f\| …)` | labels the steps inside for the perf tests |

### Anchors

Clicks name what's on screen, never a cell:

- `text("Garden")`: drawn text, the first match top to bottom; `.nth(1)`, `.dx(3)` (cells into
  it), `.in_doc()`, `.in_main()`, `.in_side()`, `.in_footer()` narrow it.
- `doc_at("Grow the newsletter", 5)`: byte 5 of the main document's line holding that text,
  found by hit-testing the frame (so wrapped rows and wide characters land where they're drawn).
- `tab(View::Tasks)`: a click target the frame declared.
- `At::Cell(x, y)`: only when nothing else will do.

### Expectations

`expect_page`, `expect_day(±n)`, `expect_view`, `expect_no_doc`, `expect_caret_after`,
`expect_caret_before`, `expect_caret_line`, `expect_line`, `expect_no_line`, `expect_depth`,
`expect_selection`, `expect_focus`, `expect_panels`, `expect_screen`, `expect_no_screen`,
`expect_at(text(…).in_side())`, `expect_saved` / `expect_saved_contains` /
`expect_saved_status` (after a save, in the store), and `expect(what, |session| bool)` for
anything else. `dump()` prints the frame.

## Invariants, after every step

- **No blank frame.**
- **No stale or torn cells:** the emulated terminal, fed only ratatui's diffs with a real
  terminal's wide-character rules (writing over either half of a wide character erases it),
  shows exactly what the frame says. No wide character is cut at the right edge.
- **No stale caches:** the frame drawn with the sidebar and page-preview caches dropped is the
  same.
- **The state round-trips through JSON** (`UiState` to text and back is equal).
- **The caret:** while writing, the cursor shows inside the document's view, and hit-testing the
  cursor's cell gives the document's caret (line and byte).
- **Another writer** (`Motion::Agent`: `agent_add`, `remote_edit`): the cursor stays where it
  was on screen (what you're typing doesn't move); the header's badges and the footer may change.
- **A click** (`Motion::Click`: `click_caret`, `alt_click`): as a caret move, and the view
  doesn't scroll at all; the text stays under the mouse.
- **No shifting** (`Motion::Typing` and `Motion::Caret` steps): the document's view keeps its
  place; every row outside it (header, tabs, footer) is unchanged, except the footer's hints,
  its counts and a toast; the view scrolls only when the caret is on its first or last row.
  While typing, every row above the caret's note is unchanged (the note's own rows may reflow:
  a word can move up a row).

At `done()`:

- **Replay:** the flow's trace (`Session::trace`) replays (`session::replay`, on scratch copies
  pinned to each segment's frontier) to the same frame at every step. Only ids a replayed write
  minted fresh (a node's short id, a tx id) and the minute a write stamped from the wall clock
  (`done 01:15`) may differ.
- **Restore:** after a save, the state restored on a fresh session on a copy of the vault
  draws the same frame (trailing spaces aside). One rule, by design: the caret's own empty
  note is never saved, so when the caret sits on an empty note that isn't the document's last
  line, the live frame is compared without that row (`Flow::unsaved_caret_row`). Last, it is
  where a fresh session arrives anyway, on a fresh line.

## Known bugs, ignored flows

A flow that fails on a product bug is filed on the board (¶ Issues) and marked
`#[ignore = "<task short id>"]`, so the suite stays green and the ignored flows are the to-do
list: `cargo test -p thc-tui flows:: -- --ignored --skip perf` runs them. When a bug only
spoils one check of an otherwise useful flow, the flow steps around it narrowly instead:
`.known("<task>", Known::Rail)` leaves the left rail out of the restore check, and
`.known("<task>", Known::Restore)` skips the restore check. Remove the `.known(…)` or the
`#[ignore]` with the fix.

## The monkey

`flows/monkey.rs` walks a page at random from a fixed seed (typing with wide characters, Enter,
Tab, Backspace, caret keys, selections, undo, clicks on the page's rows, the wheel, idling,
resizes), every invariant checked after every step. A failure names the seed and the step;
the same seed walks the same way. `THC_FLOW_SEEDS=50 cargo test -p thc-tui flows::monkey` walks
more seeds for a longer hunt.

## Budgets

`flows/perf.rs` times each step (its handling and its frame, checks off) in a release build:

| Step | Budget |
|---|---|
| typing a character | 4 ms |
| opening a page | 100 ms |
| opening the sidebar | 30 ms |
| the wheel, PgDn, ↓ | 4 ms |

on a 300-line and a 5,000-line page, and typing beside an open panel. On an M-series Mac,
quiet, 2026-10-08 (p50 / p99, ms):

| Step | 300 lines | 5,000 lines |
|---|---|---|
| typing (near the top) | 0.56 / 0.63 | 1.44 / 1.61 |
| typing at the page's end | | 2.01 / 2.22 |
| typing beside a panel | 0.56 / 0.66 | 1.45 / 1.61 |
| opening the page | 2.3 / 2.6 | 37.6 / 78.7 |
| opening the sidebar beside it | 0.8 / 1.1 | 1.9 / 4.9 |
| the page itself opened beside | 1.6 / 1.9 | **28.0 / 33.2** (vw384) |
| the wheel | 0.43 / 0.50 | 0.53 / 0.65 |
| PgDn, ↓ | 0.62 / 0.71 | 0.63 / 0.70 |

`flows::perf::diag_split` splits a keystroke's time (the frame's preparation, the key's
handling, caretline's insert, the draw). They're ignored by
default (a debug build is far slower): `cargo test --release -p thc-tui flows::perf --
--ignored --nocapture --test-threads=1` prints p50, p99 and the slowest steps, and fails on a
p99 over budget. A test whose ignore reason starts with a task id is over budget now. The
machine matters: on a shared machine expect ±40%.

## Determinism

The logical clock is pinned per flow (`Tick` messages; the header clock reads it). Set
`THC_NOW=2026-10-07T10:00` to pin "today" as well; it's process-wide, so the flows don't set it
themselves. Node ids are random per run (anchors find text, never ids). The once-per-device
drag hint is marked seen in each flow's cache.

## Writing a flow

1. Start from `flow`, `flow_with` or a helper (`typing::q4()` opens Q4 Plan).
2. Say what the person does with steps and anchors; say what they should see with expectations.
3. Pick the motion: `type_text` and `moves` hold the no-shift rules; `keys` allows anything.
4. End with `.done()`.
5. If it fails on a product bug, file it (`thc todo --vault thc-dev … -p high --under ¶ Issues`,
   the flow's name and the printed frames as the repro) and `#[ignore = "<short id>"]` it.
