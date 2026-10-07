//! The MCP face: tool definitions (names, descriptions and input schemas written for models)
//! and an rmcp `ServerHandler` that runs them.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock as Content, Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
    Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::{json, Map, Value};

use crate::tools::{ToolError, Tools};

const INSTRUCTIONS: &str = "caretline-mcp edits text in caretline, a replayable editing engine, \
including a live editor a person is typing in. Typical session: list_editors, then open (the \
newest live editor by default, or a file headless), read (note its rev), view_open (your own \
caret, so the person's stays theirs), edit with if_rev = the rev you read, watch to see what the \
person does, trace for an exact replay. An edit whose text changed since if_rev is refused as \
stale: read again and redo it. Only save writes the file, and only when the user asked.";

fn pos_schema(what: &str) -> Value {
    json!({
        "type": "object",
        "description": format!("{what}: a line and column, both 1-based, the column in characters. col = (line length + 1) is the end of the line"),
        "properties": {"line": {"type": "integer", "minimum": 1}, "col": {"type": "integer", "minimum": 1}},
        "required": ["line", "col"],
    })
}

fn session_prop() -> Value {
    json!({"type": "string", "description": "The session id from open (optional when only one session is open)"})
}

fn tool(name: &'static str, description: &str, schema: Value, read_only: bool, destructive: bool) -> Tool {
    let Value::Object(schema) = schema else { unreachable!() };
    let mut annotations = ToolAnnotations::default();
    annotations.read_only_hint = Some(read_only);
    annotations.destructive_hint = Some(destructive);
    annotations.open_world_hint = Some(false);
    let mut t = Tool::new(name, description.to_string(), Arc::new(schema));
    t.annotations = Some(annotations);
    t
}

/// Every tool, in a fixed order.
pub fn tools() -> Vec<Tool> {
    vec![
        tool(
            "list_editors",
            "List the caretline editors running on this machine that agents can attach to (started with `caretline FILE --listen`), newest first, with pid, file and socket, plus this server's open sessions.",
            json!({"type": "object", "properties": {}}),
            true,
            false,
        ),
        tool(
            "open",
            "Open a session. With no arguments, attach to the newest live editor; pid or socket picks one. With file (a path) or text, start a headless engine in this process instead (outline: read the file as Markdown blocks: lists, tasks, nesting). Returns a session id and the document's rev. Attaching never moves the person's caret or changes their text.",
            json!({
                "type": "object",
                "properties": {
                    "pid": {"type": "integer", "description": "Attach to the live editor with this process id (see list_editors)"},
                    "socket": {"type": "string", "description": "Attach to the editor or `caretline serve --socket` listening on this Unix socket"},
                    "file": {"type": "string", "description": "Headless: open this file (created on save if missing)"},
                    "text": {"type": "string", "description": "Headless: start from this text, with no file"},
                    "outline": {"type": "boolean", "description": "Headless: an outline document (Markdown lists, tasks and blocks)"},
                    "layout": {"type": "boolean", "description": "Headless: an outline with the outline layout when rendered"},
                    "width": {"type": "integer", "description": "Headless: the viewport width (default 80)"},
                    "height": {"type": "integer", "description": "Headless: the viewport height (default 24)"},
                },
            }),
            false,
            false,
        ),
        tool(
            "read",
            "Read the document: its text (all, or from_line..to_line), its rev, whether it is unsaved, and where the carets are (the person's and, after view_open, the agent's). Pass the rev to edit as if_rev. Lines come numbered (`N│ ` is not part of the text) unless numbered is false. With render, get the screen instead: a text frame at width x height of the agent's view (or the person's with view: person), as the editor draws it.",
            json!({
                "type": "object",
                "properties": {
                    "session": session_prop(),
                    "from_line": {"type": "integer", "minimum": 1, "description": "First line to return (1-based)"},
                    "to_line": {"type": "integer", "minimum": 1, "description": "Last line to return (inclusive)"},
                    "numbered": {"type": "boolean", "description": "Prefix lines with their numbers (default true)"},
                    "render": {"description": "true, or {width, height}: return the rendered frame instead of the text", "anyOf": [{"type": "boolean"}, {"type": "object", "properties": {"width": {"type": "integer"}, "height": {"type": "integer"}}}]},
                    "view": {"type": "string", "enum": ["agent", "person"], "description": "With render: whose view to draw (default: the agent's if open)"},
                },
            }),
            true,
            false,
        ),
        tool(
            "edit",
            "Change the document, guarded by if_rev (the rev from your last read or edit): if the text changed since, nothing is written and you get error stale with the current rev, who changed it and a diff. Read again and redo the edit. ops is a list of operations. Text operations, all resolved against the text you read and applied together as one change: {kind: replace, search, text} replaces the one place search occurs (an error if it occurs 0 or 2+ times: add surrounding text); {kind: replace_range, from, to, text} replaces [from, to) by line/col (text \"\" deletes); {kind: insert, at, text}. Caret operations, through your own view (view_open first in a live editor): {kind: select, from, to}; {kind: keys, keys} plays a key script through the editor's keymap: literal text types, <cr> <bs> <del> <tab> <left> <right> <up> <down> <home> <end> <a-left> (word) <s-right> (extend) <d-down> (document end) <c-z> (undo) <wait:MS>. Text and caret operations go in separate edits. The edit is attributed to this agent, never performs a save, and returns the new rev and a diff.",
            json!({
                "type": "object",
                "properties": {
                    "session": session_prop(),
                    "if_rev": {"type": "integer", "description": "The rev you last read. Required: the edit is refused if the text changed since"},
                    "ops": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "kind": {"type": "string", "enum": ["replace", "replace_range", "insert", "select", "keys"]},
                                "search": {"type": "string", "description": "replace: the exact text to find; it must occur exactly once"},
                                "text": {"type": "string", "description": "replace, replace_range, insert: the new text"},
                                "from": pos_schema("replace_range, select: the start"),
                                "to": pos_schema("replace_range, select: the end (exclusive)"),
                                "at": pos_schema("insert: where"),
                                "keys": {"type": "string", "description": "keys: a key script, e.g. \"<d-down><cr>Done.\""},
                            },
                            "required": ["kind"],
                        },
                    },
                    "undo": {"type": "string", "enum": ["shared", "outside"], "description": "shared (default): the edit is one step in the document's undo history and marks it unsaved. outside: applied as a change from elsewhere, which the person's undo skips (but which doesn't mark the document unsaved)"},
                },
                "required": ["if_rev", "ops"],
            }),
            false,
            true,
        ),
        tool(
            "view_open",
            "Open the agent's own view of the document: its own caret, selection and scroll, starting where the person's caret is. After this, select and keys move only the agent's caret and the person's stays where they left it; their edits move it only as text shifts.",
            json!({
                "type": "object",
                "properties": {
                    "session": session_prop(),
                    "width": {"type": "integer", "description": "The view's width (default 80)"},
                    "height": {"type": "integer", "description": "The view's height (default 24)"},
                },
            }),
            false,
            false,
        ),
        tool(
            "view_close",
            "Close the agent's own view.",
            json!({"type": "object", "properties": {"session": session_prop()}}),
            false,
            false,
        ),
        tool(
            "watch",
            "Wait for changes after since_rev and say who made them: the person at the keyboard, another client, or this agent (left out unless include_own). Returns when something changes (after a short pause so a typed word arrives whole) or after timeout_ms, with the actions grouped by who, a diff of the text, the person's caret and the new rev. Clock ticks and resizes are left out.",
            json!({
                "type": "object",
                "properties": {
                    "session": session_prop(),
                    "since_rev": {"type": "integer", "description": "The rev you last saw"},
                    "timeout_ms": {"type": "integer", "description": "How long to wait for a change (default 20000, at most 300000)"},
                    "settle_ms": {"type": "integer", "description": "After the first change, wait this long for more (default 400)"},
                    "include_own": {"type": "boolean", "description": "Also report this agent's own changes"},
                    "include_noise": {"type": "boolean", "description": "Also report clock ticks, frames and resizes"},
                },
                "required": ["since_rev"],
            }),
            true,
            false,
        ),
        tool(
            "trace",
            "Get the session's trace: a JSON Lines record of the state and every message applied, which `caretline --replay FILE` turns back into exactly the same document, carets and frame. By default the current segment (it replays on its own); since_rev gives only the lines after that rev; all gives everything kept. With path, write it to that file instead of returning it.",
            json!({
                "type": "object",
                "properties": {
                    "session": session_prop(),
                    "since_rev": {"type": "integer", "description": "Only the lines after this rev"},
                    "all": {"type": "boolean", "description": "Every line kept, from the editor's start"},
                    "path": {"type": "string", "description": "Write the trace to this file (.jsonl)"},
                },
            }),
            false,
            false,
        ),
        tool(
            "save",
            "Save the document to its file. Only call this when the user asked for the file to be saved: in a live editor it writes the person's file, as their Ctrl-S would.",
            json!({"type": "object", "properties": {"session": session_prop()}}),
            false,
            true,
        ),
        tool(
            "close",
            "Close a session: the agent's view closes and the server detaches. The live editor keeps running.",
            json!({"type": "object", "properties": {"session": session_prop()}}),
            false,
            false,
        ),
    ]
}

