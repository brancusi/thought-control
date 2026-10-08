//! The JSON contract: every command's real `--json` output validates against the schema that
//! `thc schema <cmd>` publishes. Keeps the schema (and generated ThoughtBar types) honest.

mod common;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn thc(vault: &Path, cache: &Path, args: &[&str]) -> (i32, String) {
    // Verbs past the default agent tier (policy.md §2.1) run as the human; the rest as an agent.
    let first = args.iter().find(|a| !a.starts_with("--")).copied().unwrap_or("");
    let human = matches!(first, "rm" | "restore" | "rewind")
        || (first == "review" && args.iter().any(|a| *a == "revert" || *a == "accept"))
        || (first == "conflict" && args.contains(&"resolve"));
    let out = common::thc()
        .args(args)
        .env("THC_VAULT", vault)
        .env("THC_CACHE_DIR", cache)
        .env("THC_CONFIG_DIR", vault.join("no-config"))
        .env("THC_ACTOR", if human { "human" } else { "agent:contract-test" })
        .output()
        .expect("run thc");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).to_string())
}

/// A small JSON Schema validator: $ref (local), anyOf, type, required, properties, items, enum, const.
fn validate(v: &Value, schema: &Value, root: &Value, path: &str, errs: &mut Vec<String>) {
    if let Some(any) = schema.get("anyOf").and_then(|a| a.as_array()) {
        let tries: Vec<Vec<String>> = any.iter().map(|s| {
            let mut e = Vec::new();
            validate(v, s, root, path, &mut e);
            e
        }).collect();
        if !tries.iter().any(|e| e.is_empty()) {
            errs.push(format!("{path}: matches no anyOf branch: {tries:?}"));
        }
        return;
    }
    if let Some(r) = schema.get("$ref").and_then(|r| r.as_str()) {
        let name = r.trim_start_matches("#/$defs/");
        let target = &root["$defs"][name];
        assert!(!target.is_null(), "dangling $ref {r}");
        return validate(v, target, root, path, errs);
    }
    if let Some(t) = schema.get("type") {
        let types: Vec<&str> = match t {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect(),
            _ => vec![],
        };
        let ok = types.iter().any(|t| match *t {
            "object" => v.is_object(),
            "array" => v.is_array(),
            "string" => v.is_string(),
            "integer" => v.is_i64() || v.is_u64(),
            "number" => v.is_number(),
            "boolean" => v.is_boolean(),
            "null" => v.is_null(),
            _ => true,
        });
        if !ok {
            errs.push(format!("{path}: expected {types:?}, got {v}"));
            return;
        }
    }
    if let Some(e) = schema.get("enum").and_then(|e| e.as_array()) {
        if !e.contains(v) {
            errs.push(format!("{path}: {v} not in {e:?}"));
        }
    }
    if let Some(c) = schema.get("const") {
        if c != v {
            errs.push(format!("{path}: {v} != const {c}"));
        }
    }
    if let (Some(obj), Some(req)) = (v.as_object(), schema.get("required").and_then(|r| r.as_array())) {
        for r in req.iter().filter_map(|r| r.as_str()) {
            if !obj.contains_key(r) {
                errs.push(format!("{path}: missing required \"{r}\""));
            }
        }
    }
    if let (Some(obj), Some(props)) = (v.as_object(), schema.get("properties").and_then(|p| p.as_object())) {
        for (k, sub) in props {
            if let Some(x) = obj.get(k) {
                validate(x, sub, root, &format!("{path}.{k}"), errs);
            }
        }
    }
    if let (Some(arr), Some(items)) = (v.as_array(), schema.get("items")) {
        for (i, x) in arr.iter().enumerate() {
            validate(x, items, root, &format!("{path}[{i}]"), errs);
        }
    }
}

fn check(vault: &Path, cache: &Path, cmd: &str, args: &[&str], errs: &mut Vec<String>) -> Value {
    let (_, schema_out) = thc(vault, cache, &["schema", cmd]);
    let schema: Value = serde_json::from_str(&schema_out).unwrap_or_else(|e| panic!("schema {cmd}: {e}"));
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let (_, out) = thc(vault, cache, &full);
    let line = out.lines().last().unwrap_or("");
    let v: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{args:?} printed non-JSON ({e}): {out}"));
    validate(&v, &schema["output"], &schema, &format!("{}", args.join(" ")), errs);
    v
}

