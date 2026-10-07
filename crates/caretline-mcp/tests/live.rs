//! The MCP server against real engines: a live editor in a pseudo-terminal (a person at the
//! keyboard) and `caretline serve`, driven over the server's stdio as an MCP client would.

mod common;

use std::process::Command;

use common::*;
use serde_json::{json, Value};

fn rev(v: &Value) -> u64 {
    v["rev"].as_u64().unwrap_or_else(|| panic!("no rev in {v}"))
}

#[test]
fn an_agent_fixes_a_typo_in_a_live_editor() {
    let dir = scratch("typo");
    let file = dir.join("notes.md");
    std::fs::write(&file, "hello wrold\nsecond line\n").unwrap();
    let mut pty = live_editor(&dir, &file);
    let mut mcp = Mcp::start(&dir, &[]);

    // The editor is discovered and attached to.
    let list = mcp.ok("list_editors", json!({}));
    assert_eq!(list["editors"][0]["pid"], pty.child.id(), "{list}");
    assert_eq!(list["editors"][0]["alive"], true);
    let open = mcp.ok("open", json!({}));
    assert_eq!(open["live"], true);
    let s = open["session"].clone();

    // Read: the text and a rev.
    let read = mcp.ok("read", json!({"session": s}));
    assert_eq!(read["text"], "hello wrold\nsecond line\n");
    assert_eq!(read["lines"], 2);
    let r0 = rev(&read);

    // An edit without if_rev is refused.
    let (err, v) = mcp.tool("edit", json!({"session": s, "ops": [{"kind": "replace", "search": "wrold", "text": "world"}]}));
    assert!(err, "{v}");

    // A guarded edit lands, on the person's screen too, attributed in the status bar.
    let e = mcp.ok("edit", json!({"session": s, "if_rev": r0, "ops": [{"kind": "replace", "search": "wrold", "text": "world"}]}));
    assert_eq!(e["dirty"], true);
    assert_eq!(e["diff"]["new_lines"][0], "hello world");
    pty.wait("the fix on screen", |sc| sc.contains("hello world") && sc.contains("test-agent: edited line 1"));
    let r1 = rev(&e);
    assert!(r1 > r0);

    // A search that matches twice, or not at all, is refused with where it matched.
    let (err, v) = mcp.tool("edit", json!({"session": s, "if_rev": r1, "ops": [{"kind": "replace", "search": "l", "text": "L"}]}));
    assert!(err);
    assert_eq!(v["error"], "ambiguous", "{v}");
    let (err, v) = mcp.tool("edit", json!({"session": s, "if_rev": r1, "ops": [{"kind": "replace", "search": "nowhere", "text": "x"}]}));
    assert!(err);
    assert_eq!(v["error"], "not_found");

    // The person types: an edit against the old rev is stale, says who and what changed.
    pty.send(b"X");
    pty.wait("the person's key", |sc| sc.contains("Xhello world"));
    let (err, v) = mcp.tool("edit", json!({"session": s, "if_rev": r1, "ops": [{"kind": "insert", "at": {"line": 2, "col": 1}, "text": "> "}]}));
    assert!(err, "a stale edit must be refused: {v}");
    assert_eq!(v["error"], "stale", "{v}");
    assert_eq!(v["changed_by"], json!(["person"]));
    assert_eq!(v["diff"]["new_lines"][0], "Xhello world");
    assert!(rev(&v) > r1);
    let read = mcp.ok("read", json!({"session": s}));
    assert_eq!(read["text"], "Xhello world\nsecond line\n", "nothing was written");

    // After reading again, the same edit lands, by line and column.
    let e = mcp.ok("edit", json!({"session": s, "if_rev": rev(&read), "ops": [{"kind": "insert", "at": {"line": 2, "col": 1}, "text": "> "}]}));
    pty.wait("the insert", |sc| sc.contains("> second line"));

    // A caret move by the person isn't a text change: the agent's rev still holds.
    pty.send(b"\x1b[B"); // down
    eventually("the caret move", || mcp.ok("read", json!({"session": s}))["carets"]["person"]["caret"]["line"] == 2);
    mcp.ok("edit", json!({"session": s, "if_rev": rev(&e), "ops": [{"kind": "replace_range", "from": {"line": 2, "col": 3}, "to": {"line": 2, "col": 9}, "text": "2nd"}]}));
    pty.wait("the range edit", |sc| sc.contains("> 2nd line"));

    // Only save writes the file.
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello wrold\nsecond line\n", "nothing saved yet");
    let saved = mcp.ok("save", json!({"session": s}));
    assert_eq!(saved["saved"], true, "{saved}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Xhello world\n> 2nd line\n");
}

