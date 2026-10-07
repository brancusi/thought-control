//! User flows: the TUI driven as a person drives it, a step at a time, with the invariants
//! checked after every step. See docs/testing-flows.md.
//!
//! A flow reads as a story:
//!
//! ```ignore
//! flow("jump to a page and type")
//!     .keys("<c-o>Q4 Plan<cr>")
//!     .expect_page("Q4 Plan")
//!     .type_text("hello")
//!     .expect_caret_after("hello")
//!     .done();
//! ```
//!
//! Every step goes through [`Session::apply`] (the one door every input takes), is drawn into
//! one long-lived emulated terminal (as the live loop draws frame after frame), and is checked:
//! the frame isn't blank, the terminal shows exactly what a fresh draw would (no stale cells,
//! no torn wide characters), a render with the caches dropped draws the same, the state
//! round-trips through JSON, the caret is on screen where the document says it is, and what
//! may move moved (typing never shifts the chrome or the rows above the caret, nor scrolls).
//! At the end the flow's trace replays to the same frames, and its state restored on a fresh
//! session draws the same frame.

#![allow(dead_code)]

mod emu;
mod fixture;

mod agents;
mod editing;
mod lists;
mod monkey;
mod mouse;
mod nav;
mod perf;
mod sidebar;
mod typing;

use crate::app::{Focus, View};
use crate::session::{Msg, Session};
use emu::Emu;
pub use fixture::Size;
use ratatui::Terminal;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde_json::Value;
use std::time::Instant;
use unicode_width::UnicodeWidthStr;

/// What a step may move on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// Typing at the caret: the chrome (header, tabs, footer), the rows above the caret's row
    /// and the scroll stay where they were (the scroll may follow a caret on the last row).
    Typing,
    /// The caret moves (an arrow, a click): the chrome stays, and the view scrolls only to keep
    /// the caret on screen.
    Caret,
    /// Another writer's change lands (`thc add`, another device): the caret, the scroll and
    /// the rows above the caret's note stay; the header's badges and the footer may say so.
    Agent,
    /// Anything may change (navigation, overlays, the sidebar).
    Any,
}

/// Which checks run. Perf runs turn them off; the rest leave them on.
#[derive(Clone, Copy, Debug)]
pub struct Checks {
    pub invariants: bool,
    /// A render with the caches dropped draws the same frame (each step).
    pub cold: bool,
    pub replay: bool,
    pub restore: bool,
}

impl Default for Checks {
    fn default() -> Self {
        Checks { invariants: true, cold: true, replay: true, restore: true }
    }
}

/// One frame and what it was drawn from.
#[derive(Clone)]
pub struct Shot {
    pub buf: Buffer,
    pub text: Vec<String>,
    /// The terminal cursor, when shown.
    pub cursor: Option<(u16, u16)>,
    /// The main document's view (x, y, w, h).
    pub doc_view: Option<Rect>,
    /// Where the main area ends (the sidebar column or drawer starts), if a sidebar shows.
    pub side_x: Option<u16>,
    pub panels: Vec<Rect>,
    pub scroll: Option<usize>,
    /// The main document's caret (line, byte) and whether it has the keyboard.
    pub caret: Option<(usize, usize)>,
    pub writing: bool,
    pub overlay: bool,
    /// The screen row where the caret's note starts (its first row, or the view's top).
    pub caret_top: Option<u16>,
}

impl Shot {
    pub fn frame(&self) -> String {
        self.text.join("\n")
    }

    fn row(&self, y: u16, x0: u16, x1: u16) -> String {
        (x0..x1.min(self.buf.area.width)).map(|x| self.buf[(x, y)].symbol()).collect()
    }
}

