//! The Elm-shaped TUI, end to end: messages replay to the same frames, pushed states change
//! what's drawn, random message sequences never panic, and the view reads no store, clock,
//! environment or file.

use crate::session::{Msg, Session};
use serde_json::json;
use thc_core::builder::TxBuilder;
use thc_core::vault::{Paths, Vault};

/// A small vault: tasks, notes, two pages, a journal day.
fn seeded(tag: &str) -> (crate::fuzz::Scratch, Vault) {
    let (s, mut vault) = crate::fuzz::scratch(tag);
    let today = thc_core::dates::today();
    for (i, text) in [
        "[ ] write the plan due:today #work",
        "[ ] call the bank due:+2d !high",
        "[ ] read [[Lisbon flat]] notes",
        "a thought about [[Garden]]",
        "[ ] water the plants #home",
        "[x] done already",
    ]
    .iter()
    .enumerate()
    {
        vault
            .transact(|st| {
                let mut b = TxBuilder::new(st, today);
                let parent = if i % 2 == 0 { None } else { Some(b.journal(today)?) };
                b.create_from_capture(parent, &thc_core::capture::parse(text, today).unwrap(), None)?;
                Ok((b.finish(), ()))
            })
            .unwrap();
    }
    (s, vault)
}

/// A scratch copy that knows the vault it came from, as `thc ui replay` opens one.
fn copy(vault: &Vault) -> Vault {
    let paths = thc_core::vault::scratch_copy(&vault.paths).unwrap();
    let mut v = Vault::open(Paths { vault: paths.vault, cache: paths.cache }, vault.actor.clone(), "tui").unwrap();
    v.origin = Some(vault.origin.clone().unwrap_or_else(|| vault.paths.clone()));
    v
}

/// `thc ui replay` in a test: each segment on a scratch copy of the vault at `paths` as of
/// where it starts (its `state` line's `log`), removed afterwards.
fn replay(paths: &Paths, trace: &str, size: Option<(u16, u16)>, every: bool) -> Vec<String> {
    let mut copies = Vec::new();
    let mut open = |at: Option<&thc_core::vault::Frontier>| -> Result<Session, String> {
        let p = thc_core::vault::scratch_copy_at(paths, at).map_err(|e| format!("{e:#}"))?;
        copies.push(p.vault.parent().unwrap().to_path_buf());
        let mut v = Vault::open(p, thc_core::event::Actor { kind: "human".into(), name: None }, "tui").map_err(|e| format!("{e:#}"))?;
        v.origin = Some(paths.clone());
        Ok(session(v, (80, 24)))
    };
    let frames = crate::session::replay(&mut open, trace, size, "text", every).unwrap();
    for c in copies {
        let _ = std::fs::remove_dir_all(c);
    }
    frames
}

fn trace_text(s: &Session, all: bool) -> String {
    let (_, lines) = s.trace(None, all).unwrap();
    lines.iter().map(|l| format!("{l}\n")).collect()
}

fn session(vault: Vault, size: (u16, u16)) -> Session {
    crate::SNAPSHOT.with(|s| s.set(true));
    let mut app = crate::app::App::new(vault).unwrap();
    app.daemon_live = false;
    Session::new(app, size)
}

fn frame(s: &mut Session) -> String {
    let (w, h) = s.size;
    s.render(w, h, "text").unwrap().frame.unwrap()
}

/// Input that only moves around: views, lists, filters, overlays, the palette.
const TOUR: &str = "3jj<tab><tab>k/plan<cr><esc>2:<esc>?<esc>4j<s-tab>gg";

#[test]
fn a_trace_replays_to_the_same_frames_every_time() {
    let (_s, vault) = seeded("replay");
    let mut live = session(vault, (110, 32));
    for m in crate::script::parse(TOUR, false).unwrap() {
        live.apply(m).unwrap();
        live.drop_effects();
    }
    let expected = frame(&mut live);
    let (_, lines) = live.trace(None, true).unwrap();
    let trace = trace_text(&live, true);
    let mut runs = Vec::new();
    for _ in 0..2 {
        runs.push(replay(&live.app.vault.paths, &trace, None, true));
    }
    assert_eq!(runs[0], runs[1], "two replays differ");
    assert_eq!(runs[0].last().unwrap(), &expected, "the replay's last frame isn't the session's");
    // A frame for the state line and one per message.
    assert_eq!(runs[0].len(), lines.len());
}

