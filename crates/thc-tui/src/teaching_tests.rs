use crate::session::{Msg, Session};
use serde_json::json;

fn session(size: (u16, u16)) -> (crate::fuzz::Scratch, Session) {
    let (scratch, mut vault) = crate::fuzz::scratch("teaching");
    crate::teaching::seed(&mut vault).unwrap();
    crate::SNAPSHOT.with(|s| s.set(true));
    let app = crate::app::App::new(vault).unwrap();
    let mut session = Session::new(app, size);
    crate::teaching::start(&mut session).unwrap();
    (scratch, session)
}

fn keys(s: &mut Session, script: &str) {
    for msg in crate::script::parse(script, false).unwrap() {
        s.apply(msg).unwrap();
        s.render(s.size.0, s.size.1, "text").unwrap();
        s.runtime(Msg::Frame);
    }
}

fn step(s: &Session) -> String {
    s.app.ui.layers.tour.current().unwrap().id.clone()
}

fn jump(s: &mut Session, to: &str) {
    s.apply(Msg::Layer {
        req: json!({"op":"tour.step", "to":to}),
        actor: None,
    })
    .unwrap();
}

#[test]
fn teaching_scenes_anchor_at_realistic_sizes_and_compact_fallback() {
    for size in [
        (140, 40),
        (120, 32),
        (100, 30),
        (80, 24),
        (60, 16),
        (30, 10),
    ] {
        let (_scratch, mut s) = session(size);
        for id in [
            "welcome", "write", "link", "navigate", "panel", "tasks", "finish",
        ] {
            jump(&mut s, id);
            let frame = s.render(size.0, size.1, "text").unwrap().frame.unwrap();
            assert!(
                frame.contains("F2") || frame.contains("next"),
                "{size:?}/{id}: {frame}"
            );
            let plan = s.app.render.layer_plan.as_ref().unwrap();
            if !frame.contains("compact") {
                assert!(
                    plan.missing.is_empty(),
                    "{size:?}/{id}: missing {:?}",
                    plan.missing
                );
                assert!(
                    plan.layers.iter().any(|l| l.rect.is_some()),
                    "{size:?}/{id}: {plan:?}"
                );
                assert!(
                    plan.layers.iter().any(|l| l.anchor.is_some()),
                    "{size:?}/{id}: no anchor"
                );
            } else {
                assert!(frame.contains("compact"), "{size:?}/{id}: {frame}");
                assert!(!plan.regions.is_empty());
            }
            assert_eq!(frame.lines().count(), size.1 as usize);
        }
        s.apply(Msg::Resize { w: 80, h: 24 }).unwrap();
        s.render(80, 24, "text").unwrap();
        keys(&mut s, "<f3>");
        assert!(!s.app.ui.layers.touring());
        assert!(s.app.ui.layers.stack.layers.is_empty());
    }
}

#[test]
fn teaching_real_edits_navigation_back_jump_tasks_and_takeover() {
    let (_scratch, mut s) = session((120, 32));
    let original = s.app.vault.store.nodes_where("1", &[]).unwrap().len();
    let idea = s
        .app
        .doc
        .as_ref()
        .unwrap()
        .blocks()
        .iter()
        .find(|l| l.text.starts_with("An idea"))
        .unwrap()
        .id
        .clone();
    keys(&mut s, "<f2>Made by me. ");
    assert_eq!(step(&s), "write");
    assert!(
        s.app
            .doc
            .as_ref()
            .unwrap()
            .blocks()
            .iter()
            .any(|l| l.text.contains("Made by me."))
    );
    keys(&mut s, "<f2><c-o>");
    assert_eq!(step(&s), "link");
    assert!(
        matches!(&s.app.doc.as_ref().unwrap().target, crate::editor::Target::Page { title,.. } if title == "Field Notes")
    );
    keys(&mut s, "<c-m-left>");
    assert!(
        matches!(&s.app.doc.as_ref().unwrap().target, crate::editor::Target::Page { title,.. } if title == "Overlay Workshop")
    );
    keys(&mut s, "<s-f2>");
    assert_eq!(step(&s), "write");
    assert!(
        s.app
            .doc
            .as_ref()
            .unwrap()
            .blocks()
            .iter()
            .any(|l| l.id == idea && l.text.contains("Made by me."))
    );
    jump(&mut s, "panel");
    s.render(120, 32, "text").unwrap();
    assert_eq!(s.app.ui.sidebar.open.len(), 1);
    keys(&mut s, "Panel edit. ");
    s.app.save_everything();
    jump(&mut s, "tasks");
    let task = s.app.ui.selected.clone().unwrap();
    keys(&mut s, "x");
    assert_eq!(
        s.app
            .vault
            .store
            .must_node(&task)
            .unwrap()
            .status
            .as_deref(),
        Some("done")
    );
    keys(&mut s, "X");
    assert_eq!(
        s.app
            .vault
            .store
            .must_node(&task)
            .unwrap()
            .status
            .as_deref(),
        Some("todo")
    );
    jump(&mut s, "finish");
    keys(&mut s, "<f3>Still mine. ");
    assert!(!s.app.ui.layers.touring());
    s.app.save_everything();
    assert!(
        s.app
            .vault
            .store
            .must_node(&idea)
            .unwrap()
            .text
            .contains("Made by me.")
    );
    assert!(
        s.app
            .vault
            .store
            .must_node(&idea)
            .unwrap()
            .text
            .contains("Still mine.")
    );
    assert!(s.app.vault.store.nodes_where("1", &[]).unwrap().len() >= original);
}

