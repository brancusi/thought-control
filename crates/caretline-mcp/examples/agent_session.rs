//! An agent session against a live editor, through caretline-mcp, as an MCP client sees it.
//!
//! In one terminal, a person opens a file and listens:
//!
//! ```sh
//! printf 'Teh plan\n\nShip it on fryday.\n' > /tmp/plan.md
//! caretline /tmp/plan.md --listen
//! ```
//!
//! In another, run the agent:
//!
//! ```sh
//! cargo run -p caretline-mcp --example agent_session
//! ```
//!
//! It attaches to the newest editor, opens its own view, fixes "Teh" and "fryday" with guarded
//! edits, then watches while you type, shows a stale edit being refused and redone, and writes
//! the session's trace for `caretline --replay`. It never saves: Ctrl-S stays yours.
//!
//! The server binary is `$CARETLINE_MCP`, else the one next to this example in target/, else
//! `caretline-mcp` on PATH.

use std::path::PathBuf;

use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use rmcp::ServiceExt;
use serde_json::{json, Value};

type Client = RunningService<RoleClient, ClientConfig>;

fn server() -> PathBuf {
    if let Ok(p) = std::env::var("CARETLINE_MCP") {
        return p.into();
    }
    let built = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.parent()?.join("caretline-mcp")));
    built.filter(|p| p.exists()).unwrap_or_else(|| "caretline-mcp".into())
}

/// Calls a tool, prints the call and a one-line result, and returns (ok, structured result).
async fn call(c: &Client, name: &'static str, args: Value) -> (bool, Value) {
    let Value::Object(map) = args.clone() else { unreachable!() };
    println!("\n→ {name} {}", Value::Object(map.clone()));
    let r = c.call_tool(CallToolRequestParams::new(name).with_arguments(map)).await.expect("tool call");
    let ok = r.is_error != Some(true);
    let data = r.structured_content.unwrap_or(Value::Null);
    let shown = serde_json::to_string(&data).unwrap_or_default();
    let shown: String = shown.chars().take(300).collect();
    println!("{} {shown}", if ok { "←" } else { "← error:" });
    (ok, data)
}

fn rev(v: &Value) -> u64 {
    v["rev"].as_u64().unwrap_or(0)
}

#[tokio::main]
async fn main() {
    let cmd = tokio::process::Command::new(server());
    let config = ClientConfig::new(ClientCapabilities::default(), Implementation::new("demo-agent", "0"));
    let c = config.serve(TokioChildProcess::new(cmd).expect("start caretline-mcp")).await.expect("MCP handshake");

    let (_, list) = call(&c, "list_editors", json!({})).await;
    if !list["editors"].as_array().is_some_and(|e| e.iter().any(|x| x["alive"] == true)) {
        eprintln!("\nNo live editor. In another terminal run:\n  printf 'Teh plan\\n\\nShip it on fryday.\\n' > /tmp/plan.md\n  caretline /tmp/plan.md --listen\nthen run this again.");
        return;
    }
    let (_, open) = call(&c, "open", json!({})).await;
    let s = open["session"].clone();
    call(&c, "view_open", json!({"session": s})).await;

    // Read, then fix typos with guarded edits: each names the rev it was read at.
    let (_, read) = call(&c, "read", json!({"session": s})).await;
    println!("\n{}", read["text"].as_str().unwrap_or(""));
    let mut at = rev(&read);
    for (wrong, right) in [("Teh", "The"), ("fryday", "Friday")] {
        let (ok, e) = call(&c, "edit", json!({"session": s, "if_rev": at, "ops": [{"kind": "replace", "search": wrong, "text": right}]})).await;
        if ok {
            at = rev(&e);
        }
    }

    // Watch the person: type something in the editor now.
    println!("\nType something in the editor within 30 seconds…");
    let (_, w) = call(&c, "watch", json!({"session": s, "since_rev": at, "timeout_ms": 30000, "settle_ms": 1500})).await;
    for g in w["changes"].as_array().into_iter().flatten() {
        println!("  {} (revs {}-{}): {}", g["who"], g["from_rev"], g["to_rev"], g["actions"]);
    }

    // An edit against the rev from before the typing is refused as stale…
    let (ok, stale) = call(&c, "edit", json!({"session": s, "if_rev": at, "ops": [{"kind": "keys", "keys": "<d-down><cr>-- checked by an agent"}]})).await;
    if !ok {
        println!("  refused: {}", stale["message"]);
    }
    // …so the agent reads again and redoes it, at its own caret.
    let (_, read) = call(&c, "read", json!({"session": s})).await;
    let (_, e) = call(&c, "edit", json!({"session": s, "if_rev": rev(&read), "ops": [{"kind": "keys", "keys": "<d-down><cr>-- checked by an agent"}]})).await;
    println!("  the agent's caret: {}", e["agent"]["caret"]);

    // The trace replays the whole session, the person's keys and the agent's edits alike.
    let path = std::env::temp_dir().join("caretline-agent-session.jsonl");
    let (_, t) = call(&c, "trace", json!({"session": s, "path": path})).await;
    println!("\nReplay it: {}", t["replay"].as_str().unwrap_or(""));
    println!("The document is unsaved: save it in the editor (Ctrl-S) if you like the changes.");
    call(&c, "close", json!({"session": s})).await;
    let _ = c.cancel().await;
}