#[derive(Clone)]
pub struct Server {
    pub tools: Arc<Tools>,
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        let caps = ServerCapabilities::builder().enable_tools().build();
        ServerConfig::new(caps)
            .with_server_info(
                Implementation::new("caretline-mcp", env!("CARGO_PKG_VERSION"))
                    .with_title("caretline")
                    .with_description("Read and edit a live caretline editor alongside a person, or a headless engine: guarded writes, attribution, exact replay")
                    .with_website_url("https://caretline.app"),
            )
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(&self, _request: Option<PaginatedRequestParams>, _context: RequestContext<RoleServer>) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tools()))
    }

    async fn call_tool(&self, request: CallToolRequestParams, context: RequestContext<RoleServer>) -> Result<CallToolResponse, McpError> {
        if let Some(info) = context.peer.peer_info() {
            let mut name = self.tools.client_name.lock().unwrap();
            if name.is_none() {
                *name = Some(info.client_info.name.clone());
            }
        }
        let tools = self.tools.clone();
        let name = request.name.to_string();
        let args: Map<String, Value> = request.arguments.unwrap_or_default();
        let result = tokio::task::spawn_blocking(move || tools.call(&name, &args))
            .await
            .map_err(|e| McpError::internal_error(format!("the tool panicked: {e}"), None))?;
        let res = match result {
            Ok(out) => {
                let mut content = vec![Content::text(pretty(&out.data))];
                if let Some(t) = out.text {
                    content.push(Content::text(t));
                }
                let mut r = CallToolResult::success(content);
                r.structured_content = Some(out.data);
                r
            }
            Err(ToolError::Msg(m)) => {
                let data = json!({"error": "failed", "message": m});
                let mut r = CallToolResult::error(vec![Content::text(m)]);
                r.structured_content = Some(data);
                r
            }
            Err(ToolError::Data(d)) => {
                let mut r = CallToolResult::error(vec![Content::text(pretty(&d))]);
                r.structured_content = Some(d);
                r
            }
        };
        Ok(res.into())
    }
}