#[test]
fn teaching_click_controls_jump_and_stop_and_sandbox_is_sticky() {
    let (_scratch, mut s) = session((100, 30));
    s.render(100, 30, "text").unwrap();
    let region = s
        .app
        .render
        .layer_plan
        .as_ref()
        .unwrap()
        .regions
        .iter()
        .find(|r| r.id == "tour-step:tasks")
        .unwrap()
        .rect;
    s.apply(Msg::Mouse {
        mouse: crate::session::Mouse {
            kind: crate::session::MouseKind::Down,
            x: region.x,
            y: region.y,
            mods: String::new(),
            clicks: Some(1),
        },
    })
    .unwrap();
    assert_eq!(step(&s), "tasks");
    assert!(
        s.apply(Msg::Patch {
            patch: json!({"teaching_demo":false}),
            actor: None
        })
        .is_err()
    );
    s.app.start_update();
    assert!(s.app.update_rx.is_none());
    s.app.switch_to = Some(std::path::PathBuf::from("not-the-scratch-vault"));
    keys(&mut s, "<f3>");
    assert!(s.app.switch_to.is_none());
    assert!(!s.app.ui.layers.touring());
}

#[test]
fn teaching_seed_refuses_existing_data() {
    let (_scratch, mut s) = session((80, 24));
    let before = s.app.vault.store.nodes_where("1", &[]).unwrap().len();
    assert!(crate::teaching::seed(&mut s.app.vault).is_err());
    assert_eq!(
        s.app.vault.store.nodes_where("1", &[]).unwrap().len(),
        before
    );
}

#[test]
fn teaching_text_anchors_follow_unicode_edits_and_agent_hints_dismiss() {
    let (_scratch, mut s) = session((100, 30));
    s.apply(Msg::Layer {
        req: json!({"op":"highlight", "anchor":"text:0..2@main"}),
        actor: None,
    })
    .unwrap();
    let id = s.app.ui.layers.stack.layers.last().unwrap().id.clone();
    keys(&mut s, "é🌱");
    let anchors = &s.app.ui.layers.stack.get(&id).unwrap().anchor;
    assert!(
        anchors
            .iter()
            .any(|a| matches!(a.unscoped(), caretline_layers::Anchor::Text {to,..} if *to >= 2))
    );
    s.apply(Msg::Layer {
        req: json!({"op":"hint.show","anchor":"caret","text":"Example agent hint"}),
        actor: Some("example-agent".into()),
    })
    .unwrap();
    keys(&mut s, "<esc>");
    assert!(!s.app.ui.layers.has_agent_layers());
    assert!(s.app.ui.layers.touring());
}

#[test]
fn teaching_headless_restore_reinstates_read_only_mode_and_tour() {
    let (_scratch, mut s) = session((100, 30));
    jump(&mut s, "tasks");
    let state = s.state().to_json();
    s.app.ui.teaching_demo = false;
    s.restore(&state).unwrap();
    assert!(s.app.ui.teaching_demo);
    assert_eq!(step(&s), "tasks");
    assert!(
        s.apply(Msg::Patch {
            patch: json!({"teaching_demo":false}),
            actor: Some("example-agent".into())
        })
        .is_err()
    );
}

#[test]
fn teaching_yields_to_help_and_supports_non_function_key_controls() {
    let (_scratch, mut s) = session((80, 24));
    keys(&mut s, "<f1>");
    assert!(s.app.ui.overlay.is_some());
    assert!(s.app.render.layer_plan.is_none());
    keys(&mut s, "<esc>");
    assert_eq!(step(&s), "welcome");
    keys(&mut s, "<m-n>");
    assert_eq!(step(&s), "write");
    keys(&mut s, "<m-p>");
    assert_eq!(step(&s), "welcome");
    keys(&mut s, "<m-g>");
    assert!(!s.app.ui.layers.touring());
}

#[test]
fn teaching_agent_tours_cannot_bypass_preferences_or_pinned_panel_policy() {
    let (_scratch, mut s) = session((120, 32));
    jump(&mut s, "panel");
    let mut panel = serde_json::to_value(&s.app.ui.sidebar.open[0]).unwrap();
    panel["pinned"] = json!(true);
    s.apply(Msg::Patch {
        patch: json!({"sidebar":{"open":[panel]}}),
        actor: None,
    })
    .unwrap();
    let preferences = s.app.ui.focus_cfg.clone();
    for host in [
        json!({"focus_cfg":{"custom_width":71}}),
        json!({"sidebar":{"open":[]},"focus":"list"}),
    ] {
        s.apply(Msg::Layer {
            req: json!({"op":"tour.start", "steps":[{"anchor":"caret", "text":"An agent's guide", "host":host}]}),
            actor: Some("example-agent".into()),
        }).unwrap();
        assert_eq!(s.app.ui.focus_cfg, preferences);
        assert!(s.app.ui.sidebar.open[0].pinned);
    }
    // Attribution must survive a reducer ending the tour before the host queue is drained.
    s.app.ui.layers.effects(
        vec![
            caretline_tour::TourEffect::Host {
                patch: json!({"focus_cfg":{"custom_width":71}}),
            },
            caretline_tour::TourEffect::Ended {
                tour: "agent-guide".into(),
                seen: caretline_tour::Seen {
                    version: 1,
                    end: caretline_tour::End::Stopped,
                },
            },
        ],
        s.app.ui.now_ms,
    );
    keys(&mut s, "<f3>");
    assert_eq!(s.app.ui.focus_cfg, preferences);
}
