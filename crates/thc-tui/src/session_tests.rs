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
    let base = copy(&vault);
    let mut live = session(vault, (110, 32));
    for m in crate::script::parse(TOUR, false).unwrap() {
        live.apply(m).unwrap();
        live.drop_effects();
    }
    let expected = frame(&mut live);
    let (_, lines) = live.trace(None, true).unwrap();
    let trace: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let mut runs = Vec::new();
    for _ in 0..2 {
        let mut s = session(copy(&base), (80, 24));
        runs.push(crate::session::replay(&mut s, &trace, None, "text", true).unwrap());
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
    let base = copy(&vault);
    let mut live = session(vault, (100, 30));
    for m in crate::script::parse("5hello world<cr>a second line<up><end> again", false).unwrap() {
        live.apply(m).unwrap();
        live.drop_effects();
    }
    assert!(live.state().document.as_ref().is_some_and(|d| d.target.is_some()), "a journal day is open");
    let (_, lines) = live.trace(None, true).unwrap();
    let trace: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let runs: Vec<Vec<String>> = (0..2)
        .map(|_| {
            let mut s = session(copy(&base), (100, 30));
            crate::session::replay(&mut s, &trace, None, "text", true).unwrap()
        })
        .collect();
    assert_eq!(runs[0], runs[1]);
    assert!(runs[0].last().unwrap().contains("hello world again"), "{}", runs[0].last().unwrap());
}

#[test]
fn changes_outside_messages_are_recorded_and_replay() {
    let (_s, vault) = seeded("external");
    let base = copy(&vault);
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
    let mut s = session(copy(&base), (100, 30));
    let frames = crate::session::replay(&mut s, &trace, None, "text", false).unwrap();
    assert_eq!(frames.last().unwrap(), &expected);
    assert!(expected.contains("an alert from elsewhere"));
    assert_eq!(s.state(), live.state());
}