#[test]
fn the_agents_view_leaves_the_persons_caret_alone() {
    let dir = scratch("view");
    let file = dir.join("notes.md");
    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    let mut pty = live_editor(&dir, &file);
    let mut mcp = Mcp::start(&dir, &[]);
    let s = mcp.ok("open", json!({}))["session"].clone();
    let r = rev(&mcp.ok("read", json!({"session": s})));

    // Without its own view, the agent may not move a caret in a live editor.
    let (err, v) = mcp.tool("edit", json!({"session": s, "if_rev": r, "ops": [{"kind": "keys", "keys": "<down>"}]}));
    assert!(err, "{v}");

    let v = mcp.ok("view_open", json!({"session": s, "width": 40, "height": 6}));
    assert!(v["view"].as_u64().unwrap() >= 1);
    let read = mcp.ok("read", json!({"session": s}));
    let e = mcp.ok("edit", json!({"session": s, "if_rev": rev(&read), "ops": [{"kind": "keys", "keys": "<d-down><up><end>!"}]}));
    assert_eq!(e["agent"]["caret"], json!({"line": 3, "col": 7}), "{e}");
    pty.wait("the agent's keys", |sc| sc.contains("three!"));

    // The person's caret is where it was, and their typing goes there.
    let read = mcp.ok("read", json!({"session": s}));
    assert_eq!(read["carets"]["person"]["caret"], json!({"line": 1, "col": 1}), "{read}");
    pty.send(b">");
    pty.wait("the person's key", |sc| sc.contains(">one"));

    // select then type, through the agent's view.
    let read = mcp.ok("read", json!({"session": s}));
    let e = mcp.ok("edit", json!({"session": s, "if_rev": rev(&read), "ops": [{"kind": "select", "from": {"line": 2, "col": 1}, "to": {"line": 2, "col": 4}}]}));
    assert_eq!(e["agent"]["selection"]["text"], "two", "{e}");
    let e = mcp.ok("edit", json!({"session": s, "if_rev": rev(&e), "ops": [{"kind": "keys", "keys": "TWO"}]}));
    pty.wait("the typed replacement", |sc| sc.contains("TWO"));
    assert_eq!(e["agent"]["caret"], json!({"line": 2, "col": 4}));
    let read = mcp.ok("read", json!({"session": s}));
    assert_eq!(read["text"], ">one\nTWO\nthree!\n");
    assert_eq!(read["carets"]["person"]["caret"], json!({"line": 1, "col": 2}));

    // The rendered frame of the agent's view, and of the person's.
    let read = mcp.ok("read", json!({"session": s, "render": {"width": 30, "height": 5}}));
    assert_eq!(read["render"]["cursor"], json!([3, 1]), "{read}");
    let person = mcp.ok("read", json!({"session": s, "render": {"width": 30, "height": 5}, "view": "person"}));
    assert_eq!(person["render"]["cursor"], json!([1, 0]));

    mcp.ok("view_close", json!({"session": s}));
    mcp.ok("close", json!({"session": s}));
    pty.send(b"!");
    pty.wait("the editor still runs", |sc| sc.contains(">!one"));
}

