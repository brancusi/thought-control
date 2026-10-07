//! `caretline demo`: built-in demos that need no files. Each writes its documents to a
//! fresh temporary directory and runs the real editor on them.
//!
//! - `tour` (the default): a guided outline that teaches by doing, with the status bar
//!   naming the next step, ⌃D to dump the whole editor as JSON and ⌃P to replay the session.
//! - `scenes`: ASCII animations pushed into the editor with the protocol's `frame` op.
//! - `agent`: a scripted agent co-editing over the editor's socket, in its own view.

mod agent;
mod replay;
mod scenes;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use caretline::helix::Selection;
use caretline::{Frame, Key, KeyCode, Msg, OutlineLayout, Session, State, Viewport};
use clap::Parser;

use caretline::trace::TraceLine;

use crate::hub::Hub;
use crate::runtime::{self, dispatch_demo, Demo, KeyAction};
use replay::Replay;

const TOUR: &str = include_str!("tour.md");
const AGENT_DOC: &str = include_str!("agent.md");

#[derive(Parser, Debug)]
#[command(
    name = "caretline demo",
    about = "Built-in demos: no files needed (they write theirs to a temporary directory)",
    after_help = "Demos:\n  tour     a guided tour of the editor, learned by doing (the default)\n  scenes   ASCII animations (warp, donut, cube, tunnel, plasma, fire) running in the editor\n  agent    co-editing: a scripted agent types beside you over the editor's socket\n\nExamples:\n  caretline demo\n  caretline demo scenes\n  caretline demo agent\n  caretline demo --snapshot 80x24          the tour's first frame, headless\n  caretline demo scenes --bench            measure frame rates against a live editor"
)]
pub struct DemoArgs {
    /// Which demo.
    #[arg(default_value = "tour", value_parser = ["tour", "scenes", "agent"])]
    demo: String,

    /// Print the demo's first frame at WIDTHxHEIGHT and exit (headless).
    #[arg(long, value_name = "WxH")]
    snapshot: Option<String>,

    /// Snapshot format.
    #[arg(long, default_value = "text", value_parser = ["text", "ansi"])]
    format: String,

    /// With --snapshot: apply this key script first, as the person would type it (the
    /// demo's own keys included), e.g. 'hi<down><c-t>'.
    #[arg(long, value_name = "SCRIPT", requires = "snapshot")]
    keys: Option<String>,

    /// Write the demo's files here instead of a new temporary directory.
    #[arg(long, value_name = "DIR")]
    dir: Option<PathBuf>,

    /// Don't capture the mouse.
    #[arg(long)]
    no_mouse: bool,

    /// agent: run the scripted agent against a headless editor over a real socket, with a
    /// simulated person typing between its reads and writes, and print a JSON report.
    #[arg(long)]
    headless: bool,

    /// scenes: some of warp,donut,cube,tunnel,plasma,fire (default all, warp first).
    #[arg(long, value_name = "NAMES")]
    scene: Option<String>,

    /// scenes: frames per second (with --bench, a list of target rates; 0 is unthrottled).
    #[arg(long, value_name = "FPS")]
    fps: Option<String>,

    /// scenes: seconds per scene before the next (with --bench, per scene and rate).
    #[arg(long, value_name = "S")]
    seconds: Option<f64>,

    /// scenes: play into an editor that is already running (the newest one, or --socket)
    /// and report the frame rates achieved.
    #[arg(long)]
    bench: bool,

    /// scenes --bench: the editor's socket.
    #[arg(long, value_name = "PATH")]
    socket: Option<PathBuf>,

    /// scenes --bench: frames precomputed per scene.
    #[arg(long, value_name = "N", default_value_t = 180)]
    frames: usize,

    /// scenes --bench: the scene size (default: the editor's text area).
    #[arg(long, value_name = "WxH")]
    size: Option<String>,
}

pub fn main(argv: &[String]) -> Result<(), String> {
    let args = DemoArgs::parse_from(std::iter::once("caretline demo".to_string()).chain(argv.iter().cloned()));
    match args.demo.as_str() {
        "scenes" => scenes::main(&args),
        "agent" if args.headless => agent::headless(),
        "agent" => editor_demo(&args, Kind::Agent),
        _ => editor_demo(&args, Kind::Tour),
    }
}

