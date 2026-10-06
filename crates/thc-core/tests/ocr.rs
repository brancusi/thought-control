//! OCR props are replayed data; search stays correct as embeds and images change.
use serde_json::json;
use thc_core::{
    event::{Actor, Event, Op},
    hlc::Hlc,
    store::Store,
};
fn apply(s: &Store, seq: u64, op: Op) {
    s.apply(&Event {
        v: 1,
        eid: format!("e{seq:011}"),
        hlc: Hlc(seq, 0),
        dev: "test".into(),
        actor: Actor {
            kind: "human".into(),
            name: None,
        },
        via: "test".into(),
        tx: format!("t{seq}"),
        op,
    })
    .unwrap();
}
fn create(id: &str, text: &str, props: serde_json::Value) -> Op {
    Op::NodeCreate {
        id: id.into(),
        parent: None,
        order: "a".into(),
        title: None,
        text: text.into(),
        props: props.as_object().unwrap().clone(),
    }
}
fn ids(s: &Store, q: &str) -> Vec<String> {
    s.query(
        q,
        chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap(),
        100,
    )
    .unwrap()
    .into_iter()
    .map(|n| n.id)
    .collect()
}
#[test]
fn replayed_image_props_and_live_edges_drive_search() {
    let s = Store::open_memory().unwrap();
    apply(&s, 1, create("note00000001", "ordinary words", json!({})));
    apply(&s, 2, create("note00000002", "another note", json!({})));
    apply(
        &s,
        3,
        create(
            "image0000001",
            "",
            json!({"system":"attachment","kind":"image","path":"files/test.png"}),
        ),
    );
    let result = json!({"hash":thc_core::ocr::hash(b"fixture"),"engine":thc_core::ocr::ENGINE,"text":"Quasar invoice 2048"});
    apply(
        &s,
        4,
        Op::NodeSet {
            id: "image0000001".into(),
            props: json!({"ocr":result}).as_object().unwrap().clone(),
        },
    );
    apply(
        &s,
        5,
        Op::EdgeAdd {
            src: "note00000001".into(),
            rel: "embed".into(),
            dst: "image0000001".into(),
        },
    );
    assert_eq!(ids(&s, "text:Quasar"), ["note00000001"]);
    assert_eq!(ids(&s, "is:image text:Quasar"), ["image0000001"]);
    assert_eq!(s.search("quasar", 100).unwrap()[0].id, "note00000001");
    assert_eq!(s.search("ordinary quasar", 100).unwrap().len(), 1);
    assert_eq!(
        thc_core::ocr::search_images(&s, "note00000001", "ordinary quasar").unwrap()[0]["snippet"],
        "Quasar invoice 2048"
    );
    assert_eq!(
        s.node("note00000001").unwrap().unwrap().text,
        "ordinary words",
        "derived indexing never edits the note"
    );
    assert_eq!(
        thc_core::ocr::cached(&s, &thc_core::ocr::hash(b"fixture")).unwrap(),
        Some(result)
    );
    // Another image in the same note still yields just one result row.
    apply(
        &s,
        6,
        create(
            "image0000002",
            "",
            json!({"system":"attachment","kind":"image","path":"files/two.png","ocr":{"hash":"other","engine":thc_core::ocr::ENGINE,"text":"Quasar second"}}),
        ),
    );
    apply(
        &s,
        7,
        Op::EdgeAdd {
            src: "note00000001".into(),
            rel: "embed".into(),
            dst: "image0000002".into(),
        },
    );
    assert_eq!(s.search("quasar", 100).unwrap().len(), 1);
    // Clearing OCR and deleting/restoring an image update the note's derived FTS index.
    apply(
        &s,
        8,
        Op::NodeSet {
            id: "image0000002".into(),
            props: json!({"ocr":null}).as_object().unwrap().clone(),
        },
    );
    apply(
        &s,
        9,
        Op::NodeDelete {
            id: "image0000001".into(),
        },
    );
    assert!(s.search("quasar", 100).unwrap().is_empty());
    assert!(ids(&s, "text:Quasar").is_empty());
    apply(
        &s,
        10,
        Op::NodeRestore {
            id: "image0000001".into(),
        },
    );
    assert_eq!(s.search("quasar", 100).unwrap().len(), 1);
    apply(
        &s,
        11,
        Op::EdgeRemove {
            src: "note00000001".into(),
            rel: "embed".into(),
            dst: "image0000001".into(),
        },
    );
    assert!(s.search("quasar", 100).unwrap().is_empty());
    // Prop changes after an embed, including an empty success result.
    apply(
        &s,
        12,
        Op::EdgeAdd {
            src: "note00000002".into(),
            rel: "embed".into(),
            dst: "image0000001".into(),
        },
    );
    apply(
        &s,
        13,
        Op::NodeSet {
            id: "image0000001".into(),
            props: json!({"ocr":{"hash":"empty","engine":thc_core::ocr::ENGINE,"text":""}})
                .as_object()
                .unwrap()
                .clone(),
        },
    );
    assert!(s.search("quasar", 100).unwrap().is_empty());
    assert!(thc_core::ocr::cached(&s, "empty").unwrap().is_some());
    assert_eq!(
        thc_core::ocr::pending(&s)
            .unwrap()
            .iter()
            .map(|x| &x.0)
            .collect::<Vec<_>>(),
        ["image0000002"]
    );
}
#[cfg(target_os = "macos")]
#[test]
fn native_vision_reads_synthetic_screenshot() {
    if !thc_core::ocr::available() {
        return;
    } // supported graceful path on pre-Vision macOS
    let s = Store::open_memory().unwrap();
    let value = thc_core::ocr::recognize(&s, include_bytes!("fixtures/ocr-screenshot.png"))
        .unwrap()
        .unwrap();
    let text = value["text"].as_str().unwrap();
    for phrase in [
        "Thought Central",
        "Friday 9 October 2026",
        "$1,234.50",
        "all checks passed",
        "codex-engineer-2",
        "without uploading files",
        "CGTN-2048",
    ] {
        assert!(text.contains(phrase), "missing {phrase:?}: {text}");
    }
    assert!(thc_core::ocr::recognize(&s, b"not an image").is_err());
}
#[cfg(not(target_os = "macos"))]
#[test]
fn unavailable_platform_keeps_recognition_optional() {
    let s = Store::open_memory().unwrap();
    assert!(!thc_core::ocr::available());
    assert!(thc_core::ocr::recognize(&s, b"unused").unwrap().is_none());
}