/// Where to click: found on screen, never a hard-coded cell.
#[derive(Clone, Debug)]
pub enum At {
    /// Text drawn on screen (the `nth` match, top to bottom), `dx` cells into it.
    Text { s: String, nth: usize, dx: i32, region: Region },
    /// A place in the main document: the line holding `line`, `byte` bytes into its text.
    Doc { line: String, byte: usize },
    /// A clickable target the last frame declared.
    Target(fn(&crate::ui::Click) -> bool, &'static str),
    Cell(u16, u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Any,
    /// Left of the sidebar.
    Main,
    /// The sidebar.
    Side,
    /// The main document's view.
    Doc,
    /// The last row.
    Footer,
    /// The first two rows.
    Header,
}

/// Text on screen.
pub fn text(s: &str) -> At {
    At::Text { s: s.to_string(), nth: 0, dx: 0, region: Region::Any }
}

/// A place in the main document, by its line's text and a byte offset into it.
pub fn doc_at(line: &str, byte: usize) -> At {
    At::Doc { line: line.to_string(), byte }
}

/// The tab of a view.
pub fn tab(v: View) -> At {
    let f: fn(&crate::ui::Click) -> bool = match v {
        View::Today => |c| matches!(c, crate::ui::Click::View(View::Today)),
        View::Inbox => |c| matches!(c, crate::ui::Click::View(View::Inbox)),
        View::Tasks => |c| matches!(c, crate::ui::Click::View(View::Tasks)),
        View::Pages => |c| matches!(c, crate::ui::Click::View(View::Pages)),
        View::Journal => |c| matches!(c, crate::ui::Click::View(View::Journal)),
        View::Search => |c| matches!(c, crate::ui::Click::View(View::Search)),
        _ => |c| matches!(c, crate::ui::Click::View(View::Log)),
    };
    At::Target(f, "a view tab")
}

impl At {
    pub fn nth(mut self, n: usize) -> At {
        if let At::Text { nth, .. } = &mut self {
            *nth = n;
        }
        self
    }
    pub fn dx(mut self, d: i32) -> At {
        if let At::Text { dx, .. } = &mut self {
            *dx = d;
        }
        self
    }
    pub fn within(mut self, r: Region) -> At {
        if let At::Text { region, .. } = &mut self {
            *region = r;
        }
        self
    }
    pub fn in_main(self) -> At {
        self.within(Region::Main)
    }
    pub fn in_side(self) -> At {
        self.within(Region::Side)
    }
    pub fn in_doc(self) -> At {
        self.within(Region::Doc)
    }
    pub fn in_footer(self) -> At {
        self.within(Region::Footer)
    }
}

/// A flow: a scratch vault, a headless session on it, and the frames so far.
pub struct Flow {
    pub name: String,
    _scratch: crate::fuzz::Scratch,
    pub s: Session,
    pub(super) term: Terminal<Emu>,
    pub shot: Shot,
    pub step: usize,
    pub checks: Checks,
    /// Per step: the trace length after it and the frame it left.
    marks: Vec<(usize, String, String)>,
    pace_ms: u64,
    /// Logical time the steps have taken that no Tick has sent yet.
    owed_ms: u64,
    /// Each step's handling + draw time, by kind.
    pub timings: Vec<(&'static str, f64)>,
    /// Each timed step's description, beside `timings`.
    pub timed_steps: Vec<String>,
    kind: &'static str,
    pub done: bool,
    /// Known product bugs this flow steps around (board task ids), each narrowing one check.
    known: Vec<(String, Known)>,
}

/// A known bug (a board task) a flow steps around, narrowly, until it's fixed. Remove the
/// `.known(…)` when the task is done: the flow then holds the fix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Known {
    /// The left rail (pages, days) in a fresh session differs from the live one: its order and
    /// counts aren't in the state / go stale. The restore check leaves the rail out.
    Rail,
    /// A fresh session on the saved state lays the page out differently (02pjq: new notes'
    /// blank rows): the restore check is skipped.
    Restore,
}

/// The start of every flow: the base vault at 140x36, Today showing.
pub fn flow(name: &str) -> Flow {
    Flow::new(name, Size::Small, (140, 36), Checks::default())
}

/// A flow with the long pages too.
pub fn flow_with(name: &str, size: Size, screen: (u16, u16)) -> Flow {
    Flow::new(name, size, screen, Checks::default())
}

/// The clock every flow starts at: today 10:00 local (THC_NOW pins "today" when set).
fn start_ms() -> (u64, i32) {
    use chrono::{Local, TimeZone};
    let today = thc_core::dates::today();
    let t = Local.from_local_datetime(&today.and_hms_opt(10, 0, 0).unwrap()).earliest().unwrap();
    (t.timestamp_millis() as u64, t.offset().local_minus_utc() / 60)
}

impl Flow {
    pub fn new(name: &str, size: Size, screen: (u16, u16), checks: Checks) -> Flow {
        crate::SNAPSHOT.with(|s| s.set(true));
        let tag: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).take(24).collect();
        let (scratch, mut vault) = crate::fuzz::scratch(&format!("flow-{tag}"));
        fixture::seed(&mut vault, size);
        // The once-per-device drag hint is already seen (mouse::the_first_drag_hint tests it).
        let _ = std::fs::create_dir_all(&vault.paths.cache);
        let _ = std::fs::write(vault.paths.cache.join("mouse-hint-shown"), "");
        let mut app = crate::app::App::new(vault).unwrap();
        app.daemon_live = false;
        let mut s = Session::new(app, screen);
        let (now_ms, utc_offset_min) = start_ms();
        s.apply(Msg::Tick { now_ms, utc_offset_min }).unwrap();
        let mut term = Terminal::new(Emu::new(screen.0, screen.1)).unwrap();
        let shot = draw(&mut term, &mut s);
        let mut f = Flow { name: name.to_string(), _scratch: scratch, s, term, shot, step: 0, checks, marks: Vec::new(), pace_ms: 60, owed_ms: 0, timings: Vec::new(), timed_steps: Vec::new(), kind: "step", done: false, known: Vec::new() };
        f.checkpoint_mark();
        f
    }

    /// Rename the flow (a helper that opened a page names it for the story).
    pub fn named(&mut self, name: &str) -> &mut Self {
        self.name = name.to_string();
        self
    }

    /// Step around a known bug (a board task id) in one check; see [`Known`].
    pub fn known(&mut self, task: &str, what: Known) -> &mut Self {
        self.known.push((task.to_string(), what));
        self
    }

    pub fn checks(&mut self, c: Checks) -> &mut Self {
        self.checks = c;
        self
    }

    /// Milliseconds of logical time between steps (typing speed; 60 by default).
    pub fn pace(&mut self, ms: u64) -> &mut Self {
        self.pace_ms = ms;
        self
    }

    fn checkpoint_mark(&mut self) {
        let len = self.s.trace(None, true).map(|(_, l)| l.len()).unwrap_or(0);
        self.marks.push((len, "start".into(), self.shot.frame()));
    }

    // ---- steps -------------------------------------------------------------------------

    /// A key script (script.rs), each token its own step. Anything may change.
    pub fn keys(&mut self, script: &str) -> &mut Self {
        self.keys_as(Motion::Any, script)
    }

    /// A key script whose steps may move only what `motion` allows.
    pub fn keys_as(&mut self, motion: Motion, script: &str) -> &mut Self {
        let groups = crate::script::parse_grouped(script, true).unwrap_or_else(|e| panic!("flow `{}`: {e}", self.name));
        let tokens = tokens(script);
        for (i, g) in groups.into_iter().enumerate() {
            let desc = tokens.get(i).cloned().unwrap_or_else(|| script.to_string());
            self.run(&desc, motion, g);
        }
        self
    }

    /// Caret keys (arrows, word motion, a selection): the chrome stays.
    pub fn moves(&mut self, script: &str) -> &mut Self {
        self.keys_as(Motion::Caret, script)
    }

    /// Typing, a character at a time (`\n` is Enter).
    pub fn type_text(&mut self, text: &str) -> &mut Self {
        let prev = std::mem::replace(&mut self.kind, "type");
        for c in text.chars() {
            let key = match c {
                '\n' => "<cr>".to_string(),
                '<' => "<lt>".to_string(),
                c => c.to_string(),
            };
            self.run(&format!("type {key:?}"), if c == '\n' { Motion::Any } else { Motion::Typing }, vec![Msg::Key { key }]);
        }
        self.kind = prev;
        self
    }

    /// A bracketed paste.
    pub fn paste(&mut self, text: &str) -> &mut Self {
        self.run(&format!("paste {:?}", text.chars().take(30).collect::<String>()), Motion::Any, vec![Msg::Paste { text: text.to_string() }])
    }

    /// Label the steps `f` takes for the timings (`page_open`, `sidebar_open`, `scroll`).
    pub fn timed(&mut self, kind: &'static str, f: impl FnOnce(&mut Flow)) -> &mut Self {
        let prev = std::mem::replace(&mut self.kind, kind);
        f(self);
        self.kind = prev;
        self
    }

    fn mouse_script(&mut self, desc: &str, motion: Motion, token: String) -> &mut Self {
        let groups = crate::script::parse_grouped(&token, false).unwrap_or_else(|e| panic!("flow `{}`: {e}", self.name));
        for g in groups {
            self.run(&format!("{desc} {token}"), motion, g);
        }
        self
    }

    pub fn click(&mut self, at: At) -> &mut Self {
        let (x, y) = self.locate(&at);
        self.mouse_script(&format!("click {at:?}"), Motion::Any, format!("<click:{x},{y}>"))
    }

    /// A click that only places the caret: the chrome stays and nothing scrolls.
    pub fn click_caret(&mut self, at: At) -> &mut Self {
        let (x, y) = self.locate(&at);
        self.mouse_script(&format!("click {at:?}"), Motion::Caret, format!("<click:{x},{y}>"))
    }

    pub fn shift_click(&mut self, at: At) -> &mut Self {
        let (x, y) = self.locate(&at);
        self.mouse_script(&format!("⇧click {at:?}"), Motion::Any, format!("<sclick:{x},{y}>"))
    }

    /// ⌘-click: what WezTerm and Ghostty send when they pass ⌘ through.
    pub fn cmd_click(&mut self, at: At) -> &mut Self {
        let (x, y) = self.locate(&at);
        self.mouse_script(&format!("⌘click {at:?}"), Motion::Any, format!("<d-click:{x},{y}>"))
    }

    pub fn ctrl_click(&mut self, at: At) -> &mut Self {
        let (x, y) = self.locate(&at);
        self.mouse_script(&format!("⌃click {at:?}"), Motion::Any, format!("<cclick:{x},{y}>"))
    }

    pub fn alt_click(&mut self, at: At) -> &mut Self {
        let (x, y) = self.locate(&at);
        self.mouse_script(&format!("⌥click {at:?}"), Motion::Caret, format!("<aclick:{x},{y}>"))
    }

    pub fn double_click(&mut self, at: At) -> &mut Self {
        let (x, y) = self.locate(&at);
        self.mouse_script(&format!("double-click {at:?}"), Motion::Caret, format!("<dclick:{x},{y}>"))
    }

    pub fn triple_click(&mut self, at: At) -> &mut Self {
        let (x, y) = self.locate(&at);
        self.mouse_script(&format!("triple-click {at:?}"), Motion::Caret, format!("<tclick:{x},{y}>"))
    }

    pub fn drag(&mut self, from: At, to: At) -> &mut Self {
        let (x1, y1) = self.locate(&from);
        let (x2, y2) = self.locate(&to);
        self.mouse_script(&format!("drag {from:?} → {to:?}"), Motion::Caret, format!("<drag:{x1},{y1},{x2},{y2}>"))
    }

    /// The wheel, `n` notches, over `at` (or the middle of the screen).
    pub fn wheel(&mut self, down: bool, n: u32, at: Option<At>) -> &mut Self {
        let pos = at.map(|a| self.locate(&a));
        let dir = if down { "down" } else { "up" };
        let label = if self.kind == "step" { "scroll" } else { self.kind };
        let prev = std::mem::replace(&mut self.kind, label);
        for _ in 0..n {
            let token = match pos {
                Some((x, y)) => format!("<wheel:{dir}@{x},{y}>"),
                None => format!("<wheel:{dir}>"),
            };
            self.mouse_script("wheel", Motion::Any, token);
        }
        self.kind = prev;
        self
    }

    pub fn hover(&mut self, at: At) -> &mut Self {
        let (x, y) = self.locate(&at);
        self.mouse_script("hover", Motion::Any, format!("<hover:{x},{y}>"))
    }

    pub fn resize(&mut self, w: u16, h: u16) -> &mut Self {
        self.run(&format!("resize {w}x{h}"), Motion::Any, vec![Msg::Resize { w, h }])
    }

    /// Time passes with nothing typed: the idle point (an undo step, the idle save).
    pub fn idle(&mut self) -> &mut Self {
        let now = self.s.app.ui.now_ms + std::mem::take(&mut self.owed_ms) + 2_000;
        self.s.apply(Msg::Tick { now_ms: now, utc_offset_min: self.s.app.ui.utc_offset_min }).unwrap();
        self.s.runtime(Msg::Idle);
        self.redraw("idle", Motion::Any)
    }

    /// An agent adds a line to today's journal (`thc add`), and the TUI takes it in.
    pub fn agent_add(&mut self, text: &str) -> &mut Self {
        self.run(&format!("agent adds {text:?}"), Motion::Agent, vec![Msg::Fixture { fixture: crate::session::Fixture::Agent { text: text.to_string() } }])
    }

    /// An agent pushes a state patch (`thc ui patch`).
    pub fn ui_patch(&mut self, patch: Value, actor: &str) -> &mut Self {
        self.run(&format!("{actor} patches {patch}"), Motion::Any, vec![Msg::Patch { patch, actor: Some(actor.to_string()) }])
    }

    /// Another device changes the text of the note whose text holds `line`.
    pub fn remote_edit(&mut self, line: &str, new_text: &str) -> &mut Self {
        let id = self.node_id(line);
        self.run(&format!("remote edit {line:?}"), Motion::Agent, vec![Msg::Fixture { fixture: crate::session::Fixture::Remote { id, text: new_text.to_string() } }])
    }

    /// Any message, as a step.
    pub fn msg(&mut self, desc: &str, motion: Motion, m: Msg) -> &mut Self {
        self.run(desc, motion, vec![m])
    }

    /// Everything typed so far saved, as leaving the TUI would.
    pub fn save(&mut self) -> &mut Self {
        // As the terminal losing focus saves: a message, so a replay saves there too.
        self.run("focus lost (saves)", Motion::Any, vec![Msg::Focus { gained: false }]);
        self.s.app.drain_saves(true);
        self
    }

    /// The terminal's paste (⌘V) of what the TUI last copied (⌃C / ⌃X).
    pub fn paste_clipboard(&mut self) -> &mut Self {
        let text = crate::runtime_effects::SNAPSHOT_CLIPBOARD.with(|c| c.borrow().clone()).unwrap_or_else(|| self.expect_fail("nothing was copied"));
        self.paste(&text)
    }

    fn node_id(&self, needle: &str) -> String {
        let nodes = self.s.app.vault.store.nodes_where("1=1", &[]).unwrap_or_default();
        if let Some(n) = nodes.iter().find(|n| n.text.contains(needle)) {
            return n.id.clone();
        }
        panic!("flow `{}`: no note holds {needle:?}", self.name)
    }

    /// One step: the clock moves on, the messages apply, the frame is drawn (and again after
    /// the runtime's after-frame work, as the live loop does), and the checks run.
    fn run(&mut self, desc: &str, motion: Motion, msgs: Vec<Msg>) -> &mut Self {
        self.step += 1;
        // The clock moves `pace` per step; a Tick goes out once a second's worth has built up
        // (each is a message to apply and replay; nothing a flow does needs finer).
        self.owed_ms += self.pace_ms;
        if self.owed_ms >= 1_000 {
            let now = self.s.app.ui.now_ms + std::mem::take(&mut self.owed_ms);
            let off = self.s.app.ui.utc_offset_min;
            self.s.apply(Msg::Tick { now_ms: now, utc_offset_min: off }).unwrap();
        }
        let before = self.shot.clone();
        let t0 = Instant::now();
        for m in msgs {
            // A terminal that changes size starts blank and ratatui draws it whole.
            if let Msg::Resize { w, h } = m {
                if (w, h) != self.s.size {
                    *self.term.backend_mut() = Emu::new(w, h);
                }
            }
            if let Err(e) = self.s.apply(m.clone()) {
                self.fail(desc, &before, &format!("refused {m:?}: {e}"));
            }
        }
        self.runtime_effects();
        let shot = draw(&mut self.term, &mut self.s);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        self.timings.push((self.kind, ms));
        self.timed_steps.push(format!("step {} {desc}", self.step));
        self.shot = shot;
        self.check(desc, motion, &before);
        // The runtime's after-frame work (the save of a line just left) and its frame.
        if self.s.app.doc_save_after_frame {
            self.s.runtime(Msg::Frame);
            let mid = self.shot.clone();
            self.shot = draw(&mut self.term, &mut self.s);
            self.check(&format!("{desc} (after-frame)"), motion, &mid);
        }
        let len = self.s.trace(None, true).map(|(_, l)| l.len()).unwrap_or(0);
        self.marks.push((len, desc.to_string(), self.shot.frame()));
        self
    }

    /// Redraw after something the runtime did outside a message.
    fn redraw(&mut self, desc: &str, motion: Motion) -> &mut Self {
        self.step += 1;
        let before = self.shot.clone();
        self.s.sync_external();
        self.shot = draw(&mut self.term, &mut self.s);
        self.check(desc, motion, &before);
        let len = self.s.trace(None, true).map(|(_, l)| l.len()).unwrap_or(0);
        self.marks.push((len, desc.to_string(), self.shot.frame()));
        self
    }

    /// What the live runtime does with a step's requests: the mouse mode, no $EDITOR.
    fn runtime_effects(&mut self) {
        self.s.drop_effects();
        let app = &mut self.s.app;
        app.reexec = false;
        if let Some(on) = app.mouse_request.take() {
            app.tui_prefs.mouse = on;
        }
        if app.editor_request.take().is_some() {
            app.info("($EDITOR skipped in a flow)");
        }
        app.switch_to = None;
    }

    // ---- anchors -----------------------------------------------------------------------

    /// The cell an anchor names on the last frame.
    pub fn locate(&self, at: &At) -> (u16, u16) {
        self.try_locate(at).unwrap_or_else(|e| panic!("flow `{}` step {}: {e}\n{}", self.name, self.step, self.shot.frame()))
    }

    pub fn try_locate(&self, at: &At) -> Result<(u16, u16), String> {
        let shot = &self.shot;
        let (w, h) = (shot.buf.area.width, shot.buf.area.height);
        match at {
            At::Cell(x, y) => Ok((*x, *y)),
            At::Text { s, nth, dx, region } => {
                let (x0, x1, y0, y1) = match region {
                    Region::Any => (0, w, 0, h),
                    Region::Main => (0, shot.side_x.unwrap_or(w), 0, h),
                    Region::Side => (shot.side_x.ok_or("no sidebar on screen")?, w, 0, h),
                    Region::Doc => {
                        let r = shot.doc_view.ok_or("no document on screen")?;
                        (r.x, r.x + r.width, r.y, r.y + r.height)
                    }
                    Region::Footer => (0, w, h - 1, h),
                    Region::Header => (0, w, 0, 2.min(h)),
                };
                let mut found = 0;
                for y in y0..y1 {
                    // The row as characters, each with its cell.
                    let mut chars: Vec<(char, u16)> = Vec::new();
                    let mut skip = 0;
                    for x in x0..x1 {
                        if skip > 0 {
                            skip -= 1;
                            continue;
                        }
                        let sym = shot.buf[(x, y)].symbol();
                        skip = UnicodeWidthStr::width(sym).saturating_sub(1);
                        for c in sym.chars() {
                            chars.push((c, x));
                        }
                    }
                    let row: String = chars.iter().map(|(c, _)| *c).collect();
                    let mut from = 0;
                    while let Some(i) = row[from..].find(s.as_str()) {
                        let ci = row[..from + i].chars().count();
                        if found == *nth {
                            let x = chars[ci].1 as i32 + dx;
                            return Ok((x.clamp(0, w as i32 - 1) as u16, y));
                        }
                        found += 1;
                        from += i + s.len().max(1);
                    }
                }
                Err(format!("{s:?} (match {nth}) isn't on screen in {region:?}"))
            }
            At::Doc { line, byte } => {
                let d = self.s.app.doc.as_ref().ok_or("no document open")?;
                let idx = d.blocks().iter().position(|l| l.text.contains(line.as_str())).ok_or_else(|| format!("no line holds {line:?}"))?;
                let r = shot.doc_view.ok_or("no document on screen")?;
                for y in r.y..r.y + r.height {
                    for x in r.x..r.x + r.width {
                        if let Some((l, b, false)) = crate::doc_ui::hit_at(&self.s.app, &self.s.app.render, x, y) {
                            if l == idx && b == *byte {
                                return Ok((x, y));
                            }
                        }
                    }
                }
                Err(format!("byte {byte} of {line:?} isn't on screen"))
            }
            At::Target(f, what) => {
                let t = self.s.app.render.click_targets.iter().find(|t| f(&t.what)).ok_or_else(|| format!("no {what} on screen"))?;
                Ok((t.x0 + (t.x1 - t.x0) / 2, t.y))
            }
        }
    }

    // ---- checks ------------------------------------------------------------------------

    fn fail(&self, desc: &str, before: &Shot, msg: &str) -> ! {
        let cursor = |s: &Shot| s.cursor.map_or("hidden".to_string(), |(x, y)| format!("({x},{y})"));
        panic!(
            "\nflow `{}` step {} `{desc}`: {msg}\n--- before (cursor {}) ---\n{}\n--- after (cursor {}) ---\n{}\n",
            self.name,
            self.step,
            cursor(before),
            numbered(&before.text),
            cursor(&self.shot),
            numbered(&self.shot.text)
        );
    }

    fn check(&mut self, desc: &str, motion: Motion, before: &Shot) {
        if !self.checks.invariants {
            return;
        }
        let shot = self.shot.clone();
        // A frame is never blank.
        if shot.text.iter().all(|l| l.trim().is_empty()) {
            self.fail(desc, before, "a blank frame");
        }
        // The terminal shows what a whole draw would: no stale cells, no torn wide characters.
        let diff = self.term.backend().diff(&shot.buf);
        if !diff.is_empty() {
            let cells: Vec<String> = diff.iter().take(12).map(|(x, y, a, b)| format!("({x},{y}) shows {a:?}, want {b:?}")).collect();
            self.fail(desc, before, &format!("the terminal differs from the frame in {} cells (stale or torn): {}", diff.len(), cells.join("; ")));
        }
        let cut = std::mem::take(&mut self.term.backend_mut().cut);
        if !cut.is_empty() {
            self.fail(desc, before, &format!("wide characters cut at the right edge: {cut:?}"));
        }
        // A render with the caches dropped draws the same frame.
        if self.checks.cold {
            let cache = std::mem::take(&mut self.s.app.derived.sidebar_cache);
            let preview = self.s.app.derived.preview.take();
            let (w, h) = self.s.size;
            let cold = self.s.render(w, h, "text").unwrap().frame.unwrap();
            let warm = crate::session::frame_text(&shot.buf);
            if cold != warm {
                let rows: Vec<String> = cold.lines().zip(warm.lines()).enumerate().filter(|(_, (a, b))| a != b).take(6).map(|(y, (a, b))| format!("row {y}:\n  cold {a:?}\n  warm {b:?}")).collect();
                self.fail(desc, before, &format!("a render without the caches differs (stale cache):\n{}", rows.join("\n")));
            }
            drop((cache, preview));
        }
        // The state round-trips through JSON.
        let state = self.s.state().clone();
        let text = serde_json::to_string(&state).unwrap();
        match serde_json::from_str::<crate::ui_state::UiState>(&text) {
            Ok(back) if back == state => {}
            Ok(back) => {
                let a = serde_json::to_value(&state).unwrap();
                let b = serde_json::to_value(&back).unwrap();
                self.fail(desc, before, &format!("the state doesn't round-trip through JSON: {}", json_diff(&a, &b)));
            }
            Err(e) => self.fail(desc, before, &format!("the state's JSON doesn't parse back: {e}")),
        }
        // The caret: on screen while writing, on the document's caret.
        if shot.writing {
            let Some((cx, cy)) = shot.cursor else { self.fail(desc, before, "writing, but no cursor shows") };
            let r = shot.doc_view.unwrap();
            if !(cx >= r.x && cx < r.x + r.width && cy >= r.y && cy < r.y + r.height) {
                self.fail(desc, before, &format!("the cursor ({cx},{cy}) is outside the document's view {r:?}"));
            }
            if let (Some(hit), Some(caret)) = (crate::doc_ui::hit_at(&self.s.app, &self.s.app.render, cx, cy), shot.caret) {
                if (hit.0, hit.1) != caret {
                    let d = self.s.app.doc.as_ref().unwrap();
                    let t = d.blocks().get(caret.0).map(|l| l.text.clone()).unwrap_or_default();
                    self.fail(desc, before, &format!("the cursor ({cx},{cy}) is on line {} byte {}, the caret on line {} byte {} of {t:?}", hit.0, hit.1, caret.0, caret.1));
                }
            }
        }
        // What may move.
        if motion == Motion::Any || before.overlay || shot.overlay {
            return;
        }
        let (Some(a), Some(b)) = (before.doc_view, shot.doc_view) else { return };
        if a != b {
            self.fail(desc, before, &format!("the document's view moved: {a:?} → {b:?}"));
        }
        let w = shot.buf.area.width;
        let main_r = shot.side_x.unwrap_or(w).min(before.side_x.unwrap_or(w));
        for y in 0..shot.buf.area.height {
            if y >= b.y && y < b.y + b.height || motion == Motion::Agent {
                continue;
            }
            let (mut p, mut q) = (before.row(y, 0, main_r), shot.row(y, 0, main_r));
            // The footer's counts (words, open tasks) change as you type; its layout doesn't.
            // The footer: its hints follow the caret (on a link: ⌥O aside) and its counts the
            // words; its status (the page, autosaved) stays put.
            // A toast takes the footer for a moment (by design).
            if y + 1 == shot.buf.area.height {
                if self.s.app.ui.toast.is_some() {
                    continue;
                }
                (p, q) = (footer_status(&p), footer_status(&q));
            }
            if p != q {
                self.fail(desc, before, &format!("row {y} (chrome) changed:\n  before {:?}\n  after  {:?}", p.trim_end(), q.trim_end()));
            }
        }
        let bottom = b.y + b.height - 1;
        let at_edge = |s: &Shot| s.cursor.is_some_and(|(_, y)| y == b.y || y == bottom);
        if before.scroll != shot.scroll && !at_edge(&shot) && !at_edge(before) {
            self.fail(desc, before, &format!("the view scrolled {:?} → {:?} with the caret mid-screen", before.scroll, shot.scroll));
        }
        if matches!(motion, Motion::Typing | Motion::Agent) && before.scroll == shot.scroll {
            // Rows above the caret's note (a note's own rows reflow: a word may move up a row).
            let top = [before.caret_top, shot.caret_top].iter().flatten().copied().min();
            if let Some(cy) = top {
                for y in b.y..cy {
                    let (p, q) = (before.row(y, b.x, b.x + b.width), shot.row(y, b.x, b.x + b.width));
                    if p != q {
                        self.fail(desc, before, &format!("row {y}, above the caret's note (from row {cy}), changed while typing:\n  before {:?}\n  after  {:?}", p.trim_end(), q.trim_end()));
                    }
                }
            }
        }
    }

    /// The end of a flow: its trace replays to the same frames, and its state restored on a
    /// fresh session (after a save) draws the same frame.
    pub fn done(&mut self) {
        self.done = true;
        if self.checks.replay {
            let trace: String = self.s.trace(None, true).unwrap().1.iter().map(|l| format!("{l}\n")).collect();
            let want: std::collections::HashSet<usize> = self.marks.iter().map(|(len, _, _)| len.saturating_sub(1)).collect();
            let frames: std::collections::HashMap<usize, String> = replay(&self.s.app.vault.paths, &trace, &want).into_iter().collect();
            for (len, desc, frame) in &self.marks {
                let Some(got) = frames.get(&len.saturating_sub(1)) else {
                    panic!("flow `{}`: the replay drew no frame for step `{desc}` (line {len})", self.name);
                };
                let got = got.trim_end_matches('\n');
                if !same_but_ids(got, frame) {
                    panic!("\nflow `{}`: replaying the trace, step `{desc}` (line {len}) draws differently\n--- live ---\n{}\n--- replay ---\n{}\n", self.name, numbered_str(frame), numbered_str(got));
                }
            }
        }
        if let Some((task, _)) = self.known.iter().find(|(_, k)| *k == Known::Restore) {
            eprintln!("flow `{}`: the restore check is skipped (known: {task})", self.name);
        } else if self.checks.restore {
            self.save();
            let state = self.s.state().to_json();
            let paths = thc_core::vault::scratch_copy(&self.s.app.vault.paths).unwrap();
            let root = paths.vault.parent().unwrap().to_path_buf();
            let mut v = thc_core::vault::Vault::open(paths, self.s.app.vault.actor.clone(), "tui").unwrap();
            v.origin = Some(self.s.app.vault.paths.clone());
            let mut app = crate::app::App::new(v).unwrap();
            app.daemon_live = false;
            let mut fresh = Session::new(app, self.s.size);
            fresh.restore(&state).unwrap_or_else(|e| panic!("flow `{}`: the state doesn't restore: {e}", self.name));
            let (w, h) = self.s.size;
            let a = self.s.render(w, h, "text").unwrap().frame.unwrap();
            let b = fresh.render(w, h, "text").unwrap().frame.unwrap();
            let _ = std::fs::remove_dir_all(root);
            let (a, b) = match self.known.iter().find(|(_, k)| *k == Known::Rail) {
                Some((task, _)) => {
                    eprintln!("flow `{}`: the restore check leaves out the rail (known: {task})", self.name);
                    (without_rail(&a), without_rail(&b))
                }
                None => (a, b),
            };
            if a != b {
                panic!("\nflow `{}`: its state restored on a fresh session draws differently\n--- live ---\n{}\n--- fresh ---\n{}\n", self.name, numbered_str(&a), numbered_str(&b));
            }
        }
    }

    // ---- expectations ------------------------------------------------------------------

    fn expect_fail(&self, msg: &str) -> ! {
        panic!("\nflow `{}` after step {}: {msg}\n{}\n", self.name, self.step, numbered(&self.shot.text))
    }

    /// Any check on the session.
    pub fn expect(&mut self, what: &str, f: impl FnOnce(&mut Session) -> bool) -> &mut Self {
        if !f(&mut self.s) {
            self.expect_fail(&format!("expected {what}"));
        }
        self
    }

    pub fn expect_screen(&mut self, s: &str) -> &mut Self {
        if self.try_locate(&text(s)).is_err() {
            self.expect_fail(&format!("{s:?} isn't on screen"));
        }
        self
    }

    pub fn expect_no_screen(&mut self, s: &str) -> &mut Self {
        if self.try_locate(&text(s)).is_ok() {
            self.expect_fail(&format!("{s:?} is on screen"));
        }
        self
    }

    pub fn expect_at(&mut self, at: At) -> &mut Self {
        if let Err(e) = self.try_locate(&at) {
            self.expect_fail(&e);
        }
        self
    }

    pub fn expect_view(&mut self, v: View) -> &mut Self {
        if self.s.app.ui.view != v {
            self.expect_fail(&format!("the view is {:?}, want {v:?}", self.s.app.ui.view));
        }
        self
    }

    /// The main document is the page titled `title`.
    pub fn expect_page(&mut self, title: &str) -> &mut Self {
        match self.s.app.doc.as_ref().map(|d| &d.target) {
            Some(crate::editor::Target::Page { title: t, .. }) if t == title => self,
            other => self.expect_fail(&format!("the document is {other:?}, want the page {title:?}")),
        }
    }

    /// The main document is the journal day `days` from today.
    pub fn expect_day(&mut self, days: i64) -> &mut Self {
        let want = thc_core::dates::today() + chrono::Duration::days(days);
        match self.s.app.doc.as_ref().map(|d| &d.target) {
            Some(crate::editor::Target::Journal { date }) if *date == want => self,
            other => self.expect_fail(&format!("the document is {other:?}, want the day {want}")),
        }
    }

    pub fn expect_no_doc(&mut self) -> &mut Self {
        if self.s.app.doc.is_some() {
            self.expect_fail("a document is open");
        }
        self
    }

    fn caret_line(&self) -> (String, usize) {
        let d = self.s.app.doc.as_ref().unwrap_or_else(|| self.expect_fail("no document open"));
        let c = d.caret();
        (d.blocks()[c.line].text.clone(), c.byte)
    }

    /// The text before the caret, on its line, ends with `s`.
    pub fn expect_caret_after(&mut self, s: &str) -> &mut Self {
        let (t, b) = self.caret_line();
        if !t[..b].ends_with(s) {
            self.expect_fail(&format!("the caret is at byte {b} of {t:?}, want it after {s:?}"));
        }
        self
    }

    /// The text after the caret, on its line, starts with `s`.
    pub fn expect_caret_before(&mut self, s: &str) -> &mut Self {
        let (t, b) = self.caret_line();
        if !t[b..].starts_with(s) {
            self.expect_fail(&format!("the caret is at byte {b} of {t:?}, want it before {s:?}"));
        }
        self
    }

    /// The caret's line reads `s`.
    pub fn expect_caret_line(&mut self, s: &str) -> &mut Self {
        let (t, b) = self.caret_line();
        if t != s {
            self.expect_fail(&format!("the caret's line is {t:?} (byte {b}), want {s:?}"));
        }
        self
    }

    /// The document has a line reading exactly `s`.
    pub fn expect_line(&mut self, s: &str) -> &mut Self {
        let lines = self.lines();
        if !lines.iter().any(|l| l == s) {
            self.expect_fail(&format!("no line reads {s:?}: {lines:#?}"));
        }
        self
    }

    pub fn expect_no_line(&mut self, s: &str) -> &mut Self {
        let lines = self.lines();
        if lines.iter().any(|l| l.contains(s)) {
            self.expect_fail(&format!("a line holds {s:?}: {lines:#?}"));
        }
        self
    }

    /// The document's lines (texts), in order.
    pub fn lines(&self) -> Vec<String> {
        self.s.app.doc.as_ref().map(|d| d.blocks().iter().map(|l| l.text.clone()).collect()).unwrap_or_default()
    }

    /// The line holding `s` sits at `depth`.
    pub fn expect_depth(&mut self, s: &str, depth: usize) -> &mut Self {
        let d = self.s.app.doc.as_ref().unwrap_or_else(|| self.expect_fail("no document open"));
        match d.blocks().iter().find(|l| l.text.contains(s)) {
            Some(l) if l.depth == depth => self,
            Some(l) => {
                let got = l.depth;
                self.expect_fail(&format!("{s:?} is at depth {got}, want {depth}"))
            }
            None => self.expect_fail(&format!("no line holds {s:?}")),
        }
    }

    /// The selection's text (lines joined with `\n`).
    pub fn selection(&mut self) -> Option<String> {
        let d = self.s.app.doc.as_mut()?;
        let (a, b) = d.selection()?;
        let lines: Vec<String> = d.blocks().iter().map(|l| l.text.clone()).collect();
        let (a, b) = if (a.line, a.byte) <= (b.line, b.byte) { (a, b) } else { (b, a) };
        if a.line == b.line {
            return Some(lines[a.line][a.byte..b.byte].to_string());
        }
        let mut out = lines[a.line][a.byte..].to_string();
        for l in &lines[a.line + 1..b.line] {
            out.push('\n');
            out.push_str(l);
        }
        out.push('\n');
        out.push_str(&lines[b.line][..b.byte]);
        Some(out)
    }

    pub fn expect_selection(&mut self, s: &str) -> &mut Self {
        let got = self.selection();
        if got.as_deref() != Some(s) {
            self.expect_fail(&format!("the selection is {got:?}, want {s:?}"));
        }
        self
    }

    pub fn expect_no_selection(&mut self) -> &mut Self {
        let got = self.selection();
        if got.as_deref().is_some_and(|s| !s.is_empty()) {
            self.expect_fail(&format!("a selection: {got:?}"));
        }
        self
    }

    pub fn expect_focus(&mut self, f: Focus) -> &mut Self {
        if self.s.app.ui.focus != f {
            self.expect_fail(&format!("focus is {:?}, want {f:?}", self.s.app.ui.focus));
        }
        self
    }

    pub fn expect_panels(&mut self, n: usize) -> &mut Self {
        let got = self.s.app.ui.sidebar.open.len();
        if got != n {
            self.expect_fail(&format!("{got} sidebar panels, want {n}"));
        }
        self
    }

    /// The vault holds a note whose text is `s` (after a save).
    pub fn expect_saved(&mut self, s: &str) -> &mut Self {
        self.save();
        let found = self.s.app.vault.store.nodes_where("1=1", &[]).unwrap_or_default().iter().any(|n| n.text == s);
        if !found {
            self.expect_fail(&format!("no saved note reads {s:?}"));
        }
        self
    }

    /// The vault holds a note whose text contains `s` (after a save).
    pub fn expect_saved_contains(&mut self, s: &str) -> &mut Self {
        self.save();
        let found = self.s.app.vault.store.nodes_where("1=1", &[]).unwrap_or_default().iter().any(|n| n.text.contains(s));
        if !found {
            self.expect_fail(&format!("no saved note holds {s:?}"));
        }
        self
    }

    /// The saved note whose text holds `s` has status `status` (after a save).
    pub fn expect_saved_status(&mut self, s: &str, status: &str) -> &mut Self {
        self.save();
        let nodes = self.s.app.vault.store.nodes_where("1=1", &[]).unwrap_or_default();
        match nodes.iter().find(|n| n.text.contains(s)) {
            Some(n) if n.status.as_deref() == Some(status) => self,
            Some(n) => {
                let got = n.status.clone();
                self.expect_fail(&format!("{s:?} is {got:?}, want {status}"))
            }
            None => self.expect_fail(&format!("no saved note holds {s:?}")),
        }
    }

    /// Something for the reader of a failing flow.
    pub fn dump(&mut self) -> &mut Self {
        eprintln!("--- flow `{}` step {} ---\n{}", self.name, self.step, numbered(&self.shot.text));
        self
    }
}

impl Drop for Flow {
    fn drop(&mut self) {
        if !self.done && !std::thread::panicking() {
            panic!("flow `{}` ended without .done()", self.name);
        }
    }
}

/// Draw a frame into the long-lived terminal, as the live loop does, and read what it shows.
fn draw(term: &mut Terminal<Emu>, s: &mut Session) -> Shot {
    s.sync_document();
    let buf = term.draw(|f| crate::ui::draw_app(f, &mut s.app)).unwrap().buffer.clone();
    let emu = term.backend();
    let cursor = emu.visible.then_some((emu.cursor.x, emu.cursor.y)).filter(|(x, y)| *x < buf.area.width && *y < buf.area.height);
    let app = &s.app;
    let doc_view = app.render.doc_view.map(|(x, y, w, h)| Rect::new(x, y, w, h));
    let side_x = app.sidebar_col.map(|c| buf.area.width.saturating_sub(c + 1)).or(app.sidebar_over.map(|(_, r)| r.x));
    let panels = app.render.panel_views.iter().map(|(_, r)| *r).collect();
    let overlay = app.ui.overlay.is_some() || app.prompt.is_some() || app.ui.link_open;
    let caret = app.doc.as_ref().map(|d| (d.caret().line, d.caret().byte));
    let writing = app.doc.is_some() && doc_view.is_some() && app.ui.focus == Focus::List && !overlay && !app.ui.doc_parked && cursor.is_some_and(|(x, y)| doc_view.is_some_and(|r| r.contains((x, y).into())));
    let caret_top = caret.and_then(|(line, _)| {
        let rows: Vec<&crate::doc_ui::HitRow> = app.render.doc_hits.iter().filter(|h| h.line == line).collect();
        rows.iter().find(|h| h.first).map(|h| h.y).or_else(|| rows.first().map(|_| doc_view.map_or(0, |r| r.y)))
    });
    let text = crate::session::frame_text(&buf).lines().map(str::to_string).collect();
    Shot { caret_top, buf, text, cursor, doc_view, side_x, panels, scroll: app.doc.as_ref().map(|d| d.scroll()), caret, writing, overlay }
}

/// A key script's tokens, as written.
fn tokens(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = script;
    while !rest.is_empty() {
        if rest.starts_with('<') {
            let end = if rest.starts_with("<paste:") { rest.rfind('>') } else { rest.find('>') };
            if let Some(e) = end {
                out.push(rest[..=e].to_string());
                rest = &rest[e + 1..];
                continue;
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c.to_string());
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// A frame with the left rail (up to its `│`, when the frame has one) blanked.
fn without_rail(frame: &str) -> String {
    frame
        .lines()
        .map(|l| match l.char_indices().find(|(_, c)| *c == '│').filter(|(i, _)| l[..*i].chars().count() < 30) {
            Some((i, _)) => format!("{}{}", " ".repeat(l[..i].chars().count()), &l[i..]),
            None => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The footer's status (up to its first wide gap), counts masked.
fn footer_status(row: &str) -> String {
    let m = counts_masked(row);
    m.trim_start().split("  ").next().unwrap_or("").to_string()
}

/// A footer row with each run of digits as `#` and each run of spaces as two: a count that
/// grows a digit doesn't count as the footer moving.
fn counts_masked(row: &str) -> String {
    let mut out = String::new();
    let mut prev = ' ';
    let mut spaces = 0;
    for c in row.chars() {
        if c == ' ' {
            spaces += 1;
            if spaces <= 2 {
                out.push(' ');
            }
        } else {
            spaces = 0;
            if c.is_ascii_digit() {
                if !prev.is_ascii_digit() {
                    out.push('#');
                }
            } else {
                out.push(c);
            }
        }
        prev = c;
    }
    out.replace("# words", "# word")
}

/// Two frames the same but for ids a write made fresh (a node's short id, a tx's): a replay
/// makes its own writes again, with new ids. Only a token that is id-shaped on both sides is
/// forgiven.
fn same_but_ids(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let id = |t: &str| (t.len() == 5 && t.chars().all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())) || (t.len() == 6 && t.chars().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()));
    let (la, lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    la.len() == lb.len()
        && la.iter().zip(&lb).all(|(x, y)| {
            if x == y {
                return true;
            }
            let (tx, ty): (Vec<&str>, Vec<&str>) = (x.split(' ').collect(), y.split(' ').collect());
            tx.len() == ty.len() && tx.iter().zip(&ty).all(|(p, q)| p == q || (id(p.trim_matches('"')) && id(q.trim_matches('"'))))
        })
}

fn numbered(lines: &[String]) -> String {
    lines.iter().enumerate().map(|(i, l)| format!("{i:>2}│{l}")).collect::<Vec<_>>().join("\n")
}

fn numbered_str(s: &str) -> String {
    numbered(&s.lines().map(str::to_string).collect::<Vec<_>>())
}

fn json_diff(a: &Value, b: &Value) -> String {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            keys.into_iter().filter(|k| x.get(*k) != y.get(*k)).map(|k| format!("{k}: {:?} → {:?}", x.get(k), y.get(k))).collect::<Vec<_>>().join("; ")
        }
        _ => format!("{a} → {b}"),
    }
}

/// `thc ui replay` in a test: each segment on a scratch copy of the vault as of where it starts.
fn replay(paths: &thc_core::vault::Paths, trace: &str, want: &std::collections::HashSet<usize>) -> Vec<(usize, String)> {
    let mut copies = Vec::new();
    let mut open = |at: Option<&thc_core::vault::Frontier>| -> Result<Session, String> {
        let p = thc_core::vault::scratch_copy_at(paths, at).map_err(|e| format!("{e:#}"))?;
        copies.push(p.vault.parent().unwrap().to_path_buf());
        let mut v = thc_core::vault::Vault::open(p, thc_core::event::Actor { kind: "human".into(), name: None }, "tui").map_err(|e| format!("{e:#}"))?;
        v.origin = Some(paths.clone());
        let mut app = crate::app::App::new(v).map_err(|e| format!("{e:#}"))?;
        app.daemon_live = false;
        Ok(Session::new(app, (80, 24)))
    };
    let frames = crate::session::replay_where(&mut open, trace, None, "text", &|i| want.contains(&i)).unwrap();
    for c in copies {
        let _ = std::fs::remove_dir_all(c);
    }
    frames
}