/// The directory the demo writes to: `--dir`, or a new one under the system's temp dir.
fn demo_dir(args: &DemoArgs) -> Result<PathBuf, String> {
    let dir = match &args.dir {
        Some(d) => d.clone(),
        None => std::env::temp_dir().join(format!("caretline-demo-{}", std::process::id())),
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Tour,
    Agent,
}

/// The demo document's state: an outline with the layout's hang glyphs, the caret where
/// the person starts.
pub(crate) fn initial_state(kind: Kind, path: Option<String>, viewport: Viewport) -> State {
    let text = match kind {
        Kind::Tour => TOUR,
        Kind::Agent => AGENT_DOC,
    };
    let mut state = crate::new_state(text, path, viewport, true);
    state.view.layout = Some(OutlineLayout { hang_glyphs: true, ..Default::default() });
    let text = state.doc.text.to_string();
    let caret = match kind {
        // The end of step 1's paragraph: the person just types.
        Kind::Tour => text.find("edge of the window.").map(|i| i + "edge of the window.".len()),
        // The empty item under "Yours".
        Kind::Agent => text.find("## Yours").and_then(|y| text[y..].find("\n- ").map(|i| y + i + 3)),
    };
    if let Some(byte) = caret {
        let pos = text[..byte].chars().count();
        state.view.selection = Selection::point(pos);
    }
    state
}

/// The tour section the caret is in: the number of the last `## N ·` heading at or above
/// `line` (0 above the first, `None` after the closing paragraph), and how many there are.
pub(crate) fn tour_section(text: &str, line: usize) -> (Option<u32>, u32) {
    let mut current = Some(0);
    let mut total = 0;
    for (i, l) in text.lines().enumerate() {
        if let Some(rest) = l.strip_prefix("## ")
            && let Some(n) = rest.split(' ').next().and_then(|n| n.parse::<u32>().ok())
        {
            total = total.max(n);
            if i <= line {
                current = Some(n);
            }
        } else if l.starts_with("That's the tour") && i <= line {
            current = None;
        }
    }
    (current, total)
}

/// The status bar's hint for the section the caret is in. `mark` is the caret's block's
/// mark (shown in the marks section).
pub(crate) fn tour_hint(text: &str, line: usize, mark: Option<u64>) -> String {
    let (section, total) = tour_section(text, line);
    let Some(n) = section else { return "tour done ✦ next: caretline demo agent".into() };
    let what = match n {
        0 => "↓ to start: the status bar follows the caret".to_string(),
        1 => "type past the edge: lines wrap at words".into(),
        2 => "⌥← ⌥→ by word · ↑ ↓ by row, keeping the column".into(),
        3 => "⇧ + arrow selects · ⇧⌥ by word · Esc collapses".into(),
        4 => "⌃N adds a caret below · type · Esc for one".into(),
        5 => "type over a selection · ⌃Z undo · ⌃Y redo".into(),
        6 => "Tab ⇧Tab indent · ⌥↑ ⌥↓ move with children".into(),
        7 => "⌃O folds the lines under the caret's item".into(),
        8 => match mark {
            Some(m) => format!("mark #{m} · ⌥↑ it, cut it, undo: the # stays"),
            None => "every block keeps its mark through edits".into(),
        },
        9 => "⌃G opens a second view below · ⌃G closes it".into(),
        10 => "⌃D dumps the whole editor as JSON".into(),
        11 => "⌃P replays your session from the start".into(),
        _ => "↓ for the next step".into(),
    };
    if n == 0 { what } else { format!("{n}/{total} · {what}") }
}

/// What the agent thread reports, for the status bar.
#[derive(Default, Clone, Debug)]
pub(crate) struct Progress {
    pub started: bool,
    pub done: bool,
    pub writes: u64,
    pub stale: u64,
    pub error: Option<String>,
}

/// The tour and the agent demo, inside the editor.
struct EditorDemo {
    kind: Kind,
    dir: PathBuf,
    hint: Option<String>,
    replay: Option<Replay>,
    generation: u64,
    agent: Arc<Mutex<Progress>>,
}

impl EditorDemo {
    fn hint(&self, hub: &Hub) -> String {
        match self.kind {
            Kind::Tour => {
                let state = hub.session.state();
                let caret = state.view.caret();
                let line = state.doc.text.char_to_line(caret.min(state.doc.text.len_chars()));
                let mark = state.blocks().and_then(|o| {
                    let i = o.index_at(state.doc.text.slice(..), caret);
                    o.blocks.get(i).map(|b| b.id.0)
                });
                tour_hint(&state.doc.text.to_string(), line, mark)
            }
            Kind::Agent => {
                let p = self.agent.lock().map(|p| p.clone()).unwrap_or_default();
                if let Some(e) = p.error {
                    format!("agent stopped: {e}")
                } else if p.done {
                    format!("agent done: {} writes, {} refused and retried · ⌃P replays you both", p.writes, p.stale)
                } else if p.started {
                    "you + agent editing · ⌃Z undoes only yours · ⌃P replay".into()
                } else {
                    "the agent is connecting… start typing".into()
                }
            }
        }
    }

    fn status(&self, hub: &mut Hub, text: String) {
        dispatch_demo(hub, vec![Msg::ShowStatus { text }]);
    }

    /// ⌃O: folds or opens the caret's block, or the nearest one above it with children.
    fn toggle_fold(&self, hub: &mut Hub) {
        let state = hub.session.state();
        let Some(outline) = state.blocks() else { return };
        let blocks = &outline.blocks;
        if blocks.is_empty() {
            return;
        }
        let text = state.doc.text.slice(..);
        let mut i = outline.index_at(text, state.view.caret());
        let has_children = |i: usize| blocks.get(i + 1).is_some_and(|b| b.depth > blocks[i].depth);
        if !has_children(i) {
            let depth = blocks[i].depth;
            match (0..i).rev().find(|&k| blocks[k].depth < depth && has_children(k)) {
                Some(k) => i = k,
                None => {
                    self.status(hub, "⌃O folds an item that has children".into());
                    return;
                }
            }
        }
        let id = blocks[i].id;
        let depth = blocks[i].depth;
        let hidden = blocks[i + 1..].iter().take_while(|b| b.depth > depth).count();
        let folding = !state.view.folds.contains(&id);
        let text = if folding {
            format!("folded {hidden} item{} · ⌃O opens", if hidden == 1 { "" } else { "s" })
        } else {
            "opened".to_string()
        };
        dispatch_demo(hub, vec![Msg::ToggleFold { id }, Msg::ShowStatus { text }]);
    }

    /// ⌃N: another caret on the row below the last one, in the same column.
    fn caret_below(&self, hub: &mut Hub) {
        let mut state = hub.session.state().clone();
        let text = &state.doc.text;
        let ranges = state.view.selection.ranges().to_vec();
        let Some(last) = ranges.iter().max_by_key(|r| r.head) else { return };
        let line = text.char_to_line(last.head);
        if line + 1 >= text.len_lines() {
            self.status(hub, "no row below".into());
            return;
        }
        let col = last.head - text.line_to_char(line);
        let next = text.line(line + 1);
        let len = next.len_chars() - if next.chars().last() == Some('\n') { 1 } else { 0 };
        let pos = text.line_to_char(line + 1) + col.min(len);
        let primary = state.view.selection.primary_index();
        let mut all: caretline::helix::SmallVec<[caretline::helix::Range; 1]> = ranges.into_iter().collect();
        all.push(caretline::helix::Range::point(pos));
        let n = all.len();
        state.view.selection = Selection::new(all, primary);
        state.view.status = Some(format!("{n} carets · type · Esc for one"));
        hub.session.set_state(state);
    }

    /// ⌃G: a second view of the document, drawn below, or closes it.
    fn toggle_view(&self, hub: &mut Hub) {
        if let Some((id, _)) = hub.session.views().first() {
            let id = *id;
            hub.session.close_view(id);
            self.status(hub, "one view again".into());
            return;
        }
        let mut view = hub.session.state().view.clone();
        view.status = Some("view 1 · the same document, its own caret and scroll".into());
        hub.session.open_view(view);
        self.status(hub, "a second view below · type here, watch it there".into());
    }

    /// ⌃D: the whole editor as JSON, written next to the document.
    fn dump(&self, hub: &mut Hub) {
        let state = hub.session.state();
        let json = state.to_json();
        let lean = serde_json::to_string(&state.without_history()).map(|h| h.len()).unwrap_or(0);
        let history = json.len().saturating_sub(lean);
        let path = self.dir.join("state.json");
        // A headless snapshot has no directory: it measures, and writes nothing.
        let written = if self.dir.as_os_str().is_empty() { Ok(()) } else { std::fs::write(&path, &json) };
        let text = match written {
            Ok(()) => format!(
                "state.json · {} · undo {} · {} msgs",
                kb(json.len()),
                kb(history),
                hub.session.trace().iter().filter(|l| matches!(l, TraceLine::Msg(_) | TraceLine::On(_))).count()
            ),
            Err(e) => format!("couldn't write state.json: {e}"),
        };
        self.status(hub, text);
    }
}

fn kb(n: usize) -> String {
    if n < 1024 { format!("{n} B") } else { format!("{:.1} KB", n as f64 / 1024.0) }
}

/// Ctrl and a letter, nothing else.
fn ctrl(key: &Key) -> Option<char> {
    match key.code {
        KeyCode::Char(c) if key.mods.ctrl && !key.mods.alt && !key.mods.cmd => Some(c.to_ascii_lowercase()),
        _ => None,
    }
}

impl Demo for EditorDemo {
    fn key(&mut self, hub: &mut Hub, key: &Key) -> KeyAction {
        if let Some(r) = &self.replay {
            // Any key stops a replay.
            let n = r.applied();
            self.replay = None;
            self.generation += 1;
            self.status(hub, format!("replay stopped after {n} messages"));
            return KeyAction::Consumed;
        }
        // Esc with several carets keeps one, the primary.
        if key.code == KeyCode::Esc && !key.mods.ctrl && !key.mods.alt && !key.mods.cmd && !key.mods.shift {
            let sel = &hub.session.state().view.selection;
            if sel.ranges().len() > 1 && sel.ranges().iter().all(|r| r.anchor == r.head) {
                let mut state = hub.session.state().clone();
                state.view.selection = Selection::point(sel.primary().head);
                hub.session.set_state(state);
                return KeyAction::Consumed;
            }
        }
        match ctrl(key) {
            Some('o') => self.toggle_fold(hub),
            Some('n') => self.caret_below(hub),
            Some('g') => self.toggle_view(hub),
            Some('d') => self.dump(hub),
            Some('p') => {
                let pane = hub.session.views().first().map(|(_, v)| v.viewport.height).unwrap_or(0);
                self.replay = Some(Replay::new(hub.session.trace(), hub.session.state(), pane, Instant::now()));
                self.generation += 1;
            }
            _ => return KeyAction::Pass,
        }
        KeyAction::Consumed
    }

    fn after(&mut self, hub: &mut Hub) {
        let hint = self.hint(hub);
        let status = hub.session.state().view.status.clone();
        let showing_ours = status.is_none() || status == self.hint;
        if showing_ours && status.as_ref() != Some(&hint) {
            self.status(hub, hint.clone());
        } else if self.hint.is_none() {
            // The first call: replace the runtime's own start-up status.
            self.status(hub, hint.clone());
        }
        self.hint = Some(hint);
    }

    fn poll(&mut self, hub: &mut Hub, now: Instant) -> Option<Instant> {
        let r = self.replay.as_mut()?;
        if r.advance(now) {
            self.generation += 1;
        }
        if r.done() {
            let same = r.matches();
            let n = r.applied();
            self.replay = None;
            self.generation += 1;
            let text = if same {
                format!("✓ replayed {n} messages: the identical state, undo history and all")
            } else {
                format!("replayed {n} messages, but the state differs: please report this")
            };
            self.status(hub, text);
            return None;
        }
        r.next_due()
    }

    fn pane_rows(&self, hub: &Hub, height: u16) -> u16 {
        if hub.session.views().is_empty() { 0 } else { pane_rows(height) }
    }

    fn overlay(&self) -> Option<Frame> {
        self.replay.as_ref().map(|r| r.frame())
    }

    fn generation(&self) -> u64 {
        self.generation
    }
}

/// The agent's pane: a third of the terminal, at least 7 rows.
pub(crate) fn pane_rows(height: u16) -> u16 {
    (height / 3).max(7).min(height.saturating_sub(4))
}

fn editor_demo(args: &DemoArgs, kind: Kind) -> Result<(), String> {
    let name = match kind {
        Kind::Tour => "tour.md",
        Kind::Agent => "agent.md",
    };
    if let Some(size) = &args.snapshot {
        let (w, h) = crate::parse_size(size)?;
        let frame = snapshot(kind, w, h, args.keys.as_deref())?;
        print!("{}", if args.format == "ansi" { frame.to_ansi() } else { frame.to_text() });
        return Ok(());
    }
    let dir = demo_dir(args)?;
    let path = dir.join(name);
    let (width, height) = crossterm::terminal::size().unwrap_or((80, 24));
    let state = initial_state(kind, Some(path.to_string_lossy().into_owned()), Viewport { width, height });
    // The document is on disk too, so ⌃S has somewhere to go and the person can keep it.
    let text = state.doc.text.to_string();
    std::fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;

    let agent = Arc::new(Mutex::new(Progress::default()));
    let listen = match kind {
        Kind::Agent => {
            let socket = crate::hub::default_socket_path()?;
            agent::spawn(socket.clone(), agent.clone());
            Some(socket)
        }
        Kind::Tour => None,
    };
    let demo = EditorDemo { kind, dir: dir.clone(), hint: None, replay: None, generation: 0, agent };
    let file = path.to_string_lossy().into_owned();
    let result = runtime::run_interactive(
        state,
        runtime::Interactive {
            trace: None,
            mouse: !args.no_mouse,
            listen,
            file: Some(&file),
            trace_limit: caretline::session::DEFAULT_TRACE_LIMIT,
            max_fps: 120,
            frame_clock: 0,
            stats: false,
            demo: Some(Box::new(demo)),
        },
    );
    eprintln!("caretline demo: the files are in {}", dir.display());
    result
}

/// The demo's frame at `w`x`h`, as the editor draws it after `keys` (its first frame
/// without), headless. A replay (⌃P) shows at its start.
pub(crate) fn snapshot(kind: Kind, w: u16, h: u16, keys: Option<&str>) -> Result<Frame, String> {
    let mut demo = EditorDemo {
        kind,
        dir: PathBuf::new(),
        hint: None,
        replay: None,
        generation: 0,
        agent: Arc::new(Mutex::new(Progress::default())),
    };
    let rows = if kind == Kind::Agent { pane_rows(h) } else { 0 };
    let state = initial_state(kind, Some(format!("{}.md", if kind == Kind::Tour { "tour" } else { "agent" })), Viewport { width: w, height: h - rows });
    let mut hub = Hub::new(Session::new(state), None);
    if kind == Kind::Agent {
        // What the agent does first: a view of its own at the end of the document.
        let mut view = hub.session.state().view.clone();
        view.viewport = Viewport { width: w, height: rows };
        view.status = None;
        let id = hub.session.open_view(view);
        hub.session.apply_on(id, Msg::Move { dir: caretline::Dir::Forward, by: caretline::By::DocEnd, extend: false });
        hub.session.apply_on(id, Msg::ShowStatus { text: agent::CONNECTING.into() });
    }
    demo.after(&mut hub);
    for item in caretline::parse_keys(keys.unwrap_or(""))? {
        match item {
            caretline::keymap::ScriptItem::Key(key) => {
                if let KeyAction::Pass = demo.key(&mut hub, &key) {
                    let outline = hub.session.state().doc.outline.is_some();
                    if let Some(msg) = caretline::keymap_for(outline, &key) {
                        dispatch_demo(&mut hub, vec![msg]);
                    }
                }
            }
            caretline::keymap::ScriptItem::Wait(ms) => {
                let now_ms = hub.session.state().doc.now_ms + ms;
                dispatch_demo(&mut hub, vec![Msg::Tick { now_ms }]);
            }
        }
        demo.after(&mut hub);
    }
    // A view opened by the keys (⌃G) takes the bottom of the terminal, as it would live.
    let rows = if hub.session.views().is_empty() { 0 } else { pane_rows(h) };
    if hub.session.state().view.viewport.height != h - rows {
        dispatch_demo(&mut hub, vec![Msg::Resize { width: w, height: h - rows }]);
    }
    Ok(demo.overlay().unwrap_or_else(|| runtime::compose(&hub, rows)))
}

/// For tests: whether `dir` holds what a demo wrote.
#[allow(dead_code)]
pub(crate) fn wrote(dir: &Path, name: &str) -> bool {
    dir.join(name).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tour_hint_follows_the_caret() {
        let line = |needle: &str| TOUR.lines().position(|l| l.contains(needle)).unwrap();
        assert!(tour_hint(TOUR, 0, None).starts_with("↓ to start"));
        assert!(tour_hint(TOUR, line("just start typing"), None).starts_with("1/11 · "));
        assert!(tour_hint(TOUR, line("- one pear"), None).starts_with("4/11 · ⌃N"));
        assert!(tour_hint(TOUR, line("Hold on to me"), Some(7)).contains("#7"));
        assert!(tour_hint(TOUR, line("That's the tour"), None).starts_with("tour done"));
        assert!(!TOUR.contains("[ ]") && !TOUR.contains("⌃T"), "the tour shows no tasks");
    }

    #[test]
    fn the_tour_starts_with_the_caret_at_step_one() {
        let s = initial_state(Kind::Tour, None, Viewport { width: 80, height: 24 });
        let text = s.doc.text.to_string();
        let before: String = text.chars().take(s.view.caret()).collect();
        assert!(before.ends_with("edge of the window."), "{before:?}");
    }

    #[test]
    fn fold_dump_and_replay_keys_work() {
        let dir = std::env::temp_dir().join(format!("caretline-demo-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = initial_state(Kind::Tour, None, Viewport { width: 80, height: 40 });
        let mut hub = Hub::new(Session::new(state), None);
        let mut demo = EditorDemo { kind: Kind::Tour, dir: dir.clone(), hint: None, replay: None, generation: 0, agent: Default::default() };
        demo.after(&mut hub);
        let ctrl_key = |c| Key { code: KeyCode::Char(c), mods: caretline::Mods { ctrl: true, ..Default::default() } };

        // Fold: the caret on "Put the caret on this line…".
        let text = hub.session.state().doc.text.to_string();
        let at = text[..text.find("press ⌃O").unwrap()].chars().count();
        let mut s = hub.session.state().clone();
        s.view.selection = Selection::point(at);
        hub.session.set_state(s);
        assert!(matches!(demo.key(&mut hub, &ctrl_key('o')), KeyAction::Consumed));
        assert_eq!(hub.session.state().view.folds.len(), 1, "folded");

        // A second view, and closing it.
        demo.key(&mut hub, &ctrl_key('g'));
        assert_eq!(hub.session.views().len(), 1);
        demo.key(&mut hub, &ctrl_key('g'));
        assert!(hub.session.views().is_empty());

        // Carets: two more below "one apple", then Esc for one.
        let at = text[..text.find("one apple").unwrap()].chars().count();
        let mut s = hub.session.state().clone();
        s.view.selection = Selection::point(at);
        hub.session.set_state(s);
        demo.key(&mut hub, &ctrl_key('n'));
        demo.key(&mut hub, &ctrl_key('n'));
        assert_eq!(hub.session.state().view.selection.ranges().len(), 3);
        dispatch_demo(&mut hub, vec![Msg::InsertText { text: "two ".into() }]);
        assert_eq!(hub.session.state().doc.text.to_string().matches("two one").count(), 3);
        let esc = Key { code: KeyCode::Esc, mods: Default::default() };
        demo.key(&mut hub, &esc);
        assert_eq!(hub.session.state().view.selection.ranges().len(), 1);

        // Dump.
        demo.key(&mut hub, &ctrl_key('d'));
        assert!(wrote(&dir, "state.json"));
        assert!(hub.session.state().view.status.as_deref().unwrap().starts_with("state.json · "));

        // Replay: runs to the end and finds the identical state.
        dispatch_demo(&mut hub, vec![Msg::InsertText { text: "hello".into() }]);
        demo.key(&mut hub, &ctrl_key('p'));
        assert!(demo.overlay().is_some());
        let far = Instant::now() + std::time::Duration::from_secs(120);
        demo.poll(&mut hub, far);
        assert!(demo.overlay().is_none());
        let status = hub.session.state().view.status.clone().unwrap();
        assert!(status.starts_with("✓ replayed"), "{status}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