#[test]
fn a_state_round_trips_through_the_session_and_draws_the_same() {
    let (_s, vault) = seeded("roundtrip");
    let mut a = session(copy(&vault), (100, 30));
    for m in crate::script::parse("3<tab>j", false).unwrap() {
        a.apply(m).unwrap();
    }
    let state = a.state().to_json();
    let text = serde_json::to_string(&state).unwrap();
    let back: crate::ui_state::UiState = serde_json::from_str(&text).unwrap();
    assert_eq!(&back, a.state());
    let mut b = session(copy(&vault), (100, 30));
    b.restore(&state).unwrap();
    assert_eq!(b.state().to_json(), state);
    assert_eq!(frame(&mut b), frame(&mut a));
}

#[test]
fn a_pushed_state_changes_the_frame_steps_history_and_says_who() {
    let (_s, vault) = seeded("push");
    let mut s = session(vault, (110, 32));
    let before = frame(&mut s);
    let rev = s.rev;
    let steps = s.state().history.entries.len();
    s.apply(Msg::Patch { patch: json!({"view": "tasks", "tasks_filter": "status:open #work"}), actor: Some("claude".into()) }).unwrap();
    let after = frame(&mut s);
    assert_ne!(before, after);
    assert!(after.contains("write the plan") && !after.contains("water the plants"), "{after}");
    assert!(after.contains("claude changed your view"), "{after}");
    assert_eq!(s.rev, rev + 1);
    assert!(s.state().history.entries.len() > steps, "an external change is a history step");
    // The same patch again changes nothing.
    let rev = s.rev;
    s.apply(Msg::Patch { patch: json!({"view": "tasks"}), actor: Some("claude".into()) }).unwrap();
    assert_eq!(s.state().view, crate::app::View::Tasks);
    assert_eq!(s.rev, rev + 1, "a message is still a message");
    // A refused patch changes nothing, not even the rev.
    let rev = s.rev;
    assert!(s.apply(Msg::Patch { patch: json!({"veiw": "log"}), actor: None }).unwrap_err().contains("did you mean `view`"));
    assert!(s.apply(Msg::Patch { patch: json!({"vault_name": "elsewhere"}), actor: None }).is_err());
    assert_eq!(s.rev, rev);
}

#[test]
fn rendering_at_another_size_leaves_the_state_alone() {
    let (_s, vault) = seeded("render");
    let mut s = session(vault, (100, 30));
    for m in crate::script::parse("3jjjj", false).unwrap() {
        s.apply(m).unwrap();
    }
    let state = s.state().clone();
    let small = s.render(60, 24, "text").unwrap().frame.unwrap();
    let big = s.render(160, 50, "cells").unwrap().rows.unwrap();
    assert_eq!(small.lines().count(), 24);
    assert_eq!(big.as_array().unwrap().len(), 50);
    assert_eq!(s.state(), &state);
    assert_eq!(s.render(100, 30, "text").unwrap().frame, s.render(100, 30, "text").unwrap().frame);
}

#[test]
fn random_messages_on_every_screen_never_panic() {
    let (_s, vault) = seeded("random");
    let mut s = session(vault, (100, 30));
    // A tiny deterministic generator: the same sequence every run.
    let mut seed: u64 = 0x2545F4914F6CDD1D;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let keys = ["j", "k", "<cr>", "<esc>", "<tab>", "<s-tab>", "1", "2", "3", "4", "5", "6", "7", "/", "?", ":", "x", "a", " ", "<bs>", "<up>", "<down>", "<left>", "<right>", "<c-o>", "<pgdn>", "<pgup>", "g", "G", "z", "<m-z>", "<c-z>", "e", "o", "<space>"];
    let views = ["today", "inbox", "tasks", "pages", "journal", "search", "log"];
    for step in 0..1500u64 {
        let r = next();
        let msg = match r % 20 {
            0 => Msg::Resize { w: 40 + (next() % 140) as u16, h: 10 + (next() % 50) as u16 },
            1 => Msg::Patch { patch: json!({"view": views[(next() % views.len() as u64) as usize], "selected": null}), actor: None },
            2 => Msg::Tick { now_ms: s.app.ui.now_ms + next() % 5000, utc_offset_min: 0 },
            3 => Msg::Mouse {
                mouse: crate::session::Mouse {
                    kind: [crate::session::MouseKind::Down, crate::session::MouseKind::Up, crate::session::MouseKind::ScrollDown, crate::session::MouseKind::Moved][(next() % 4) as usize],
                    x: (next() % 120) as u16,
                    y: (next() % 40) as u16,
                    mods: String::new(),
                    clicks: None,
                },
            },
            4 => Msg::Paste { text: "pasted text".into() },
            _ => Msg::Key { key: keys[(next() % keys.len() as u64) as usize].to_string() },
        };
        if let Err(e) = s.apply(msg.clone()) {
            panic!("step {step}: {msg:?}: {e}");
        }
        // The runtime never performs a headless session's effects.
        s.drop_effects();
        s.app.reexec = false;
        if step % 50 == 0 {
            let (w, h) = s.size;
            s.render(w, h, "text").unwrap();
        }
    }
}