#[test]
fn an_agent_inserting_at_the_persons_caret_leaves_their_typing_on_their_line() {
    // The live-demo bug: the person types at the end of a line; the agent inserts a note at
    // exactly that place; the person's next key must continue their own line.
    let dir = scratch("caret");
    let file = dir.join("notes.md");
    std::fs::write(&file, "Hey there, \nnext\n").unwrap();
    let mut pty = live_editor(&dir, &file);
    let mut mcp = Mcp::start(&dir, &[]);
    let s = mcp.ok("open", json!({}))["session"].clone();
    pty.send(b"\x05"); // ctrl-e: the end of the line
    eventually("the person's caret at the line's end", || mcp.ok("read", json!({"session": s}))["carets"]["person"]["caret"] == json!({"line": 1, "col": 12}));

    let mut want = String::from("Hey there, ");
    let mut notes = String::new();
    // Shared undo and outside it, without an agent view; then through the agent's own view.
    for (i, (undo, view)) in [("shared", false), ("outside", false), ("shared", true)].into_iter().enumerate() {
        if view {
            mcp.ok("view_open", json!({"session": s}));
        }
        let read = mcp.ok("read", json!({"session": s}));
        let caret = read["carets"]["person"]["caret"].clone();
        assert_eq!(caret["line"], 1, "{read}");
        let note = format!("\n- Agent note {i}: safely.");
        mcp.ok("edit", json!({"session": s, "if_rev": rev(&read), "undo": undo, "ops": [{"kind": "insert", "at": caret, "text": note}]}));
        let after = mcp.ok("read", json!({"session": s}));
        assert_eq!(after["carets"]["person"]["caret"], caret, "{undo} view={view}: the person's caret stayed before the note");
        let key = (b'w' + i as u8) as char;
        pty.send(key.to_string().as_bytes());
        want.push(key);
        notes = format!("{note}{notes}");
        let full = format!("{want}{notes}\nnext\n");
        eventually("the person's key on their own line", || mcp.ok("read", json!({"session": s}))["text"] == json!(full));
    }
    pty.wait("the person's line", |sc| sc.contains("Hey there, wxy"));
}

#[test]
fn watch_reports_the_persons_typing() {
    let dir = scratch("watch");
    let file = dir.join("notes.md");
    std::fs::write(&file, "draft\n").unwrap();
    let mut pty = live_editor(&dir, &file);
    let mut mcp = Mcp::start(&dir, &[]);
    let s = mcp.ok("open", json!({}))["session"].clone();
    let r = rev(&mcp.ok("read", json!({"session": s})));

    // Nothing happens: the watch times out.
    let w = mcp.ok("watch", json!({"session": s, "since_rev": r, "timeout_ms": 300}));
    assert_eq!(w["timed_out"], true);
    assert_eq!(w["changes"], json!([]));

    // The person types while the agent watches.
    let typist = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        for b in b"abc" {
            pty.send(&[*b]);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        pty
    });
    let w = mcp.ok("watch", json!({"session": s, "since_rev": r, "timeout_ms": 10000}));
    let _pty = typist.join().unwrap();
    assert_eq!(w["timed_out"], false, "{w}");
    assert_eq!(w["changes"][0]["who"], "person", "{w}");
    assert_eq!(w["changes"][0]["actions"], json!(["typed \"abc\""]), "{w}");
    assert_eq!(w["text_changed"], true);
    assert_eq!(w["diff"]["new_lines"][0], "abcdraft");

    // The agent's own edits are left out unless asked for.
    let r2 = rev(&w);
    mcp.ok("read", json!({"session": s}));
    mcp.ok("edit", json!({"session": s, "if_rev": r2, "ops": [{"kind": "insert", "at": {"line": 1, "col": 1}, "text": "# "}]}));
    let w = mcp.ok("watch", json!({"session": s, "since_rev": r2, "timeout_ms": 200}));
    assert_eq!(w["changes"], json!([]), "{w}");
    let w = mcp.ok("watch", json!({"session": s, "since_rev": r2, "timeout_ms": 200, "include_own": true}));
    assert_eq!(w["changes"][0]["who"], "this agent", "{w}");
}