fn tmp() -> PathBuf {
    let d = thc_core::scratch::dir(&format!("thc-contract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_validator_catches_violations() {
    let schema = serde_json::json!({ "$defs": { "N": { "type": "object", "required": ["id"], "properties": { "status": { "enum": ["todo", "done"] } } } }, "output": { "$ref": "#/$defs/N" } });
    let mut errs = Vec::new();
    validate(&serde_json::json!({ "status": "nope" }), &schema["output"], &schema, "x", &mut errs);
    assert_eq!(errs.len(), 2, "{errs:?}");
}

#[test]
fn proto_schema_covers_the_socket() {
    let out = common::thc().args(["schema", "--proto"]).output().unwrap();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    for m in ["hello", "status", "today", "query", "capture", "complete", "set", "snooze", "ack", "undo", "alert.delivered", "shutdown"] {
        assert!(v["methods"][m].is_object(), "method {m} missing");
    }
    for e in ["changed", "conflict", "alert.fire", "alert.withdraw"] {
        assert!(v["events"][e].is_object(), "event {e} missing");
    }
    assert!(v["$defs"]["Delivery"].is_object() && v["$defs"]["Notification"].is_object());
}

#[test]
fn every_command_matches_its_published_schema() {
    let root = tmp();
    let (vault, cache) = (root.join("vault"), root.join("cache"));
    let init = common::thc().current_dir(&root).args(["init", "vault"]).output().unwrap();
    assert!(init.status.success());
    let mut errs = Vec::new();
    let e = &mut errs;
    let added = check(&vault, &cache, "add", &["add", "Contract note #test [[Contract Page]]"], e);
    let note = added["nodes"][0]["id"].as_str().unwrap().to_string();
    let todo = check(&vault, &cache, "todo", &["todo", "Contract task", "--due", "tomorrow", "-p", "high", "--repeat", "every week"], e);
    let task = todo["nodes"][0]["id"].as_str().unwrap().to_string();
    check(&vault, &cache, "todo", &["todo", "Keyed task", "--key", "contract-1"], e);
    check(&vault, &cache, "todo", &["todo", "Keyed task", "--key", "contract-1"], e); // idempotent repeat
    check(&vault, &cache, "remind", &["remind", "Contract reminder", "--at", "+2h"], e);
    check(&vault, &cache, "set", &["set", &task, "client=acme"], e);
    check(&vault, &cache, "text", &["text", &note, "Contract note edited #test"], e);
    check(&vault, &cache, "tag", &["tag", &note, "+extra"], e);
    check(&vault, &cache, "link", &["link", &note, &task, "--rel", "blocks"], e);
    check(&vault, &cache, "done", &["done", &task], e);
    check(&vault, &cache, "show", &["show", &task], e);
    check(&vault, &cache, "today", &["today"], e);
    check(&vault, &cache, "agenda", &["agenda"], e);
    check(&vault, &cache, "inbox", &["inbox"], e);
    check(&vault, &cache, "pages", &["pages"], e);
    check(&vault, &cache, "search", &["search", "contract"], e);
    check(&vault, &cache, "board", &["board", "--board", vault.to_str().unwrap()], e);
    check(&vault, &cache, "prime", &["prime", "--role", "engineer", "--board", vault.to_str().unwrap()], e);
    check(&vault, &cache, "role", &["role", "show", "engineer"], e);
    check(&vault, &cache, "msg", &["msg", "engineer", "watch schema", "--board", vault.to_str().unwrap()], e);
    check(&vault, &cache, "watch", &["watch", "--for", "engineer", "--once", "--board", vault.to_str().unwrap()], e);
    check(&vault, &cache, "next", &["next", "--board", vault.to_str().unwrap()], e);
    check(&vault, &cache, "status", &["status", "--board", vault.to_str().unwrap(), "--since", "all"], e);
    check(&vault, &cache, "q", &["q", "is:task"], e);
    check(&vault, &cache, "q", &["q", "status:open (due<=+3d or !high) -#x sort:due", "--explain"], e);
    check(&vault, &cache, "q", &["q", "status:any group:tag"], e);
    check(&vault, &cache, "journal", &["journal"], e);
    check(&vault, &cache, "history", &["history", &task], e);
    check(&vault, &cache, "log", &["log"], e);
    check(&vault, &cache, "alert", &["alert", "ls"], e);
    check(&vault, &cache, "conflict", &["conflict", "ls"], e);
    check(&vault, &cache, "doctor", &["doctor"], e);
    check(&vault, &cache, "diff", &["diff", &task, "--since", "1h"], e);
    check(&vault, &cache, "diff", &["diff", &task, "--as-of", "today"], e);
    check(&vault, &cache, "review", &["review"], e);
    check(&vault, &cache, "review", &["review", "show", "1"], e);
    check(&vault, &cache, "review", &["review", "revert", "1"], e);
    check(&vault, &cache, "rewind", &["rewind", &task, "--to", "today", "--yes"], e);
    check(&vault, &cache, "view", &["view", "ls"], e);
    check(&vault, &cache, "view", &["view", "add", "contract", "status:open"], e);
    check(&vault, &cache, "view", &["view", "rm", "contract"], e);
    check(&vault, &cache, "unlink", &["unlink", &note, &task], e);
    check(&vault, &cache, "undo", &["undo"], e);
    check(&vault, &cache, "rm", &["rm", &note], e);
    // The daemon's TodayPanel (what ThoughtBar decodes) validates against the proto schema.
    {
        let (_, proto) = thc(&vault, &cache, &["schema", "--proto"]);
        let proto: Value = serde_json::from_str(&proto).unwrap();
        let (_, out) = thc(&vault, &cache, &["today", "--panel", "--json"]);
        let v: Value = serde_json::from_str(&out).unwrap();
        validate(&v, &proto["$defs"]["TodayPanel"], &proto, "today --panel", e);
    }
    // `--fields` output still validates (fields are optional unless required on Node).
    let (code, _) = thc(&vault, &cache, &["daemon", "status", "--json"]);
    assert_eq!(code, 3, "daemon status exits 3 when offline");
    let _ = std::fs::remove_dir_all(&root);
    assert!(errs.is_empty(), "schema violations:\n{}", errs.join("\n"));
}

#[test]
fn repo_agent_files_match_the_binary() {
    // AGENTS.md Part 1 and the skill are generated by `thc setup claude`; this fails on drift.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = common::thc().current_dir(&root).args(["setup", "claude", "--check"]).output().unwrap();
    assert!(out.status.success(), "stale agent files — run `thc setup claude --yes` in the repo root:\n{}", String::from_utf8_lossy(&out.stdout));
}
