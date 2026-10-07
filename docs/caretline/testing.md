# Testing

Because `update` and `view` are pure, a caretline test is just data: a starting state, some
keys or messages, and the expected result. No terminal, no timing, no mocks.

## Where the tests live

| File | What it checks |
|---|---|
| [`caretline/tests/goldens.rs`](../../crates/caretline/tests/goldens.rs) | Behaviour goldens: text and selection before, keys, text and selection after (and frames where it matters). The baseline is a macOS text field |
| [`caretline/tests/rehydrate.rs`](../../crates/caretline/tests/rehydrate.rs) | A state survives JSON exactly; a recorded session replays to the live result |
| [`caretline/tests/fuzz.rs`](../../crates/caretline/tests/fuzz.rs) | Property tests: seeded random documents and messages, with invariants checked after every step |
| [`caretline/tests/marks.rs`](../../crates/caretline/tests/marks.rs) | Block marks: each mapping rule, undo and redo restoring marks exactly, cut and paste keeping ids, serialization; random sessions with marks at line starts and unique ids after every step, every undo restoring the marks |
| [`caretline/tests/outline.rs`](../../crates/caretline/tests/outline.rs) | Outline goldens in block notation (` ‖ ` blocks with a blank row between, ` ¦ ` without, `⏎` a soft break), with block ids: Enter, Backspace and Delete at block edges, Tab, the task cycle, moves, atomic images, copy and paste, blank rows, host messages and effects |
| [`caretline/tests/outline_fuzz.rs`](../../crates/caretline/tests/outline_fuzz.rs) | Outline properties over random outlines and messages: blocks and marks agree, ids unique, no caret in a marker or an image, undo and redo exact (marks included), kind changes move no other block, cut and paste in place keeps ids, Markdown files round-trip. `CARETLINE_OUTLINE_SEEDS` runs more seeds |
| [`caretline/tests/keymap.rs`](../../crates/caretline/tests/keymap.rs) | Key bindings and the key-script parser |
| [`caretline/tests/common/mod.rs`](../../crates/caretline/tests/common/mod.rs) | Shared helpers: caret notation, `golden`, `keys`, `send`, `frame`, random generators |
| `caretline/src/helix/**` | Helix's own unit tests, vendored with the code |
| [`caretline-app/tests/cli.rs`](../../crates/caretline-app/tests/cli.rs) | The binary: fixtures render to their saved snapshots, traces replay, `--keys` and `--dump-state` round-trip, effects are reported and never performed |

Run them:

```sh
cargo test -p caretline
cargo test -p caretline-cli
```

## Caret notation

Goldens write the text and the selection as one string:

| Notation | Means |
|---|---|
| `▮` | The caret (the selection's head) |
| `⟦abc▮⟧` | `abc` selected left to right: the anchor before `a`, the caret after `c` |
| `⟦▮abc⟧` | `abc` selected right to left: the caret before `a` |

`state("Hello ⟦wor▮⟧ld")` builds a state at 80x24 from notation; `state_wh(…, w, h)` picks the
size. `show(&state)` prints the primary selection back in notation.

## Goldens

A golden is a before, a key script and an after:

```rust
#[test]
fn e01_left_collapses_to_start() {
    golden("Hello ⟦wor▮⟧ld", "<left>", "Hello ▮world");
}
```

When a case needs more than text and selection, drive the state and assert on what you need:
effects, the clipboard, the history, the frame or the caret's cell.

```rust
#[test]
fn e37_cut_is_one_undo_step() {
    let mut s = state("Hello ⟦wor▮⟧ld");
    let fx = keys(&mut s, "<c-x>");
    assert_eq!(fx, vec![Effect::ClipboardSet { text: "wor".into() }]);
    assert_eq!(show(&s), "Hello ▮ld");
    keys(&mut s, "<c-z>");
    assert_eq!(show(&s), "Hello ⟦wor▮⟧ld");
}
```

| Helper | Does |
|---|---|
| `golden(before, script, after)` | Builds, runs the keys, compares the notation |
| `keys(&mut state, script)` | Runs a key script through the keymap; returns the effects |
| `send(&mut state, msgs)` | Sends messages; returns the effects |
| `frame(&state)` | The rendered frame as text |
| `cursor(&state)` | The caret's screen cell |

Use `<wait:MS>` in a script when undo grouping matters: `"one<wait:2000> two<c-z>"` undoes
only `" two"`.

## Rehydration and replay tests

`rehydrate.rs` checks two promises.

- **A state survives JSON.** `round_trip` asserts that `from_json(to_json(s))` equals `s`,
  renders the same frame and serializes to the same JSON. It covers fresh and edited states,
  the undo history, the goal column, an open typing run, and hand-broken states that load
  repaired.
- **A trace replays exactly.** A `Recorder` stands in for the interactive runtime: it writes
  the initial state and every message (including the `saved` its effects produce) to a trace.
  `replay_trace` must then give the identical state and frame, for scripted sessions and for
  random ones.

## The property fuzzer

`fuzz.rs` runs 24 seeds of 500 random messages each (`SEEDS`, `STEPS`). Documents mix ASCII,
tabs, CRLF, emoji with skin tones, ZWJ families, flags, combining accents, CJK, zero-width and
control characters. Viewports range from 1x1 to 200 columns. Now and then a step starts from a
random multi-range selection.

After **every** step it checks:

| Invariant | |
|---|---|
| Positions | Every anchor and head is within the text and on a grapheme boundary |
| EI1 | A motion without Shift leaves no selection |
| EI2 | A motion with Shift never moves the anchor |
| EI4 | Copy changes nothing but the clipboard |
| EI5 | An edit that made a new revision undoes to exactly the state before it and redoes to exactly the state after |
| EI6, EI7 | Typing over a selection replaces exactly it; a delete with a selection removes exactly it |
| Effects | Effects are plain values that match the message |
| `view` | Never panics, at the state's size and at extreme sizes |
| Serialization | At random points, the state round-trips to the same state and frame |
| Undo all | Undoing everything gives back the original text |

Separate tests check EI3 and EI8 (cut-then-paste and copy-then-paste in place are
identities) and EI12 (select all, delete, one undo restores everything).

A failure names its seed and step (`seed 7 step 312: …`). The generator is seeded, so the same
seed fails the same way every time. To focus on it, temporarily run only that seed in
`random_sessions_keep_every_invariant`.

## Snapshot fixtures

[`crates/caretline-app/fixtures`](../../crates/caretline-app/fixtures) holds
`NAME.state.json` files with `NAME.snapshot.txt` and `NAME.snapshot.ansi` beside them.
`fixtures_render_their_snapshots` finds every `*.state.json`, renders it at its own
viewport in both formats, and compares. Adding a fixture needs no code:

```sh
cd crates/caretline-app/fixtures
printf 'First line\nA second line that is long enough to wrap.\n' > my-case.md
caretline --new-state my-case.md --size 30x6 > my-case.state.json && rm my-case.md
caretline --state my-case.state.json --keys '<down><s-end>' --dump-state my-case.state.json
caretline --state my-case.state.json --snapshot 30x6 > my-case.snapshot.txt
caretline --state my-case.state.json --snapshot 30x6 --format ansi > my-case.snapshot.ansi
```

The snapshot size must be the state's viewport, because the test renders each fixture at its
own viewport.

`--new-state` records the path you give it, so run it from the fixtures directory (or edit
`path` afterwards) to keep machine paths out of the fixture. Review the snapshot by eye
before committing it: from then on, the test holds the engine to it.

## From a recorded trace to a test

When something goes wrong in a real session, record it and turn it into a test.

**1. Record it.**

```sh
caretline notes.md --trace bug.jsonl
```

**2. Check that it replays.** The replay is exact, so the bug shows up in the final frame or
state.

```sh
caretline --replay bug.jsonl --snapshot 80x24
caretline --replay bug.jsonl --dump-state -
```

**3. Split it** into a starting state and a message file. Then you can trim messages until
only the ones that matter remain:

```sh
head -1 bug.jsonl | jq .state > bug.state.json
tail -n +2 bug.jsonl | jq -c .msg > bug.msgs.jsonl
caretline --state bug.state.json --msgs bug.msgs.jsonl --snapshot 80x24   # same frame as the replay
```

`--msgs` and `--replay` agree as long as the trace has a single `state` line. A trace that
was appended to by several sessions has one `state` line per session; split at the last one.

**4. Write the test.** Pick one:

- **A golden**, when the bug is about text and selection. Turn the state's text and selection
  into notation, and the messages into a key script (or use `send` with the messages).
- **A fixture**, when the bug is about the frame. Save the trimmed state with
  `--dump-state` as a fixture, as above.
- **A trace test**, when the exact sequence matters (timing, effects, resizes). Commit the
  trimmed trace and assert on its replay:

```rust
use caretline::trace::replay_trace;
use caretline::view;

#[test]
fn recorded_session_replays_to_the_saved_frame() {
    let trace = include_str!("../../caretline-app/fixtures/session.trace.jsonl");
    let (state, _) = replay_trace(trace).unwrap();
    // The trace ends with a resize to 36x8, the snapshot's size.
    assert_eq!(
        view(&state).to_text(),
        include_str!("../../caretline-app/fixtures/session.snapshot.txt")
    );
}
```

Run it before the fix to see it fail, then fix the engine and keep the test.