/// The view (ui.rs above its runtime section, doc_ui.rs, node_row.rs) reads no store, clock,
/// environment, file or process, and calls none of App's store-reading helpers.
#[test]
fn the_view_reads_no_store_clock_environment_or_file() {
    let forbidden = [
        ".store", "Instant::now", "Local::now", "SystemTime", "std::env", "env::var", "std::fs", "File::open", "std::process", "dates::today", "now_local",
        "Registry::load", "settings::current", "settings::load", ".reload(", ".link_matches(", ".finder_matches(", ".move_items(", ".recipe(",
        ".palette_entries(", ".node_label(", "about::releases(",
    ];
    let view_part = |src: &str, end_marker: &str| -> String {
        let end = src.find(end_marker).unwrap_or(src.len());
        let src = &src[..end];
        // Strip line comments.
        src.lines().map(|l| l.split("//").next().unwrap_or("")).collect::<Vec<_>>().join("\n")
    };
    let files = [
        ("ui.rs", view_part(include_str!("ui.rs"), "// ---- runtime: frame preparation")),
        ("doc_ui.rs", view_part(include_str!("doc_ui.rs"), "#[cfg(test)]")),
        ("editor_pane/view.rs", view_part(include_str!("editor_pane/view.rs"), "#[cfg(test)]")),
        ("node_row.rs", view_part(include_str!("node_row.rs"), "#[cfg(test)]")),
    ];
    let mut found = Vec::new();
    for (name, src) in &files {
        for (i, line) in src.lines().enumerate() {
            for f in forbidden {
                if line.contains(f) {
                    found.push(format!("{name}:{}: {f}: {}", i + 1, line.trim()));
                }
            }
        }
    }
    assert!(found.is_empty(), "the view reaches outside its inputs:\n{}", found.join("\n"));
}

#[test]
fn typing_in_a_document_replays_the_same_on_two_copies() {
    let (_s, vault) = seeded("replay-doc");
    let mut live = session(vault, (100, 30));
    // As the terminal loop does: the frame after each key (the save of a line just left).
    for m in crate::script::parse("5hello world<cr>a second line<up><end> again", false).unwrap() {
        live.apply(m).unwrap();
        live.drop_effects();
        live.runtime(Msg::Frame);
    }
    assert!(live.state().document.as_ref().is_some_and(|d| d.target.is_some()), "a journal day is open");
    let trace = trace_text(&live, true);
    // Replayed on the vault as it is now, which already has the session's saves: the trace's
    // `state` line pins where it started, so they land once.
    let expected = frame(&mut live);
    let runs: Vec<Vec<String>> = (0..2).map(|_| replay(&live.app.vault.paths, &trace, None, true)).collect();
    assert_eq!(runs[0], runs[1]);
    let last = runs[0].last().unwrap();
    assert_eq!(last, &expected, "the replay's last frame is the session's");
    assert_eq!(last.matches("hello world").count(), 1, "{last}");
}

