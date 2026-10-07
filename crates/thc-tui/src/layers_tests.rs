//! Layers end to end: renderer goldens (every renderer at three sizes, in ember dark and light,
//! 16 colours and ASCII), anchors surviving scroll, resize and edits, the person's controls
//! over agents' layers, the policy, and replay.
//!
//! Goldens live in tests/goldens/layers/; `THC_UPDATE_GOLDENS=1 cargo test -p thc-tui layers_`
//! rewrites them.

use crate::session::{Msg, Session};
use serde_json::{Value, json};
use thc_core::builder::TxBuilder;
use thc_core::vault::{Paths, Vault};

/// A scratch folder, removed when dropped.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn day() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 10, 7).unwrap()
}

/// 10:00 UTC on Wed 7 Oct 2026.
const NOW: u64 = 1_791_367_200_000;

fn id(k: &str) -> String {
    thc_core::id::from_key(&format!("layers-test:{k}"))
}

/// A page with a paragraph and three tasks, three tasks of its own, and `extra` more notes
/// on the page (to scroll). Ids are keyed, so frames are the same every run.
fn fixture(tag: &str, extra: usize) -> (Scratch, Vault) {
    // Named `acme` whatever the scratch folder: frames show the vault's name.
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("thc-layers-{}-{n}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("acme");
    thc_core::vault::init(&dir, None, None).unwrap();
    let mut vault = Vault::open(Paths { vault: dir, cache: root.join("cache") }, thc_core::event::Actor { kind: "human".into(), name: None }, "tui").unwrap();
    let s = Scratch(root);
    let today = day();
    vault
        .transact(|st| {
            let mut b = TxBuilder::new(st, today);
            let page = b.create_page("Launch plan", &[])?;
            let cap = |t: &str| thc_core::capture::parse(t, today).unwrap();
            b.create_from_capture(Some(page.clone()), &cap("Ship the beta to the first 50 teams by the end of the month."), Some(id("goal")))?;
            b.create_from_capture(Some(page.clone()), &cap("[ ] Finish the sign-up flow due:2026-10-09 !high"), Some(id("signup")))?;
            b.create_from_capture(Some(page.clone()), &cap("[ ] Write the getting-started guide due:2026-10-12"), Some(id("docs")))?;
            b.create_from_capture(Some(page.clone()), &cap("[ ] Send the invites due:2026-10-16"), Some(id("invites")))?;
            for i in 0..extra {
                b.create_from_capture(Some(page.clone()), &cap(&format!("A line to scroll past, number {i}")), Some(id(&format!("x{i}"))))?;
            }
            b.create_from_capture(None, &cap("[ ] Buy groceries due:2026-10-08"), Some(id("groceries")))?;
            b.create_from_capture(None, &cap("[ ] Book the dentist due:2026-10-13"), Some(id("dentist")))?;
            b.create_from_capture(None, &cap("[ ] Renew the passport"), Some(id("passport")))?;
            Ok((b.finish(), ()))
        })
        .unwrap();
    (s, vault)
}

fn page_id(s: &Session) -> String {
    let _ = s;
    thc_core::id::from_key("page:launch plan")
}

fn theme(name: &str) -> crate::theme::Theme {
    let (mode, ascii) = match name {
        "dark" => (json!({"truecolor": {"dark": true}}), false),
        "light" => (json!({"truecolor": {"dark": false}}), false),
        "ansi16" => (json!({"ansi": {"no_color": false}}), false),
        _ => (json!({"truecolor": {"dark": true}}), true),
    };
    serde_json::from_value(json!({"mode": mode, "palette": "ember", "ascii": ascii, "no_dim": false, "accent": "ember"})).unwrap()
}

fn session(vault: Vault, size: (u16, u16), th: &str) -> Session {
    crate::SNAPSHOT.with(|s| s.set(true));
    let mut app = crate::app::App::new(vault).unwrap();
    app.daemon_live = false;
    app.theme = theme(th);
    app.layer_limits = crate::layers::AgentLimits::Off;
    let mut s = Session::new(app, size);
    s.apply(Msg::Tick { now_ms: NOW, utc_offset_min: 0 }).unwrap();
    s
}

fn layer(s: &mut Session, req: Value, actor: Option<&str>) {
    s.apply(Msg::Layer { req, actor: actor.map(str::to_string) }).unwrap();
}

fn tasks(s: &mut Session) {
    s.apply(Msg::Patch { patch: json!({"view": "tasks", "tasks_filter": "status:open sort:due"}), actor: None }).unwrap();
}

fn open_page(s: &mut Session) {
    let p = page_id(s);
    s.apply(Msg::Patch { patch: json!({"view": "pages", "page_open": p}), actor: None }).unwrap();
}

/// Each renderer's scene.
fn scene(s: &mut Session, name: &str) {
    match name {
        "hint" => {
            tasks(s);
            layer(s, json!({"op": "hint.show", "anchor": format!("row:{}", id("signup")), "title": "Start here", "text": "Due Friday, and the invites wait on it. Two screens left: plan and payment."}), Some("claude"));
        }
        "highlight" => {
            tasks(s);
            layer(s, json!({"op": "highlight", "anchor": format!("row:{}", id("docs"))}), Some("claude"));
            layer(s, json!({"op": "highlight", "anchor": "ui:tab:pages"}), None);
        }
        "focus" => {
            tasks(s);
            layer(s, json!({"op": "focus", "anchor": format!("row:{}", id("invites")), "title": "Next week", "text": "These go out once sign-up works."}), Some("claude"));
        }
        "tour" => {
            open_page(s);
            let steps = json!([
                {"anchor": format!("row:{}", id("signup")), "title": "The blocker", "text": "Finish the sign-up flow first."},
                {"anchor": format!("row:{}", id("goal")), "title": "Why", "text": "The goal for the month, in one line."},
                {"anchor": "ui:tab:tasks", "title": "Everything else", "text": "Tasks has the rest, by due date."}
            ]);
            layer(s, json!({"op": "tour.start", "steps": steps, "spotlight": true}), Some("claude"));
            layer(s, json!({"op": "tour.next"}), Some("claude"));
        }
        other => panic!("no scene {other}"),
    }
}

fn golden(name: &str, got: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/layers").join(name);
    if std::env::var_os("THC_UPDATE_GOLDENS").is_some() || !path.exists() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap();
    assert!(want == got, "{name} differs from its golden (THC_UPDATE_GOLDENS=1 rewrites it)\n--- got:\n{got}");
}

#[test]
fn layers_renderer_goldens() {
    for scn in ["hint", "highlight", "focus", "tour"] {
        for (w, h) in [(140u16, 40u16), (100, 30), (80, 24)] {
            for th in ["dark", "light", "ansi16", "ascii"] {
                let (_x, vault) = fixture(&format!("golden-{scn}-{w}-{th}"), 0);
                let mut s = session(vault, (w, h), th);
                // The detail pane shows when notes were made (the wall clock): not in goldens.
                s.apply(Msg::Patch { patch: json!({"show_detail": false}), actor: None }).unwrap();
                scene(&mut s, scn);
                let (format, ext) = if th == "ascii" { ("text", "txt") } else { ("ansi", "ansi") };
                let frame = s.render(w, h, format).unwrap().frame.unwrap();
                // Every scene places its layer.
                let plan = s.app.render.layer_plan.clone().expect("a plan");
                assert!(plan.missing.is_empty(), "{scn} {w}x{h}: anchors not found: {:?}", plan.missing);
                golden(&format!("{scn}.{w}x{h}.{th}.{ext}"), &frame);
            }
        }
    }
}

#[test]
fn layers_draw_what_the_design_says() {
    let (_x, vault) = fixture("design", 0);
    let mut s = session(vault, (100, 30), "dark");
    scene(&mut s, "hint");
    let f = s.render(100, 30, "text").unwrap().frame.unwrap();
    // The box, its attribution on the top edge, the title and the text.
    assert!(f.contains("◆ claude"), "{f}");
    assert!(f.contains("Start here") && f.contains("invites wait on it"), "{f}");
    assert!(f.contains('╭') && f.contains('╯'), "{f}");
    // ASCII: no box-drawing.
    let (_y, vault) = fixture("design-ascii", 0);
    let mut s = session(vault, (100, 30), "ascii");
    scene(&mut s, "hint");
    let f = s.render(100, 30, "text").unwrap().frame.unwrap();
    assert!(f.contains("@ claude") && !f.contains('╭') && !f.contains('◆'), "{f}");
    // A spotlight dims everything outside its holes: the tab row is dimmed, the row isn't.
    let (_z, vault) = fixture("design-focus", 0);
    let mut s = session(vault, (100, 30), "dark");
    scene(&mut s, "focus");
    s.render(100, 30, "text").unwrap();
    let p = s.app.render.layer_plan.clone().unwrap();
    let anchor = p.layers[0].anchor.clone().unwrap().rects[0];
    assert!(p.dimmed(1, 0), "the header is dimmed");
    assert!(!p.dimmed(anchor.x, anchor.y), "the anchor is lit");
    // The walkthrough's controls: dots, back, next and stop are regions.
    let (_t, vault) = fixture("design-tour", 0);
    let mut s = session(vault, (100, 30), "dark");
    scene(&mut s, "tour");
    let f = s.render(100, 30, "text").unwrap().frame.unwrap();
    assert!(f.contains("●●○") && f.contains("‹ back") && f.contains("next ›"), "{f}");
    let ids: Vec<String> = s.app.render.layer_plan.as_ref().unwrap().regions.iter().map(|r| r.id.clone()).collect();
    for want in ["s2/0/back", "s2/0/next", "s2/0/stop"] {
        assert!(ids.iter().any(|i| i == want), "{want} in {ids:?}");
    }
}

/// Where layer `id` resolved this frame (rects or off).
fn resolved(s: &mut Session, layer: &str) -> caretline_layers::Resolved {
    let (w, h) = s.size;
    s.render(w, h, "text").unwrap();
    s.app.render.layer_plan.as_ref().and_then(|p| p.layers.iter().find(|l| l.id == layer)).and_then(|l| l.anchor.clone()).expect("resolved")
}

/// The text of the cells a rect covers.
fn cells(s: &Session, r: &caretline_layers::Rect) -> String {
    let buf = s.app.render.cells.as_ref().unwrap();
    (r.x..r.right()).map(|x| buf[(x, r.y)].symbol().to_string()).collect()
}

#[test]
fn layers_anchors_survive_scroll_resize_and_edits() {
    let (_x, vault) = fixture("survive", 60);
    let mut s = session(vault, (100, 30), "dark");
    open_page(&mut s);
    // A text anchor on a line far down the page: the agent's view gives its chars.
    let line = id("x50");
    s.apply(Msg::DocView { req: json!({"op": "doc.view.open", "at": {"id": line}}), actor: Some("claude".into()) }).unwrap();
    let r = s.app.doc_view_reply.take().unwrap();
    let (from, _to) = (r["line"]["text"]["from"].as_u64().unwrap() as usize, r["line"]["text"]["to"].as_u64().unwrap() as usize);
    layer(&mut s, json!({"op": "highlight", "anchor": [{"text": {"from": from + 2, "to": from + 6}}]}), Some("claude"));
    layer(&mut s, json!({"op": "hint.show", "anchor": format!("row:{line}"), "text": "down here"}), Some("claude"));
    // Off screen: which way it lies, and the hint's edge chip.
    assert!(matches!(resolved(&mut s, "L-1").off, Some(caretline_layers::Off::Below { .. })), "{:?}", resolved(&mut s, "L-1"));
    assert!(matches!(resolved(&mut s, "L-2").off, Some(caretline_layers::Off::Below { .. })));
    assert!(s.app.render.layer_plan.as_ref().unwrap().layers.iter().any(|l| l.chip.is_some()), "{}", serde_json::to_string_pretty(s.app.render.layer_plan.as_ref().unwrap()).unwrap());
    // Scroll to it: it resolves to its cells, the same text.
    for _ in 0..3 {
        s.apply(Msg::Key { key: "<pgdn>".into() }).unwrap();
    }
    let on = resolved(&mut s, "L-1");
    assert_eq!(on.rects.len(), 1, "{on:?}");
    assert_eq!(cells(&s, &on.rects[0]), "line", "the anchor shows its own word");
    // Resize: still on its word.
    s.apply(Msg::Resize { w: 80, h: 24 }).unwrap();
    let r80 = resolved(&mut s, "L-1");
    if let Some(rect) = r80.rects.first() {
        assert_eq!(cells(&s, rect), "line");
    } else {
        assert!(r80.off.is_some(), "{r80:?}");
    }
    s.apply(Msg::Resize { w: 100, h: 30 }).unwrap();
    // Edit before it: the person types at the top of the page; the anchor moves with its text.
    let before = s.app.ui.layers.stack.get("L-1").unwrap().anchor.clone();
    s.apply(Msg::Key { key: "<c-home>".into() }).ok();
    s.apply(Msg::Key { key: "<home>".into() }).unwrap();
    for c in ["N", "e", "w", " "] {
        s.apply(Msg::Key { key: c.into() }).unwrap();
    }
    let after = s.app.ui.layers.stack.get("L-1").unwrap().anchor.clone();
    assert_ne!(before, after, "the edit moved the text anchor");
    let caretline_layers::Anchor::Text { from: f2, .. } = after[0] else { panic!("{after:?}") };
    assert_eq!(f2, from + 2 + 4, "four chars typed before it");
    for _ in 0..3 {
        s.apply(Msg::Key { key: "<pgdn>".into() }).unwrap();
    }
    let on = resolved(&mut s, "L-1");
    if let Some(rect) = on.rects.first() {
        assert_eq!(cells(&s, rect), "line", "still on its word after the edit");
    }
    // Delete its text: the anchor goes with it (its fallback, none here: the layer goes).
    let n = s.app.ui.layers.stack.layers.len();
    s.apply(Msg::DocView { req: json!({"op": "doc.text.set", "id": line, "text": "gone"}), actor: Some("claude".into()) }).unwrap();
    assert_eq!(s.app.ui.layers.stack.layers.len(), n - 1, "a layer whose text was deleted goes");
}

#[test]
fn layers_the_person_takes_agents_layers_away() {
    let (_x, vault) = fixture("person", 0);
    let mut s = session(vault, (100, 30), "dark");
    tasks(&mut s);
    layer(&mut s, json!({"op": "highlight", "anchor": "ui:tab:pages"}), None);
    layer(&mut s, json!({"op": "hint.show", "anchor": format!("row:{}", id("signup")), "text": "one"}), Some("claude"));
    s.apply(Msg::Tick { now_ms: NOW + 10, utc_offset_min: 0 }).unwrap();
    layer(&mut s, json!({"op": "highlight", "anchor": format!("row:{}", id("docs"))}), Some("claude"));
    // ⌘[ takes back the newest agent layer, not a view step.
    let view = s.app.ui.view;
    s.apply(Msg::Key { key: "<d-[>".into() }).unwrap();
    assert_eq!(s.app.ui.view, view);
    assert_eq!(s.app.ui.layers.stack.layers.len(), 2);
    assert!(s.app.ui.layers.stack.get("L-2").is_some(), "the older agent layer stays");
    // Esc: every agent layer goes; thc's own stays.
    s.apply(Msg::Key { key: "<esc>".into() }).unwrap();
    let left: Vec<_> = s.app.ui.layers.stack.layers.iter().map(|l| l.owner.clone()).collect();
    assert_eq!(left, vec![caretline_layers::Owner::Host]);
    // A click on a hint's box dismisses it.
    layer(&mut s, json!({"op": "hint.show", "anchor": format!("row:{}", id("signup")), "text": "click me"}), Some("claude"));
    s.render(100, 30, "text").unwrap();
    let r = s.app.render.layer_plan.as_ref().unwrap().layers.iter().find(|l| l.agent).unwrap().rect.unwrap();
    s.apply(Msg::Mouse { mouse: crate::session::Mouse { kind: crate::session::MouseKind::Down, x: r.x + 2, y: r.y + 1, mods: String::new(), clicks: Some(1) } }).unwrap();
    assert!(!s.app.ui.layers.has_agent_layers());
    // An agent can't change layers through a state.
    let mut patch = json!({"layers": {"stack": {"layers": []}}});
    patch["layers"]["stack"]["next"] = json!(0);
    assert!(s.apply(Msg::Patch { patch, actor: Some("claude".into()) }).is_err());
}

#[test]
fn layers_walkthrough_keys_and_buttons() {
    let (_x, vault) = fixture("tour-keys", 0);
    let mut s = session(vault, (100, 30), "dark");
    scene(&mut s, "tour");
    let step = |s: &Session| s.app.ui.layers.tour.index();
    assert_eq!(step(&s), Some(1));
    s.apply(Msg::Key { key: "<f2>".into() }).unwrap();
    assert_eq!(step(&s), Some(2));
    s.apply(Msg::Key { key: "<s-f2>".into() }).unwrap();
    assert_eq!(step(&s), Some(1));
    // The next › button.
    s.render(100, 30, "text").unwrap();
    let next = s.app.render.layer_plan.as_ref().unwrap().regions.iter().find(|r| r.id.ends_with("/next")).unwrap().rect;
    s.apply(Msg::Mouse { mouse: crate::session::Mouse { kind: crate::session::MouseKind::Down, x: next.x, y: next.y, mods: String::new(), clicks: Some(1) } }).unwrap();
    assert_eq!(step(&s), Some(2));
    s.apply(Msg::Key { key: "<f3>".into() }).unwrap();
    assert_eq!(step(&s), None);
    assert!(s.app.ui.layers.stack.layers.is_empty());
}

#[test]
fn layers_policy_defaults_hold_agents_over_the_socket() {
    let (_x, vault) = fixture("policy", 0);
    let mut s = session(vault, (100, 30), "dark");
    s.app.layer_limits = crate::layers::AgentLimits::Defaults;
    tasks(&mut s);
    let host = crate::ui_proto::Host { clock: false, effects: false };
    let ask = |s: &mut Session, req: Value| -> Value { serde_json::from_str(&crate::ui_proto::handle(s, &req.to_string(), &host).response).unwrap() };
    let rev = s.rev;
    let r = ask(&mut s, json!({"op": "focus", "anchor": format!("row:{}", id("signup")), "actor": "claude"}));
    assert_eq!(r["error"]["reason"], "dim_not_allowed", "{r}");
    assert_eq!(s.rev, rev, "a refusal changes nothing");
    let r = ask(&mut s, json!({"op": "hint.show", "anchor": format!("row:{}", id("signup")), "text": "ok", "actor": "claude"}));
    assert_eq!(r["result"]["layer"], "L-1", "{r}");
    assert!(r["result"]["resolved"]["rects"].is_array(), "{r}");
    // Under the defaults an agent layer has a lifetime: the clock takes it away.
    s.apply(Msg::Tick { now_ms: NOW + 60_000, utc_offset_min: 0 }).unwrap();
    assert!(s.app.ui.layers.stack.layers.is_empty());
    // Off (the default): the same focus is fine.
    s.app.layer_limits = crate::layers::AgentLimits::Off;
    let r = ask(&mut s, json!({"op": "focus", "anchor": format!("row:{}", id("signup")), "actor": "claude"}));
    assert!(r["result"]["layer"].is_string(), "{r}");
    let ls = ask(&mut s, json!({"op": "layer.ls"}));
    assert_eq!(ls["result"]["layers"]["layers"].as_array().unwrap().len(), 1);
}

#[test]
fn layers_a_trace_replays_byte_identically() {
    let (_x, vault) = fixture("replay", 8);
    let mut live = session(vault, (120, 32), "dark");
    open_page(&mut live);
    live.apply(Msg::DocView { req: json!({"op": "doc.view.open", "at": {"id": id("goal"), "end": true}}), actor: Some("claude".into()) }).unwrap();
    live.apply(Msg::DocView { req: json!({"op": "doc.msgs", "msgs": [{"msg": "insert_newline"}, {"msg": "insert_text", "text": "Written by claude, through its own view."}]}), actor: Some("claude".into()) }).unwrap();
    let para = live.app.doc_view_reply.take().unwrap();
    layer(&mut live, json!({"op": "highlight", "anchor": format!("row:{}", id("signup"))}), Some("claude"));
    layer(&mut live, json!({"op": "hint.show", "anchor": [{"text": para["line"]["text"]}], "text": "I wrote this."}), Some("claude"));
    for k in ["<down>", "T", "y", "p", "e", "d"] {
        live.apply(Msg::Key { key: k.into() }).unwrap();
    }
    scene(&mut live, "tour");
    live.apply(Msg::Key { key: "<f2>".into() }).unwrap();
    let expected = live.render(120, 32, "ansi").unwrap().frame.unwrap();
    // The person's caret stayed where they put it: the agent typed through its own view.
    let (_, lines) = live.trace(None, true).unwrap();
    let trace: String = lines.iter().map(|l| format!("{l}\n")).collect();
    assert!(trace.contains("\"msg\":\"layer\"") && trace.contains("\"msg\":\"doc_view\""));
    let paths = live.app.vault.paths.clone();
    let mut runs = Vec::new();
    for _ in 0..2 {
        let mut copies = Vec::new();
        let mut open = |at: Option<&thc_core::vault::Frontier>| -> Result<Session, String> {
            let p = thc_core::vault::scratch_copy_at(&paths, at).map_err(|e| format!("{e:#}"))?;
            copies.push(p.vault.parent().unwrap().to_path_buf());
            let mut v = Vault::open(Paths { vault: p.vault, cache: p.cache }, thc_core::event::Actor { kind: "human".into(), name: None }, "tui").map_err(|e| format!("{e:#}"))?;
            v.origin = Some(paths.clone());
            crate::SNAPSHOT.with(|s| s.set(true));
            let mut app = crate::app::App::new(v).map_err(|e| format!("{e:#}"))?;
            app.daemon_live = false;
            Ok(Session::new(app, (80, 24)))
        };
        runs.push(crate::session::replay(&mut open, &trace, None, "ansi", true).unwrap());
        for c in copies {
            let _ = std::fs::remove_dir_all(c);
        }
    }
    assert_eq!(runs[0], runs[1], "two replays differ");
    assert_eq!(runs[0].last().unwrap(), &expected, "the replay's last frame isn't the live one");
}

#[test]
fn layers_keys_live_in_the_keymap() {
    let (_x, vault) = fixture("keys", 0);
    let mut s = session(vault, (120, 32), "dark");
    scene(&mut s, "tour");
    // The footer and help show the walkthrough's keys.
    let ctxs = crate::keymap::footer_ctxs(&s.app);
    assert_eq!(ctxs.first(), Some(&crate::keymap::Ctx::Layers), "{ctxs:?}");
    let hints: Vec<String> = crate::keymap::footer(&s.app, &ctxs).iter().map(|h| format!("{} {}", h.keys, h.label)).collect();
    assert!(hints.iter().any(|h| h.contains("F2") && h.contains("next")), "{hints:?}");
    assert!(crate::keymap::help_ctxs(&s.app, false).contains(&crate::keymap::Ctx::Layers));
    let f = s.render(120, 32, "text").unwrap().frame.unwrap();
    assert!(f.lines().last().unwrap().contains("F2"), "{f}");
    // Gone with the walkthrough.
    s.apply(Msg::Key { key: "<f3>".into() }).unwrap();
    assert!(!crate::keymap::footer_ctxs(&s.app).contains(&crate::keymap::Ctx::Layers));
}

#[test]
fn layers_a_step_still_to_come_follows_edits() {
    let (_x, vault) = fixture("pending", 0);
    let mut s = session(vault, (100, 30), "dark");
    open_page(&mut s);
    s.apply(Msg::DocView { req: json!({"op": "doc.view.open", "at": {"id": id("invites")}}), actor: Some("claude".into()) }).unwrap();
    let r = s.app.doc_view_reply.take().unwrap();
    let from = r["line"]["text"]["from"].as_u64().unwrap() as usize;
    let steps = json!([
        {"anchor": format!("row:{}", id("signup")), "text": "first"},
        {"anchor": {"text": {"from": from, "to": from + 4}}, "text": "Send"},
    ]);
    layer(&mut s, json!({"op": "tour.start", "steps": steps}), Some("claude"));
    // The person types at the top of the page while step 1 shows.
    s.apply(Msg::Key { key: "<home>".into() }).unwrap();
    for c in ["N", "e", "w", " "] {
        s.apply(Msg::Key { key: c.into() }).unwrap();
    }
    let a = s.app.ui.layers.tour.tour.as_ref().unwrap().steps[1].layers[0].anchor.clone();
    assert_eq!(a, vec![caretline_tour::StepAnchor::At(caretline_layers::Anchor::Text { from: from + 4, to: from + 8 })], "the step still to come moved with its text");
    layer(&mut s, json!({"op": "tour.next"}), Some("claude"));
    let on = resolved(&mut s, "s2/0");
    assert_eq!(cells(&s, &on.rects[0]), "Send");
}

#[test]
fn layers_a_walkthrough_box_stays_when_clicked() {
    let (_x, vault) = fixture("tour-click", 0);
    let mut s = session(vault, (100, 30), "dark");
    scene(&mut s, "tour");
    s.render(100, 30, "text").unwrap();
    let r = s.app.render.layer_plan.as_ref().unwrap().layers.iter().find(|l| l.id == "s2/0").unwrap().rect.unwrap();
    s.apply(Msg::Mouse { mouse: crate::session::Mouse { kind: crate::session::MouseKind::Down, x: r.x + 2, y: r.y + 1, mods: String::new(), clicks: Some(1) } }).unwrap();
    assert!(s.app.ui.layers.touring() && s.app.ui.layers.stack.get("s2/0").is_some());
}

#[test]
fn layers_a_segment_starting_mid_session_reopens_agent_views() {
    let (_x, vault) = fixture("midseg", 0);
    let mut live = session(vault, (100, 30), "dark");
    open_page(&mut live);
    live.apply(Msg::DocView { req: json!({"op": "doc.view.open", "at": {"id": id("goal"), "end": true}}), actor: Some("claude".into()) }).unwrap();
    assert_eq!(live.state().agent_views["claude"].caret_id, id("goal"));
    // A new segment starts here; then the agent writes at its caret.
    live.checkpoint_saved();
    live.apply(Msg::DocView { req: json!({"op": "doc.msgs", "msgs": [{"msg": "insert_text", "text": " (agreed)"}]}), actor: Some("claude".into()) }).unwrap();
    let expected = live.render(100, 30, "text").unwrap().frame.unwrap();
    assert!(expected.contains("end of the month. (agreed)"), "{expected}");
    let (_, lines) = live.trace(None, false).unwrap();
    assert!(lines[0]["state"]["agent_views"]["claude"]["caret_id"] == json!(id("goal")), "{}", lines[0]);
    let trace: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let paths = live.app.vault.paths.clone();
    let mut copies = Vec::new();
    let mut open = |at: Option<&thc_core::vault::Frontier>| -> Result<Session, String> {
        let p = thc_core::vault::scratch_copy_at(&paths, at).map_err(|e| format!("{e:#}"))?;
        copies.push(p.vault.parent().unwrap().to_path_buf());
        let mut v = Vault::open(Paths { vault: p.vault, cache: p.cache }, thc_core::event::Actor { kind: "human".into(), name: None }, "tui").map_err(|e| format!("{e:#}"))?;
        v.origin = Some(paths.clone());
        crate::SNAPSHOT.with(|s| s.set(true));
        let mut app = crate::app::App::new(v).map_err(|e| format!("{e:#}"))?;
        app.daemon_live = false;
        Ok(Session::new(app, (80, 24)))
    };
    let frames = crate::session::replay(&mut open, &trace, None, "text", false).unwrap();
    for c in copies {
        let _ = std::fs::remove_dir_all(c);
    }
    assert_eq!(frames.last().unwrap(), &expected);
}

#[test]
fn layers_a_caretline_tour_sets_the_scene_and_advances_on_an_action() {
    let (_x, vault) = fixture("ct", 0);
    let mut s = session(vault, (100, 30), "dark");
    let tour = json!({
        "id": "thc.test", "version": 2, "title": "Test", "kind": "thc.tour",
        "step": [
            {"id": "tasks", "host": {"view": "tasks"}, "anchor": [format!("row:{}", id("signup")), {"screen": "center"}], "data": {"title": "Tasks", "text": "Open Pages next."}, "place": {"arrow": true}, "advance": {"command": "go.pages"}},
            {"id": "pages", "anchor": {"screen": "center"}, "data": {"text": "That's all."}}
        ]
    });
    let host = crate::ui_proto::Host { clock: false, effects: false };
    let r: Value = serde_json::from_str(&crate::ui_proto::handle(&mut s, &json!({"op": "tour.start", "tour": tour, "actor": "claude"}).to_string(), &host).response).unwrap();
    assert_eq!(r["result"]["layer"], "tasks/0", "{r}");
    // The step's host patch set the scene, as a step the agent took.
    assert_eq!(s.app.ui.view, crate::app::View::Tasks);
    let f = s.render(100, 30, "text").unwrap().frame.unwrap();
    assert!(f.contains("◆ claude") && f.contains("Open Pages next."), "{f}");
    // The person goes to Pages: the step's `advance` holds, the walkthrough moves on.
    s.apply(Msg::Key { key: "4".into() }).unwrap();
    assert_eq!(s.app.ui.layers.tour.step.as_deref(), Some("pages"));
    s.apply(Msg::Key { key: "<f2>".into() }).unwrap();
    assert!(!s.app.ui.layers.touring());
    assert_eq!(s.app.ui.layers.tour.seen["thc.test"].version, 2);
    // Its ops schema is in the protocol's.
    assert!(caretline_tour::ops::schema().get("tour.start").is_some() || caretline_tour::ops::schema().to_string().contains("tour.start"));
}
