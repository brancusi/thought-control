//! A real MCP client (the official Rust SDK's) against the server: the handshake, the tool
//! list and a tool call, in the current protocol version and the last one with `initialize`.

use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation, ProtocolVersion};
use rmcp::transport::TokioChildProcess;
use rmcp::ServiceExt;
use serde_json::json;

const TOOLS: [&str; 11] = ["list_editors", "open", "read", "edit", "view_open", "view_close", "watch", "trace", "save", "commands", "close"];

async fn session(config: ClientConfig) {
    let dir = std::env::temp_dir().join(format!("clm-hs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_caretline-mcp"));
    cmd.env("TMPDIR", &dir);
    let client = config.serve(TokioChildProcess::new(cmd).unwrap()).await.expect("handshake");

    let info = client.peer_info().expect("server info");
    assert_eq!(info.server_info.as_ref().map(|i| i.name.as_str()), Some("caretline-mcp"));
    assert!(info.capabilities.tools.is_some());
    assert!(info.instructions.as_deref().unwrap_or("").contains("if_rev"));

    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(names, TOOLS);
    let edit = tools.iter().find(|t| t.name == "edit").unwrap();
    assert_eq!(edit.input_schema["required"], json!(["if_rev", "ops"]));
    assert_eq!(edit.annotations.as_ref().unwrap().read_only_hint, Some(false));

    // A headless session from text, end to end.
    let call = |name: &'static str, args: serde_json::Value| {
        let serde_json::Value::Object(args) = args else { unreachable!() };
        CallToolRequestParams::new(name).with_arguments(args)
    };
    let open = client.call_tool(call("open", json!({"text": "teh cat\n"}))).await.unwrap();
    let s = open.structured_content.unwrap()["session"].clone();
    let read = client.call_tool(call("read", json!({"session": s}))).await.unwrap();
    assert_eq!(read.content.len(), 2, "the metadata, then the text");
    let rev = read.structured_content.unwrap()["rev"].clone();
    let edit = client.call_tool(call("edit", json!({"session": s, "if_rev": rev, "ops": [{"kind": "replace", "search": "teh", "text": "the"}]}))).await.unwrap();
    assert_ne!(edit.is_error, Some(true), "{edit:?}");
    let read = client.call_tool(call("read", json!({"session": s, "numbered": false}))).await.unwrap();
    assert_eq!(read.structured_content.unwrap()["text"], "the cat\n");
    let c = client.call_tool(call("commands", json!({"outline": true}))).await.unwrap();
    let c = c.structured_content.unwrap();
    assert!(c["commands"].as_array().unwrap().iter().any(|x| x["id"] == "history.undo"), "{c}");
    assert!(c["keymap"].as_array().unwrap().iter().any(|b| b["keys"] == "<tab>" && b["command"] == "structure.indent"), "{c}");

    // A tool error is a result the model sees, not a protocol error.
    let stale = client.call_tool(call("edit", json!({"session": s, "if_rev": 999, "ops": [{"kind": "insert", "at": {"line": 1, "col": 1}, "text": "x"}]}))).await.unwrap();
    assert_eq!(stale.is_error, Some(true));
    assert_eq!(stale.structured_content.unwrap()["error"], "stale");

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn the_sdk_client_in_the_current_protocol() {
    session(ClientConfig::new(ClientCapabilities::default(), Implementation::new("sdk-test", "0"))).await;
}

#[tokio::test]
async fn the_sdk_client_with_initialize() {
    let config = ClientConfig::new(ClientCapabilities::default(), Implementation::new("sdk-test", "0")).with_protocol_version(ProtocolVersion::V_2025_11_25);
    session(config).await;
}