/// The bug a replay doubled every edit with: it ran on the vault as it is now. A trace pins the
/// vault it started on; what others wrote meanwhile rides along (`_log`) and lands once, where
/// it landed live; the runtime's own saves and polls are messages; a checkpoint starts over.
#[test]
fn a_trace_pins_its_vault_and_others_writes_land_once() {
    let (_s, vault) = seeded("replay-pin");
    let paths = vault.paths.clone();
    let mut live = session(vault, (100, 30));
    let step = |live: &mut Session, keys: &str| {
        for m in crate::script::parse(keys, false).unwrap() {
            live.apply(m).unwrap();
            live.drop_effects();
            live.runtime(Msg::Frame);
        }
    };
    step(&mut live, "5coffee notes<cr><tab>nested line");
    // Time passes with nothing typed: the idle point (one undo step, the idle save).
    let now = live.app.ui.now_ms + 2000;
    live.apply(Msg::Tick { now_ms: now, utc_offset_min: 0 }).unwrap();
    live.runtime(Msg::Idle);
    // An agent writes through the same store (`thc add` beside the TUI), then the poll.
    let mut agent = Vault::open(paths.clone(), thc_core::event::Actor { kind: "agent".into(), name: Some("claude".into()) }, "cli").unwrap();
    let today = thc_core::dates::today();
    agent
        .transact(|st| {
            let mut b = TxBuilder::new(st, today);
            let j = b.journal(today)?;
            b.create_from_capture(Some(j), &thc_core::capture::parse("from the agent", today).unwrap(), None)?;
            Ok((b.finish(), ()))
        })
        .unwrap();
    live.runtime(Msg::Poll);
    live.sync_external();
    step(&mut live, "<cr>after the agent");
    live.apply(Msg::Patch { patch: json!({"view": "tasks"}), actor: Some("claude".into()) }).unwrap();
    live.apply(Msg::Patch { patch: json!({"view": "journal"}), actor: Some("claude".into()) }).unwrap();
    live.sync_external();

    let trace = trace_text(&live, true);
    let first: serde_json::Value = serde_json::from_str(trace.lines().next().unwrap()).unwrap();
    assert!(first["log"].is_object(), "the state line pins the vault: {first}");
    let carried: Vec<serde_json::Value> = trace.lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()).filter(|v| v.get("_log").is_some()).collect();
    assert_eq!(carried.len(), 1, "the agent's write rides along once:\n{trace}");
    assert!(carried[0]["_log"].as_array().unwrap().iter().all(|e| e["via"] == "cli"), "only other writers' lines: {}", carried[0]);
    for m in ["idle", "poll", "frame"] {
        assert!(trace.contains(&format!("{{\"msg\":\"{m}\"")), "no {m} in the trace:\n{trace}");
    }
    let expected = frame(&mut live);
    let got = replay(&paths, &trace, None, false);
    assert_eq!(got.last().unwrap(), &expected, "the replay's frame is the live one");
    for text in ["coffee notes", "nested line", "from the agent", "after the agent"] {
        assert_eq!(expected.matches(text).count(), 1, "{text} once:\n{expected}");
    }
    // At another size: the frame the live session draws at that size.
    let small = live.render(70, 24, "text").unwrap().frame.unwrap();
    assert_eq!(replay(&paths, &trace, Some((70, 24)), false).last().unwrap(), &small);

    // A checkpoint: the segment after it replays on its own, on the vault as of then.
    live.checkpoint_saved();
    step(&mut live, "<cr>past the checkpoint");
    let segment = trace_text(&live, false);
    assert!(segment.lines().next().unwrap().starts_with("{\"state\""), "{segment}");
    let expected = frame(&mut live);
    assert_eq!(replay(&paths, &segment, None, false).last().unwrap(), &expected);
    // And the whole trace, segment by segment.
    assert_eq!(replay(&paths, &trace_text(&live, true), None, false).last().unwrap(), &expected);
}

#[test]
fn changes_outside_messages_are_recorded_and_replay() {
    let (_s, vault) = seeded("external");
    let mut live = session(vault, (100, 30));
    live.apply(Msg::Key { key: "3".into() }).unwrap();
    // Something the runtime did between frames: an alert toast and a flash, as the daemon's
    // push would leave them.
    live.app.info("an alert from elsewhere");
    live.app.ui.flashes.insert("x1".into(), (live.app.ui.now_ms, true));
    let rev = live.rev;
    live.sync_external();
    assert_eq!(live.rev, rev + 1);
    live.sync_external();
    assert_eq!(live.rev, rev + 1, "nothing new, no message");
    let (_, lines) = live.trace(None, true).unwrap();
    assert!(lines.last().unwrap()["msg"] == "external", "{:?}", lines.last());
    let expected = frame(&mut live);
    let trace: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let frames = replay(&live.app.vault.paths, &trace, None, false);
    assert_eq!(frames.last().unwrap(), &expected);
    assert!(expected.contains("an alert from elsewhere"));
}