#[test]
fn the_trace_replays_to_the_same_state() {
    let dir = scratch("trace");
    let file = dir.join("notes.md");
    std::fs::write(&file, "alpha\nbeta\n").unwrap();
    let mut pty = live_editor(&dir, &file);
    let mut mcp = Mcp::start(&dir, &[]);
    let s = mcp.ok("open", json!({}))["session"].clone();
    let r = rev(&mcp.ok("read", json!({"session": s})));
    mcp.ok("view_open", json!({"session": s}));
    let e = mcp.ok("edit", json!({"session": s, "if_rev": r, "ops": [{"kind": "replace", "search": "beta", "text": "gamma"}]}));
    pty.send(b"Z");
    pty.wait("the person's key", |sc| sc.contains("Zalpha"));
    let read = mcp.ok("read", json!({"session": s}));
    mcp.ok("edit", json!({"session": s, "if_rev": rev(&read), "ops": [{"kind": "keys", "keys": "<d-down>delta"}]}));
    assert!(rev(&e) > r);

    let path = dir.join("t.jsonl");
    let t = mcp.ok("trace", json!({"session": s, "path": path}));
    assert_eq!(t["replayable_alone"], true, "{t}");
    let final_text = mcp.ok("read", json!({"session": s}))["text"].clone();
    assert_eq!(final_text, "Zalpha\ngamma\ndelta");
    let out = Command::new(caretline()).arg("--replay").arg(&path).args(["--dump-state", "-"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let replayed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(replayed["text"], final_text);

    // Inline, and since a rev.
    let (err, t) = mcp.tool("trace", json!({"session": s, "since_rev": r}));
    assert!(!err);
    assert_eq!(t["replayable_alone"], false);
    assert!(t["lines"].as_u64().unwrap() >= 3, "{t}");
}

#[test]
fn a_served_engine_on_a_socket_and_read_only_mode() {
    let dir = scratch("serve");
    let file = dir.join("doc.txt");
    std::fs::write(&file, "one two\n").unwrap();
    let sock = dir.join("serve.sock");
    let mut serve = Command::new(caretline()).arg("serve").arg(&file).arg("--socket").arg(&sock).spawn().unwrap();
    eventually("the socket", || sock.exists());

    let mut mcp = Mcp::start(&dir, &[]);
    let s = mcp.ok("open", json!({"socket": sock}))["session"].clone();
    let r = rev(&mcp.ok("read", json!({"session": s})));
    let e = mcp.ok("edit", json!({"session": s, "if_rev": r, "ops": [
        {"kind": "replace", "search": "one", "text": "1"},
        {"kind": "replace", "search": "two", "text": "2"}
    ]}));
    assert_eq!(e["diff"]["new_lines"][0], "1 2");
    // Overlapping operations are refused.
    let (err, _) = mcp.tool("edit", json!({"session": s, "if_rev": rev(&e), "ops": [
        {"kind": "replace_range", "from": {"line": 1, "col": 1}, "to": {"line": 1, "col": 3}, "text": "x"},
        {"kind": "insert", "at": {"line": 1, "col": 2}, "text": "y"}
    ]}));
    assert!(err);

    // A read-only server reads and watches, and refuses to write.
    let mut ro = Mcp::start(&dir, &["--read-only"]);
    let s2 = ro.ok("open", json!({"socket": sock}))["session"].clone();
    let read = ro.ok("read", json!({"session": s2}));
    assert_eq!(read["text"], "1 2\n");
    let (err, v) = ro.tool("edit", json!({"session": s2, "if_rev": rev(&read), "ops": [{"kind": "insert", "at": {"line": 1, "col": 1}, "text": "x"}]}));
    assert!(err, "{v}");
    let (err, _) = ro.tool("save", json!({"session": s2}));
    assert!(err);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "one two\n");

    // Another client's edit shows up in the first agent's watch as "another client".
    let r = rev(&mcp.ok("read", json!({"session": s})));
    let mut other = Mcp::start(&dir, &[]);
    let s3 = other.ok("open", json!({"socket": sock}))["session"].clone();
    let rr = rev(&other.ok("read", json!({"session": s3})));
    other.ok("edit", json!({"session": s3, "if_rev": rr, "ops": [{"kind": "insert", "at": {"line": 1, "col": 1}, "text": "0 "}]}));
    let w = mcp.ok("watch", json!({"session": s, "since_rev": r, "timeout_ms": 5000}));
    assert_eq!(w["changes"][0]["who"], "another client", "{w}");
    let _ = serve.kill();
    let _ = serve.wait();
}

#[test]
fn headless_sessions_edit_select_and_save() {
    let dir = scratch("headless");
    let file = dir.join("todo.md");
    std::fs::write(&file, "- [ ] Pay rent\n- Buy milk\n").unwrap();
    let mut mcp = Mcp::start(&dir, &[]);
    let open = mcp.ok("open", json!({"file": file, "outline": true}));
    assert_eq!(open["live"], false);
    let s = open["session"].clone();
    let read = mcp.ok("read", json!({"session": s, "from_line": 2, "to_line": 2}));
    assert_eq!(read["text"], "- Buy milk"); // an outline buffer has no final line break
    // A headless engine has no person: keys and select use its own caret.
    let e = mcp.ok("edit", json!({"session": s, "if_rev": rev(&read), "ops": [{"kind": "select", "from": {"line": 1, "col": 7}, "to": {"line": 1, "col": 10}}]}));
    let e = mcp.ok("edit", json!({"session": s, "if_rev": rev(&e), "ops": [{"kind": "keys", "keys": "Send<c-t>"}]}));
    assert_eq!(e["diff"]["new_lines"][0], "- [x] Send rent", "{e}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "- [ ] Pay rent\n- Buy milk\n");
    let saved = mcp.ok("save", json!({"session": s}));
    assert_eq!(saved["saved"], true);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "- [x] Send rent\n- Buy milk\n");

    // From text: no file to save.
    let t = mcp.ok("open", json!({"text": "abc"}))["session"].clone();
    let r = rev(&mcp.ok("read", json!({"session": t})));
    mcp.ok("edit", json!({"session": t, "if_rev": r, "ops": [{"kind": "insert", "at": {"line": 1, "col": 4}, "text": "def"}]}));
    assert_eq!(mcp.ok("read", json!({"session": t}))["text"], "abcdef");
    let (err, _) = mcp.tool("save", json!({"session": t}));
    assert!(err);
    let (err, v) = mcp.tool("read", json!({}));
    assert!(err, "two sessions open: session is required: {v}");
}

#[test]
fn the_persons_undo_and_the_agents_edits() {
    let dir = scratch("undo");
    let file = dir.join("notes.md");
    std::fs::write(&file, "keep\n").unwrap();
    let mut pty = live_editor(&dir, &file);
    let mut mcp = Mcp::start(&dir, &["--quiet"]);
    let s = mcp.ok("open", json!({}))["session"].clone();

    // shared (the default): the agent's edit is one undo step of its own, never merged into
    // the person's typing run.
    pty.send(b"ab");
    pty.wait("typing", |sc| sc.contains("abkeep"));
    let r = rev(&mcp.ok("read", json!({"session": s})));
    mcp.ok("edit", json!({"session": s, "if_rev": r, "ops": [{"kind": "insert", "at": {"line": 1, "col": 7}, "text": "!!"}]}));
    pty.wait("the edit", |sc| sc.contains("abkeep!!"));
    pty.send(b"\x1a"); // Ctrl-Z
    pty.wait("undo", |sc| sc.contains("abkeep") && !sc.contains("!!"));
    pty.send(b"\x1a");
    pty.wait("undo", |sc| sc.contains("keep") && !sc.contains("abkeep"));

    // outside: the person's undo skips the agent's change.
    pty.send(b"cd");
    pty.wait("typing", |sc| sc.contains("cdkeep"));
    let r = rev(&mcp.ok("read", json!({"session": s})));
    let e = mcp.ok("edit", json!({"session": s, "if_rev": r, "undo": "outside", "ops": [{"kind": "replace", "search": "keep", "text": "KEEP"}]}));
    assert_eq!(e["diff"]["new_lines"][0], "cdKEEP", "{e}");
    pty.wait("the edit", |sc| sc.contains("cdKEEP"));
    pty.send(b"\x1a");
    pty.wait("undo", |sc| sc.contains("KEEP") && !sc.contains("cdKEEP"));
    assert_eq!(mcp.ok("read", json!({"session": s}))["text"], "KEEP\n");
}
