//! `thc schema`: JSON Schema (draft 2020-12) for each command's input and output.
//!
//! Inputs are generated from the clap definitions, so they can't drift from the binary.
//! Outputs reference shared `$defs` (Node, Alert, Conflict, …). The stability promise is in
//! AGENTS.md: fields are only ever added, never removed or retyped, within a major version.

use serde_json::{Map, Value, json};

/// Every field a node object can carry in JSON output (for `--fields` discovery).
pub const NODE_FIELDS: &[(&str, &str)] = &[
    ("id", "full 12-char id"),
    ("short", "shortest unique prefix (display)"),
    ("rev", "eid of the node's last change (for --if-match)"),
    ("kind", "task | page | journal | inbox | block | tag"),
    ("parent", "parent node id, absent for roots"),
    ("title", "page title"),
    ("text", "node text with [[links]] rendered as titles"),
    ("status", "todo | doing | waiting | done | cancelled"),
    ("scheduled", "planned date, YYYY-MM-DD or YYYY-MM-DDTHH:MM"),
    ("due", "hard deadline, same format"),
    ("priority", "high | med | low"),
    ("repeat", "{rule, mode, text}"),
    ("done_at", "completion time"),
    ("journal", "date, for journal day nodes"),
    ("tags", "tag names"),
    ("props", "custom properties"),
    ("vault", "the vault's name (vaults.md)"),
    ("created_by", "human | agent:<name>"),
    ("created", "local time"),
    ("updated", "local time"),
    ("deleted", "true when deleted (only with is:deleted)"),
    ("conflicts", "open sync conflicts (show)"),
    ("children", "child nodes (show, journal)"),
    ("backlinks", "nodes linking here (show)"),
    ("alerts", "alerts on the node (show)"),
    ("relations", "typed links [{rel, dst}] (show)"),
    ("ocr", "on-device image text {hash, engine, text} (show)"),
    ("image_matches", "images responsible for a search match [{id, caption, snippet}]"),
];

fn board_schema() -> Value {
    json!({ "type": "object", "required": ["vault", "path", "page", "source"], "properties": {
        "vault": { "type": "string" }, "path": { "type": "string" }, "source": { "enum": ["flag", "env", "project", "capture"] }, "source_file": { "type": "string" },
        "page": { "type": ["object", "null"], "required": ["id", "title"], "properties": { "id": { "type": "string" }, "title": { "type": "string" } } }
    } })
}

fn defs() -> Value {
    let date = json!({ "type": "string", "pattern": "^\\d{4}-\\d{2}-\\d{2}(T\\d{2}:\\d{2})?$" });
    let fields = json!({ "type": "object", "description": "{from?, to?} for values, {add?, remove?} for sets; additive" });
    json!({
        "ReviewItem": {
            "type": "object",
            "required": ["n", "tx", "short", "actor", "via", "dev", "at", "later_changed_by_human", "conflict", "changes"],
            "properties": {
                "n": { "type": "integer", "description": "position in this listing (valid 10 minutes)" },
                "tx": { "type": "string" },
                "short": { "type": "string" },
                "actor": { "type": "string" },
                "via": { "type": "string" },
                "dev": { "type": "string" },
                "at": { "type": "string" },
                "later_changed_by_human": { "type": "boolean" },
                "later_changed": { "type": "array", "items": { "type": "object", "properties": { "node": { "type": "string" }, "field": { "type": "string" }, "actor": { "type": "string" }, "at": { "type": "string" } } } },
                "conflict": { "type": "boolean" },
                "verdict": { "enum": ["accepted", "reverted"] },
                "changes": { "type": "array", "items": {
                    "type": "object",
                    "required": ["node", "short", "change", "text", "fields"],
                    "properties": {
                        "node": { "type": "string" },
                        "short": { "type": "string" },
                        "change": { "enum": ["create", "change", "move", "complete", "delete", "restore", "alert"] },
                        "text": { "type": "string" },
                        "fields": fields,
                        "children": { "type": "integer" }
                    }
                } },
                "events": { "type": "array", "items": { "$ref": "#/$defs/Event" } }
            }
        },
        "View": {
            "type": "object", "required": ["id", "name", "query", "bar"],
            "properties": {
                "id": { "type": "string" }, "name": { "type": "string" }, "query": { "type": "string" },
                "title": { "type": "string" }, "tasks": { "type": "integer" }, "bar": { "type": "boolean" },
                "capture": { "type": "string" }, "count": { "type": ["integer", "null"] }
            }
        },
        "Skipped": {
            "type": "object",
            "properties": { "tx": { "type": "string" }, "node": { "type": "string" }, "short": { "type": "string" }, "field": { "type": "string" }, "reason": { "enum": ["changed_later", "deleted"] }, "actor": { "type": "string" }, "at": { "type": "string" } }
        },
        "Node": {
            "type": "object",
            "required": ["id", "short", "kind", "text", "tags", "created_by", "created", "updated"],
            "properties": {
                "id": { "type": "string", "pattern": "^[0-9a-hjkmnp-tv-z]{12}$" },
                "short": { "type": "string" },
                "rev": { "type": "string", "description": "eid of the node's last change; pass to --if-match" },
                "reasons": { "type": "array", "description": "today/agenda rows only: why the row is there (views.md §3.3)", "items": { "enum": ["overdue", "due-today", "scheduled", "alert", "alert-fired", "repeating", "doing", "done-today"] } },
                "place": { "type": "string", "description": "where it lives, in words: `§ 2026-10-03`, `¶ Health`, a parent's text, `inbox` (daemon)" },
                "kind": { "enum": ["task", "page", "journal", "inbox", "block", "tag"] },
                "parent": { "type": "string" },
                "title": { "type": "string" },
                "text": { "type": "string" },
                "status": { "enum": ["todo", "doing", "waiting", "done", "cancelled"] },
                "scheduled": date, "due": date,
                "priority": { "enum": ["high", "med", "low"] },
                "repeat": { "type": "object", "properties": { "rule": { "type": "string" }, "mode": { "enum": ["fixed", "catch_up", "from_done"] }, "text": { "type": "string" } } },
                "done_at": { "type": "string" },
                "journal": { "type": "string" },
                "tags": { "type": "array", "items": { "type": "string" } },
                "props": { "type": "object" },
                "created_by": { "type": "string" },
                "vault": { "type": "string" },
                "created": { "type": "string" }, "updated": { "type": "string" },
                "deleted": { "type": "boolean" },
                "children": { "type": "array", "items": { "$ref": "#/$defs/Node" } },
                "backlinks": { "type": "array", "items": { "type": "object", "properties": { "id": { "type": "string" }, "label": { "type": "string" } } } },
                "alerts": { "type": "array", "items": { "$ref": "#/$defs/Alert" } },
                "conflicts": { "type": "array", "items": { "$ref": "#/$defs/Conflict" } },
                "relations": { "type": "array", "items": { "type": "object", "properties": { "rel": { "type": "string" }, "dst": { "type": "string" } } } }
            }
        },
        "Alert": {
            "type": "object",
            "required": ["id", "node", "state"],
            "properties": {
                "id": { "type": "string" }, "node": { "type": "string" },
                "at": { "type": ["string", "null"] }, "offset": { "type": ["string", "null"] },
                "anchor": { "type": ["string", "null"], "enum": ["due", "scheduled", null] },
                "state": { "enum": ["pending", "fired", "snoozed", "acked"] },
                "fire_at": { "type": ["string", "null"] }, "fired_at": { "type": ["string", "null"] }
            }
        },
        "ConflictVersion": {
            "type": "object",
            "properties": { "text": { "type": "string" }, "actor": { "type": "string" }, "dev": { "type": "string" }, "at": { "type": "string" }, "eid": { "type": ["string", "null"] } }
        },
        "Conflict": {
            "type": "object",
            "required": ["node", "kind"],
            "properties": {
                "node": { "type": "string" }, "kind": { "enum": ["text", "move", "rehomed"] }, "since": { "type": ["string", "null"] },
                "current": { "$ref": "#/$defs/ConflictVersion" }, "other": { "$ref": "#/$defs/ConflictVersion" },
                "base": { "type": ["string", "null"] },
                "deleted_parent": { "type": "object", "properties": { "id": { "type": "string" }, "title": { "type": ["string", "null"] }, "device": { "type": "string" }, "at": { "type": "string" } } },
                "now_under": { "type": "object", "properties": { "id": { "type": ["string", "null"] }, "title": { "type": ["string", "null"] } } }
            }
        },
        "ListContext": {
            "type": "object", "required": ["applied"],
            "description": "the context (views.md §2) on a listing a context can filter; always present on CLI listings, absent from daemon replies (the daemon never applies one)",
            "properties": {
                "name": { "type": ["string", "null"], "description": "the resolved context, else this device's setting" },
                "applied": { "type": "boolean", "description": "true when it filtered these results (--context or THC_CONTEXT for JSON)" },
                "hidden": { "type": "integer", "description": "how many results it removed (only when applied)" },
                "device": { "type": ["string", "null"], "description": "this device's setting" }
            }
        },
        "QueryExplain": {
            "type": "object", "required": ["query", "means", "sql", "matches", "ms"],
            "description": "thc q --explain (views.md §3.1): the query in plain words, relative dates resolved",
            "properties": {
                "query": { "type": "string" },
                "means": { "type": "array", "items": { "type": "string" }, "description": "one line per top-level term, then the sort" },
                "short": { "type": "string", "description": "the meaning on one line (the TUI's)" },
                "ast": { "type": ["object", "null"], "description": "{op: and|or, terms[]} · {op: not, term} · {op: term, text, means, short}" },
                "sorts": { "type": "array", "items": { "type": "string" } },
                "views": { "type": "array", "items": { "type": "object", "properties": { "name": { "type": "string" }, "query": { "type": "string" } } } },
                "sql": { "type": "string", "description": "the WHERE and ORDER BY with parameters inlined, for reading" },
                "matches": { "type": "integer" },
                "ms": { "type": "number" },
                "context": { "$ref": "#/$defs/ListContext" }
            }
        },
        "QueryGroups": {
            "type": "object", "required": ["group", "count", "groups"],
            "description": "thc q '… group:<by>' (views.md §3.2): the same matches, laid out in groups",
            "properties": {
                "group": { "enum": ["parent", "tag", "status", "due", "actor"] },
                "count": { "type": "integer", "description": "distinct matches (group:tag can list a node more than once)" },
                "groups": { "type": "array", "items": { "type": "object", "required": ["key", "label", "count", "items"], "properties": {
                    "key": { "type": "string", "description": "a node id (parent), or the bucket: overdue/earlier/today/tomorrow/week/later/none, a tag, a status, human/agent:<name>" },
                    "label": { "type": "string" },
                    "count": { "type": "integer" },
                    "items": { "type": "array", "items": { "$ref": "#/$defs/Node" } } } } },
                "repeated": { "type": "boolean", "description": "group:tag listed some node under several tags" },
                "context": { "$ref": "#/$defs/ListContext" }
            }
        },
        "List": {
            "type": "object", "required": ["count", "items"],
            "properties": { "count": { "type": "integer" }, "items": { "type": "array", "items": { "$ref": "#/$defs/Node" } }, "context": { "$ref": "#/$defs/ListContext" } }
        },
        "Write": {
            "type": "object", "required": ["ok", "events"],
            "properties": { "ok": { "const": true }, "tx": { "type": ["string", "null"] }, "events": { "type": "integer" }, "nodes": { "type": "array", "items": { "$ref": "#/$defs/Node" } } }
        },
        "Today": {
            "type": "object",
            "properties": {
                "date": { "type": "string" },
                "overdue": { "type": "array", "items": { "$ref": "#/$defs/Node" } },
                "today": { "type": "array", "items": { "$ref": "#/$defs/Node" } },
                "done": { "type": "array", "items": { "$ref": "#/$defs/Node" } },
                "alerts": { "type": "array", "items": { "$ref": "#/$defs/Alert" } },
                "journal_entries": { "type": "integer" },
                "context": { "$ref": "#/$defs/ListContext" }
            }
        },
        "Agenda": {
            "type": "object",
            "properties": {
                "overdue": { "type": "array", "items": { "$ref": "#/$defs/Node" } },
                "days": { "type": "array", "items": { "type": "object", "properties": { "date": { "type": "string" }, "items": { "type": "array", "items": { "$ref": "#/$defs/Node" } } } } },
                "context": { "$ref": "#/$defs/ListContext" }
            }
        },
        "Event": {
            "type": "object",
            "properties": { "eid": { "type": "string" }, "ms": { "type": "integer" }, "dev": { "type": "string" }, "actor": { "type": "string" }, "via": { "type": "string" }, "tx": { "type": "string" }, "op": { "type": "string" }, "entity": { "type": "string" }, "body": { "type": "object" } }
        },
        "DaemonStatus": {
            "type": "object", "required": ["state"],
            "properties": {
                "state": { "enum": ["live", "offline"] }, "pid": { "type": "integer" }, "version": { "type": "string" }, "proto": { "type": "integer" },
                "socket": { "type": "string", "description": "unix socket path (live only)" }, "exe": { "type": "string", "description": "the running daemon's binary" }, "exe_id": { "type": "string", "description": "dev:inode of the binary the daemon started from (differs from the file at exe once an install replaced it)" }, "started_ms": { "type": "integer" }, "stale": { "type": "string", "description": "why the daemon isn't on the binary its login item (or this thc) runs; absent when current" }, "vault": { "type": "string" }, "device": { "type": "string" },
                "uptime_s": { "type": "integer" }, "notify": { "type": "string" }, "alerts": { "type": "object" }, "install": { "type": "object" }, "devices": {}, "last_change": {},
                "vault_source": { "$ref": "#/$defs/VaultSource" }
            }
        },
        "VaultSource": {
            "type": "object", "required": ["kind", "text"],
            "description": "why this vault: --vault, THC_VAULT, a .thc.toml, inside the vault folder, or the global config",
            "properties": {
                "kind": { "enum": ["flag", "env", "project", "inside", "global"] },
                "file": { "type": "string", "description": "the .thc.toml or config.toml (project, global)" },
                "text": { "type": "string", "description": "\"from ~/.config/thought/config.toml\"" }
            }
        },
        "Error": {
            "type": "object", "required": ["error"],
            "properties": { "error": { "type": "object", "required": ["kind", "message"], "properties": {
                "kind": { "enum": ["error", "usage", "not_found", "conflict", "ambiguous", "validation"] },
                "message": { "type": "string" }, "candidates": { "type": "array", "items": { "type": "string" } } } } }
        }
    })
}

/// CLI-only enrichment. The daemon does not emit OCR fields, so its shared types stay stable.
fn cli_defs() -> Value {
    let mut value = defs();
    value["Node"]["properties"]["ocr"] = json!({ "type": "object", "required": ["hash", "engine", "text"], "properties": { "hash": { "type": "string" }, "engine": { "type": "string" }, "text": { "type": "string" } } });
    value["Node"]["properties"]["image_matches"] = json!({ "type": "array", "items": { "type": "object", "required": ["id", "snippet"], "properties": { "id": { "type": "string" }, "caption": { "type": ["string", "null"] }, "snippet": { "type": "string" } } } });
    value
}

fn output_for(cmd: &str) -> Value {
    let r = |d: &str| json!({ "$ref": format!("#/$defs/{d}") });
    match cmd {
        "msg" | "add" | "todo" | "remind" | "done" | "reopen" | "skip" | "set" | "text" | "tag" | "mv" | "restore" | "page" => r("Write"),
        "link" | "unlink" => json!({ "type": "object", "properties": { "ok": { "const": true }, "tx": { "type": ["string", "null"] }, "changed": { "type": "integer" } } }),
        "rm" => json!({ "type": "object", "properties": { "ok": { "const": true }, "tx": { "type": ["string", "null"] }, "deleted": { "type": "integer" } } }),
        "show" => r("Node"),
        "ocr" => json!({ "type": "object", "properties": { "ok": { "type": "boolean" }, "dry_run": { "type": "boolean" }, "available": { "type": "boolean" }, "pending": { "type": "integer" }, "recognized": { "type": "integer" }, "tx": { "type": ["string", "null"] }, "images": { "type": "array", "items": { "type": "object" } }, "skipped": { "type": "array", "items": { "type": "object", "properties": { "id": { "type": "string" }, "path": { "type": "string" }, "reason": { "type": "string" } } } } } }),
        "today" => r("Today"),
        "agenda" => r("Agenda"),
        "inbox" | "pages" | "search" => r("List"),
        "board" => board_schema(),
        "team" => json!({"type":"object","properties":{
            "ok":{"type":"boolean"},"dry_run":{"type":"boolean"},"project":{"type":"string"},
            "board":board_schema(),"host":{"enum":["herdr","wezterm","printed"]},
            "closed":{"type":"integer"},"model_passed":{"type":"boolean"},
            "layout":{"type":["object","null"],"properties":{"columns":{"type":"integer"},"tab":{"type":"string"},"pm":{"type":["string","null"]}}},
            "member":{"type":"object"},"removed":{"type":"string"},"command":{"type":"string"},
            "agents":{"type":"array"},"claims":{"type":"array"},
            "team":{"type":"array","items":{"type":"object","properties":{
                "role":{"type":"string"},"actor":{"type":"string"},"agent":{"type":"string"},
                "model":{"type":["string","null"]},"pane":{"type":["string","null"]},
                "added_by":{"type":"string"},"task":{"type":["string","null"]},
                "state":{"type":"string"},"unread":{"type":"integer"},"claims":{"type":"array"},
                "last_activity":{"type":["string","null"]}
            }}},"commands":{"type":"array"},"new_board":{"type":"string"},"roles":{"type":"array"}
        }}),
        "role" => json!({ "type": "object", "properties": { "role": { "type": "string" }, "card": { "type": "string" } } }),
        "prime" => json!({ "type": "object", "properties": {
            "role": { "type": "string" }, "actor": { "type": ["string", "null"] }, "board": board_schema(),
            "card": { "type": "object", "properties": { "builtin": { "type": "string" }, "project": { "type": "array", "items": { "type": "string" } } } },
            "next": { "type": "array", "items": { "$ref": "#/$defs/Node" } }, "claim": { "type": ["string", "null"] },
            "messages": { "type": "array" }, "team": { "type": "array" }, "waiting":{"type":["string","null"]},"first_plan":{"type":["string","null"]}, "rules": { "type": "array", "items": { "type": "string" } }, "attribution": { "type": "object" }
        } }),
        "msgs" => r("List"),
        "watch" => json!({ "type": "object", "description": "One JSON object per line; read-only stream, including initial/reconnect reconciliation.",
            "required": ["event", "node", "board", "source"], "properties": {
                "event": { "enum": ["message", "task"] }, "node": r("Node"), "board": board_schema(),
                "source": { "enum": ["daemon", "poll", "reconcile"] },
                "change": { "type": "object", "description": "Triggering daemon changed event; reconciliation can also emit other affected items.", "properties": {
                    "ids": { "type": "array", "items": { "type": "string" } }, "tx": { "type": "string" }, "actor": { "type": "string" }, "dev": { "type": "string" }
                } }
            }
        }),
        "next" => json!({ "type": "object", "required": ["queue", "count", "items", "claim"], "properties": {
            "queue": { "type": "object", "required": ["id", "title"], "properties": {
                "id": { "type": ["string", "null"] }, "title": { "type": ["string", "null"] }
            } },
            "count": { "type": "integer" }, "items": { "type": "array", "items": { "$ref": "#/$defs/Node" } },
            "claim": { "type": ["string","null"] },"waiting":{"type":["string","null"]}
        } }),
        "status" => {
            let n = || json!({ "type": "integer" });
            let s = || json!({ "type": "string" });
            let tokens = json!({ "type": "object", "required": ["in", "out", "cache"], "properties": { "in": n(), "out": n(), "cache": n(), "source": { "enum": ["transcript", "self"] }, "sessions": { "type": "array", "items": s() } } });
            let totals = json!({ "type": "object", "required": ["in", "out", "cache", "known", "of"], "properties": { "in": n(), "out": n(), "cache": n(), "known": n(), "of": n() } });
            json!({ "type": "object", "required": ["board", "range", "counts", "timing", "momentum", "tokens", "actors", "tasks", "blocked"], "properties": {
                "board": { "type": "object", "required": ["vault"], "properties": { "vault": s(), "page": { "type": ["string", "null"] }, "id": { "type": ["string", "null"] } } },
                "range": { "type": "object", "required": ["label", "to"], "properties": { "label": s(), "from": s(), "to": s() } },
                "counts": { "type": "object", "properties": { "done": n(), "done_today": n(), "doing": n(), "ready": n(), "todo": n(), "waiting": n(), "blocked": n(), "to_review": n(), "untracked": n(), "batch": n() } },
                "timing": { "type": "object", "required": ["sample"], "properties": { "worked_median": n(), "worked_p90": n(), "waited_median": n(), "lead_median": n(), "sample": n() } },
                "momentum": { "type": "object", "properties": {
                    "days": { "type": "array", "items": { "type": "object", "required": ["date", "done", "started"], "properties": { "date": s(), "done": n(), "started": n() } } },
                    "today": { "type": "array", "items": { "type": "object", "required": ["hour", "done"], "properties": { "hour": n(), "done": n() } } }
                } },
                "tokens": totals,
                "actors": { "type": "array", "items": { "type": "object", "required": ["actor", "done", "doing", "tokens"], "properties": { "actor": s(), "done": n(), "doing": n(), "worked_median": n(), "tokens": totals } } },
                "tasks": { "type": "array", "items": { "type": "object", "required": ["id", "short", "title", "status", "filed", "reopened"], "properties": {
                    "id": s(), "short": s(), "title": s(), "status": s(), "owner": s(), "role": s(), "priority": s(),
                    "filed": s(), "started": s(), "done": s(), "reviewed": s(), "worked": n(), "waited": n(), "lead": n(),
                    "untracked": { "type": "boolean" }, "batch": { "type": "boolean" }, "long": { "type": "boolean" }, "reopened": n(), "tokens": tokens
                } } },
                "blocked": { "type": "array", "items": { "type": "object", "required": ["id", "short", "title", "status"], "properties": {
                    "id": s(), "short": s(), "title": s(), "status": s(), "owner": s(),
                    "holds_up": { "type": "array", "items": s() }, "direct": { "type": "array", "items": s() }, "blocked_by": { "type": "array", "items": s() }
                } } }
            } })
        }
        "q" => json!({ "anyOf": [r("List"), r("QueryExplain"), r("QueryGroups")] }),
        "journal" => json!({ "type": "object", "properties": { "context": { "$ref": "#/$defs/ListContext" }, "journal": { "type": "string" }, "id": { "type": ["string", "null"] }, "items": { "type": "array", "items": { "$ref": "#/$defs/Node" } } } }),
        "history" => json!({ "type": "object", "properties": { "id": { "type": "string" }, "events": { "type": "array", "items": { "$ref": "#/$defs/Event" } } } }),
        "log" => json!({ "type": "object", "properties": { "events": { "type": "array", "items": { "$ref": "#/$defs/Event" } } } }),
        "undo" => json!({ "type": "object", "properties": {
            "ok": { "const": true }, "undone": { "type": "string" }, "tx": { "type": ["string", "null"] }, "events": { "type": "integer" },
            "reverted": { "type": "array", "items": { "type": "string" } }, "skipped": { "type": "array", "items": { "$ref": "#/$defs/Skipped" } },
            "pending": { "type": "integer" } } }),
        "review" => json!({ "type": "object", "description": "listing: {pending, items}; show: one ReviewItem with events; accept/revert: {ok, tx, accepted|reverted, skipped?, pending}",
            "properties": {
                "pending": { "type": "integer" },
                "items": { "type": "array", "items": { "$ref": "#/$defs/ReviewItem" } },
                "ok": { "const": true },
                "tx": { "type": ["string", "null"] },
                "accepted": { "type": "array", "items": { "type": "string" } },
                "reverted": { "type": "array", "items": { "type": "string" } },
                "skipped": { "type": "array", "items": { "$ref": "#/$defs/Skipped" } }
            } }),
        "diff" => json!({ "type": "object", "required": ["node", "short", "from", "fields", "txs"], "properties": {
            "node": { "type": "string" }, "short": { "type": "string" },
            "from": { "type": "object", "properties": { "at": { "type": "string" }, "tx": { "type": "string" } } },
            "fields": { "type": "object", "description": "by display key (text, status, due, place, tags, links, props…): {from?, to?} or {add?, remove?}, plus by, at, tx and edits (>1)" },
            "change": { "type": ["string", "null"] },
            "children": { "type": "object", "properties": { "add": { "type": "integer" }, "remove": { "type": "integer" } } },
            "deleted": { "type": "object", "properties": { "at": { "type": "string" }, "by": { "type": "string" } } },
            "created": { "type": "object", "properties": { "at": { "type": "string" }, "by": { "type": "string" } } },
            "txs": { "type": "array", "items": { "type": "string" } } } }),
        "apply" => json!({ "type": "object", "description": "dry run: {dry_run, ok, ops, lines, events}; success: {ok, tx, ops, events, lines, nodes}; invalid (exit 6): {ok:false, ops, invalid, errors[{line, cmd, error{kind, message}}]}",
            "properties": {
                "ok": { "type": "boolean" }, "dry_run": { "type": "boolean" }, "tx": { "type": ["string", "null"] },
                "ops": { "type": "integer" }, "invalid": { "type": "integer" },
                "events": { "type": ["integer", "array"] },
                "lines": { "type": "array", "items": { "type": "object", "properties": { "line": { "type": "integer" }, "cmd": { "type": "string" }, "nodes": { "type": "array", "items": { "type": "string" } } } } },
                "errors": { "type": "array", "items": { "type": "object", "properties": { "line": { "type": "integer" }, "cmd": { "type": "string" }, "error": { "type": "object" } } } },
                "nodes": { "type": "array", "items": { "$ref": "#/$defs/Node" } }
            } }),
        "import" => json!({ "type": "object", "properties": { "ok": { "const": true }, "tx": { "type": ["string", "null"] }, "created": { "type": "integer" }, "root": { "type": "string" }, "events": { "type": "integer" } } }),
        "view" => json!({ "type": "object", "description": "ls: {views[]}; add/set: {ok, tx, view}; rm: {ok, tx, removed}",
            "properties": {
                "views": { "type": "array", "items": { "$ref": "#/$defs/View" } },
                "view": { "$ref": "#/$defs/View" },
                "ok": { "const": true }, "tx": { "type": ["string", "null"] }, "removed": { "type": "string" }
            } }),
        "rewind" => json!({ "type": "object", "properties": { "ok": { "const": true }, "tx": { "type": ["string", "null"] }, "fields": { "type": "integer" }, "to": { "type": "string" } } }),
        "alert" => json!({ "type": "object", "properties": { "alerts": { "type": "array", "items": { "$ref": "#/$defs/Alert" } }, "alert": { "$ref": "#/$defs/Alert" } } }),
        "conflict" => json!({ "type": "object", "properties": { "conflicts": { "type": "array", "items": { "$ref": "#/$defs/Conflict" } } } }),
        "daemon" => r("DaemonStatus"),
        "doctor" => json!({ "type": "object", "properties": { "issues": { "type": "array", "items": { "type": "string" } }, "notices": { "type": "array", "items": { "type": "string" } }, "daemon": { "type": "object", "description": "the vault's daemon: state, version, exe, manager, and stale (why it isn't on the expected binary) when it isn't" } } }),
        _ => json!({ "type": "object" }),
    }
}

/// Input schema from clap: one property per argument, with types and help text.
fn input_for(cmd: &clap::Command) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for a in cmd.get_arguments() {
        let id = a.get_id().as_str();
        // Root globals (--json, --vault, …) are documented once, not per command.
        if matches!(id, "help" | "version" | "json" | "board" | "vault" | "actor" | "dry_run" | "limit" | "fields" | "if_match" | "expect") {
            continue;
        }
        let multiple = matches!(a.get_num_args(), Some(r) if r.max_values() > 1);
        let flag = matches!(a.get_action(), clap::ArgAction::SetTrue | clap::ArgAction::SetFalse);
        let mut p = Map::new();
        if flag {
            p.insert("type".into(), json!("boolean"));
        } else if multiple || matches!(a.get_action(), clap::ArgAction::Append) {
            p.insert("type".into(), json!("array"));
            p.insert("items".into(), json!({ "type": "string" }));
        } else {
            p.insert("type".into(), json!("string"));
        }
        let vals: Vec<String> = a.get_possible_values().iter().map(|v| v.get_name().to_string()).collect();
        if !vals.is_empty() {
            p.insert("enum".into(), json!(vals));
        }
        if let Some(h) = a.get_help() {
            p.insert("description".into(), json!(h.to_string()));
        }
        if let Some(d) = a.get_default_values().first() {
            p.insert("default".into(), json!(d.to_string_lossy()));
        }
        if a.is_positional() {
            p.insert("positional".into(), json!(true));
        }
        if a.is_required_set() {
            required.push(id.to_string());
        }
        props.insert(id.replace('_', "-"), Value::Object(p));
    }
    let mut subs = Map::new();
    for s in cmd.get_subcommands() {
        subs.insert(s.get_name().to_string(), input_for(s));
    }
    let mut v = json!({ "type": "object", "properties": props, "required": required });
    if !subs.is_empty() {
        v["subcommands"] = Value::Object(subs);
    }
    v
}

/// The daemon socket protocol (thc_core::proto v1), for clients such as ThoughtBar.
pub fn proto_schema() -> Value {
    let mut d = defs();
    let n = d.as_object_mut().unwrap();
    n.insert("CapturePreview".into(), json!({
        "type": "object", "required": ["text", "kind", "tags", "links", "bad"],
        "description": "what a capture would save (the same lenient parser as `capture`)",
        "properties": {
            "text": { "type": "string" }, "kind": { "enum": ["task", "note", "event"] },
            "status": { "type": "string" }, "scheduled": { "type": "string" }, "due": { "type": "string" },
            "priority": { "type": "string" }, "repeat": { "type": "string" },
            "tags": { "type": "array", "items": { "type": "string" } }, "links": { "type": "array", "items": { "type": "string" } },
            "bad": { "type": "array", "items": { "type": "string" }, "description": "tokens whose value didn't parse; saved as plain text" },
            "bad_why": { "type": "array", "items": { "type": "string" }, "description": "why each `bad` token didn't parse, same order (e.g. `\"30m\" is ambiguous here · use 30min or 30mo`)" }
        }
    }));
    n.insert("Notification".into(), json!({
        "type": "object", "required": ["alert", "node", "title", "line1", "line2", "thread", "fire_at"],
        "properties": {
            "alert": { "type": "string" }, "node": { "type": "string" }, "title": { "type": "string" },
            "line1": { "type": "string" }, "line2": { "type": "string" }, "line2_plain": { "type": "string" },
            "thread": { "type": "string" }, "time_sensitive": { "type": "boolean" }, "is_task": { "type": "boolean" }, "fire_at": { "type": "string" }
        }
    }));
    n.insert("Delivery".into(), json!({
        "type": "object", "required": ["kind"],
        "properties": {
            "kind": { "enum": ["single", "summary", "silent"] },
            "notification": { "$ref": "#/$defs/Notification" },
            "title": { "type": "string" }, "body": { "type": "string" },
            "alerts": { "type": "array", "items": { "type": "string" } }, "missed": { "type": "boolean" },
            "thread": { "type": "string", "description": "summaries: the day thread, thc.alerts.<date>" }
        }
    }));
    n.insert("TodayPanel".into(), json!({
        "type": "object",
        "properties": {
            "date": { "type": "string" },
            "overdue": { "type": "array", "items": { "$ref": "#/$defs/Node" } },
            "today": { "type": "array", "items": { "$ref": "#/$defs/Node" } },
            "upcoming": { "type": "array", "items": { "$ref": "#/$defs/Node" } },
            "inbox": { "type": "object", "properties": { "count": { "type": "integer" }, "latest": { "type": "array", "items": { "$ref": "#/$defs/Node" } } } },
            "alerts": { "type": "array", "items": { "$ref": "#/$defs/Alert" } },
            "conflicts": { "type": "integer" },
            "to_review": { "type": "integer", "description": "agent transactions waiting for review" },
            "badge": { "type": "integer", "description": "menu bar count: overdue + today (due or scheduled) + fired unacknowledged alerts, each node once" },
            "conflict_ids": { "type": "array", "items": { "type": "string" }, "description": "nodes with an open sync conflict" },
            "context": { "type": ["string", "null"], "description": "this device's context (ThoughtBar shows it, never applies it)" },
            "presets": { "type": "array", "description": "views marked --bar (at most 3), for the panel's Today ▾ menu", "items": { "type": "object", "required": ["name", "query"], "properties": { "name": { "type": "string" }, "title": { "type": ["string", "null"] }, "query": { "type": "string" }, "count": { "type": "integer" } } } },
            "upcoming_more": { "type": "integer", "description": "upcoming rows beyond the 8 a panel shows" }
        }
    }));
    n.insert("Block".into(), json!({
        "type": "object", "required": ["id", "depth", "kind", "text", "tags"],
        "description": "a node as the editor shows it (mac-editor-arch.md §4): document order, clean text, fields for the gutter",
        "properties": {
            "id": { "type": "string" }, "parent": { "type": "string" }, "depth": { "type": "integer" },
            "kind": { "enum": ["para", "bullet", "task"] }, "status": { "type": "string" }, "text": { "type": "string" },
            "scheduled": { "type": "string" }, "due": { "type": "string" }, "priority": { "type": "string" }, "repeat": { "type": "string" },
            "tags": { "type": "array", "items": { "type": "string" } },
            "rev": { "type": "string", "description": "the node's latest event (guards move/delete/kind/status)" },
            "text_rev": { "type": "string", "description": "the event that last set the text: an edit's base" },
            "conflict": { "type": "boolean", "description": "an open text conflict" },
            "done_at": { "type": "string" }
        }
    }));
    n.insert("LogTx".into(), json!({
        "type": "object", "required": ["tx", "ms", "actor", "via", "ops"],
        "properties": {
            "tx": { "type": "string" }, "ms": { "type": "integer" }, "actor": { "type": "string" }, "via": { "type": "string" }, "dev": { "type": "string" },
            "review": { "type": "boolean", "description": "an agent's transaction waiting for review" },
            "ops": { "type": "array", "items": { "type": "object", "required": ["op", "entity"], "properties": { "op": { "type": "string" }, "entity": { "type": "string" }, "label": { "type": "string" } } } }
        }
    }));
    n.insert("PageSummary".into(), json!({
        "type": "object", "required": ["id", "title", "lines", "updated_ms"],
        "properties": { "id": { "type": "string" }, "title": { "type": "string" }, "lines": { "type": "integer" }, "updated_ms": { "type": "integer", "description": "last edit anywhere in the page (epoch ms)" } }
    }));
    n.insert("BlockResult".into(), json!({
        "type": "object", "required": ["index", "id", "state"],
        "properties": {
            "index": { "type": "integer" }, "id": { "type": "string" },
            "state": { "enum": ["ok", "stale", "exists", "error"] }, "error": { "type": "string" },
            "block": { "$ref": "#/$defs/Block" }
        }
    }));
    let obj = |props: Value| json!({ "type": "object", "properties": props });
    let id_param = obj(json!({ "id": { "type": "string" } }));
    let blocks = json!({ "type": "array", "items": { "$ref": "#/$defs/Block" } });
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "proto": thc_core::proto::PROTO_VERSION,
        "transport": "newline-delimited JSON over a Unix socket; discover via <cache>/daemon.json",
        "envelope": {
            "request": obj(json!({ "id": { "type": "integer" }, "method": { "type": "string" }, "params": { "type": "object" } })),
            "response": obj(json!({ "id": { "type": "integer" }, "result": {}, "error": { "type": "object", "properties": { "kind": { "type": "string" }, "message": { "type": "string" } } } })),
            "event": obj(json!({ "event": { "type": "string" }, "data": {} }))
        },
        "methods": {
            "hello": { "params": obj(json!({ "client": { "type": "string" }, "version": { "type": "string" }, "proto": { "type": "integer" }, "topics": { "type": "array", "items": { "enum": ["changed", "alerts", "tokens", "*"] } }, "deliver": { "type": "boolean" } })),
                       "result": obj(json!({ "daemon": { "type": "string" }, "proto": { "type": "integer" }, "proto_minor": { "type": "integer", "description": "additive revision within proto" }, "device": { "type": "string" }, "version_mismatch": { "type": "boolean" } })) },
            "status": { "params": obj(json!({})), "result": { "$ref": "#/$defs/DaemonStatus" } },
            "tokens": { "params": obj(json!({})), "result": obj(json!({ "items": { "type": "array", "items": obj(json!({ "id": { "type": "string" }, "tokens": obj(json!({ "in": { "type": "integer" }, "out": { "type": "integer" }, "cache": { "type": "integer" }, "source": { "type": "string" }, "sessions": { "type": "array", "items": { "type": "string" } } })), "collected_ms": { "type": "integer" } })) } })) },
            "today": { "params": obj(json!({})), "result": { "$ref": "#/$defs/TodayPanel" } },
            "query": { "params": obj(json!({ "q": { "type": "string" }, "limit": { "type": "integer" } })), "result": { "$ref": "#/$defs/List" } },
            "parse": { "params": obj(json!({ "text": { "type": "string" } })), "result": { "$ref": "#/$defs/CapturePreview" } },
            "capture": { "params": obj(json!({ "text": { "type": "string" }, "target": { "type": "string", "description": "journal (default) | inbox | <node id>" } })), "result": obj(json!({ "id": { "type": "string" }, "tx": { "type": "string" }, "events": { "type": "integer" } })) },
            "complete": { "params": id_param.clone(), "result": obj(json!({ "id": { "type": "string" }, "next": {}, "tx": { "type": "string" } })) },
            "set": { "params": obj(json!({ "id": { "type": "string" }, "props": { "type": "object" } })), "result": obj(json!({ "id": { "type": "string" }, "tx": { "type": "string" } })) },
            "snooze": { "params": obj(json!({ "alert": { "type": "string" }, "until": { "type": "string", "description": "duration (1h) or date" } })), "result": obj(json!({ "alert": { "type": "string" }, "until": { "type": "string" } })) },
            "ack": { "params": obj(json!({ "alert": { "type": "string" } })), "result": obj(json!({ "alert": { "type": "string" } })) },
            "undo": { "params": obj(json!({ "tx": { "type": "string" } })), "result": obj(json!({ "undone": { "type": "string" }, "tx": { "type": "string" } })) },
            "alert.delivered": { "params": obj(json!({ "alerts": { "type": "array", "items": { "type": "string" } } })), "result": obj(json!({ "ok": { "type": "boolean" }, "claimed": { "type": "integer" } })) },
            "shutdown": { "params": obj(json!({})), "result": obj(json!({ "ok": { "type": "boolean" } })) },
            "locate": { "params": id_param.clone(), "result": obj(json!({ "id": { "type": "string" }, "root": { "type": "string" }, "journal": { "type": ["string", "null"] }, "title": { "type": ["string", "null"] }, "inbox": { "type": "boolean" } })) },
            "delete": { "params": id_param.clone(), "result": obj(json!({ "id": { "type": "string" }, "deleted": { "type": "integer" }, "tx": { "type": "string" } })) },
            "conflicts": { "params": obj(json!({ "id": { "type": "string" } })), "result": obj(json!({ "items": { "type": "array", "items": { "$ref": "#/$defs/Conflict" } } })) },
            "conflict.resolve": { "params": obj(json!({ "id": { "type": "string" }, "keep": { "type": "string", "enum": ["current", "other", "both"] }, "top": { "type": "string", "enum": ["current", "other"] } })), "result": obj(json!({ "id": { "type": "string" }, "kept": { "type": "string" }, "new_note": { "type": "string" }, "tx": { "type": "string" } })) },
            "markdown": { "params": obj(json!({ "root": { "type": "string" }, "ids": { "type": "array", "items": { "type": "string" } } })), "result": obj(json!({ "markdown": { "type": "string" } })) },
            "backlinks": { "params": id_param.clone(), "result": { "$ref": "#/$defs/List" } },
            "search": { "params": obj(json!({ "text": { "type": "string" }, "limit": { "type": "integer" } })), "result": { "$ref": "#/$defs/List" } },
            "log": { "params": obj(json!({ "limit": { "type": "integer" } })), "result": obj(json!({ "txs": { "type": "array", "items": { "$ref": "#/$defs/LogTx" } } })) },
            "pages": { "params": obj(json!({})), "result": obj(json!({ "pages": { "type": "array", "items": { "$ref": "#/$defs/PageSummary" } } })) },
            "page": { "params": id_param.clone(), "result": obj(json!({ "root": { "type": "string" }, "title": { "type": "string" }, "journal": { "type": "string" }, "blocks": blocks.clone() })) },
            "journal": { "params": obj(json!({ "date": { "type": "string", "description": "YYYY-MM-DD, today, -1d" } })), "result": obj(json!({ "root": { "type": ["string", "null"], "description": "null until the day's first save" }, "date": { "type": "string" }, "blocks": blocks.clone() })) },
            "blocks": { "params": obj(json!({ "root": { "type": "string" }, "ids": { "type": "array", "items": { "type": "string" } } })), "result": obj(json!({ "root": { "type": "string" }, "blocks": blocks.clone(), "gone": { "type": "array", "items": { "type": "string" } } })) },
            "blocks.apply": { "params": obj(json!({ "root": { "type": "string" }, "via": { "type": "string", "description": "who typed it, when not the daemon (`tui`): stamped on the events" }, "journal": { "type": "string", "description": "instead of root: the day (created by its first save)" }, "ops": { "type": "array", "description": "block ops: op = edit, create, move, delete, kind or status (FORMAT.md Block ops)", "items": { "type": "object" } } })),
                              "result": obj(json!({ "root": { "type": "string" }, "tx": { "type": ["string", "null"] }, "events": { "type": "integer" }, "results": { "type": "array", "items": { "$ref": "#/$defs/BlockResult" } } })) }
        },
        "events": {
            "tokens": { "topic": "tokens", "data": obj(json!({ "ids": { "type": "array", "items": { "type": "string" } } })) },
            "changed": { "topic": "changed", "data": obj(json!({ "ids": { "type": "array", "items": { "type": "string" } }, "tx": { "type": "string" }, "actor": { "type": "string" }, "dev": { "type": "string" } })) },
            "conflict": { "topic": "changed", "data": obj(json!({ "id": { "type": "string" }, "kind": { "enum": ["text", "move"] } })) },
            "alert.fire": { "topic": "alerts", "data": { "$ref": "#/$defs/Delivery" } },
            "alert.withdraw": { "topic": "alerts", "data": obj(json!({ "alerts": { "type": "array", "items": { "type": "string" } } })) },
            "shutdown": { "topic": "*", "description": "sent to every client just before the daemon stops", "data": obj(json!({ "reason": { "enum": ["requested", "signal", "error"] } })) }
        },
        "$defs": d,
    })
}

pub fn schema(cmd: Option<&str>, all: bool) -> anyhow::Result<Value> {
    let root = crate::registry::command();
    let globals: Vec<Value> = root
        .get_arguments()
        .filter(|a| a.is_global_set())
        .map(|a| json!({ "name": a.get_id().as_str(), "description": a.get_help().map(|h| h.to_string()) }))
        .collect();
    let command_entry = |c: &clap::Command| json!({ "command": c.get_name(), "about": c.get_about().map(|a| a.to_string()), "input": input_for(c), "output": output_for(c.get_name()) });
    match (cmd, all) {
        (Some(name), _) => {
            let c = root
                .get_subcommands()
                .find(|c| c.get_name() == name)
                .ok_or_else(|| thc_core::error::not_found(format!("no command {name} · thc schema lists them")))?;
            Ok(json!({ "$schema": "https://json-schema.org/draft/2020-12/schema", "command": name, "input": input_for(c), "output": output_for(name), "$defs": cli_defs(), "globals": globals }))
        }
        (None, true) => Ok(json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "version": env!("CARGO_PKG_VERSION"),
            "stability": "additive: fields are only added, never removed or retyped, within a major version",
            "globals": globals,
            "commands": root.get_subcommands().map(command_entry).collect::<Vec<_>>(),
            "$defs": cli_defs(),
        })),
        (None, false) => Ok(json!({
            "commands": root.get_subcommands().map(|c| json!({ "command": c.get_name(), "about": c.get_about().map(|a| a.to_string()) })).collect::<Vec<_>>(),
            "hint": "thc schema <command> for input/output schemas, thc schema --all for everything",
        })),
    }
}

/// Keep only `fields` on every node object (objects with `short` and `kind`).
pub fn filter_fields(v: &mut Value, fields: &[String]) {
    match v {
        Value::Object(m) => {
            let is_node = m.contains_key("short") && m.contains_key("kind");
            for (_, child) in m.iter_mut() {
                filter_fields(child, fields);
            }
            if is_node {
                m.retain(|k, _| fields.iter().any(|f| f == k));
            }
        }
        Value::Array(a) => {
            for x in a {
                filter_fields(x, fields);
            }
        }
        _ => {}
    }
}

// ---- Swift types for ThoughtBar (`thc schema --proto --swift`) -----------------------------------

fn camel(s: &str, upper: bool) -> String {
    let mut out = String::new();
    let mut up = upper;
    for c in s.chars() {
        if c == '_' || c == '.' || c == '-' {
            up = true;
        } else if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Schema names that would shadow Swift standard types.
fn swift_name(def: &str) -> String {
    match def {
        "Error" => "ProtoError".into(),
        "View" => "SavedView".into(),
        other => other.to_string(),
    }
}

/// Swift type for a schema; nested inline objects become structs named `<parent><Prop>`.
fn swift_type(schema: &Value, name_hint: &str, out: &mut Vec<(String, Value)>) -> (String, bool) {
    if let Some(r) = schema.get("$ref").and_then(|r| r.as_str()) {
        return (swift_name(r.trim_start_matches("#/$defs/")), false);
    }
    let (types, nullable): (Vec<String>, bool) = match schema.get("type") {
        Some(Value::String(t)) => (vec![t.clone()], false),
        Some(Value::Array(a)) => {
            let ts: Vec<String> = a.iter().filter_map(|x| x.as_str()).filter(|t| *t != "null").map(str::to_string).collect();
            (ts, a.iter().any(|x| x == "null"))
        }
        _ if schema.get("enum").is_some() => (vec!["string".into()], schema["enum"].as_array().is_some_and(|e| e.contains(&Value::Null))),
        _ if schema.get("const").is_some_and(|c| c.is_boolean()) => (vec!["boolean".into()], false),
        _ => (vec![], false),
    };
    let t = match types.as_slice() {
        [t] => match t.as_str() {
            "string" => "String".to_string(),
            "integer" => "Int".to_string(),
            "number" => "Double".to_string(),
            "boolean" => "Bool".to_string(),
            "array" => {
                let (inner, _) = swift_type(schema.get("items").unwrap_or(&Value::Null), &format!("{name_hint}Item"), out);
                format!("[{inner}]")
            }
            "object" if schema.get("properties").and_then(|p| p.as_object()).is_some_and(|p| !p.is_empty()) => {
                out.push((name_hint.to_string(), schema.clone()));
                name_hint.to_string()
            }
            _ => "JSONValue".to_string(),
        },
        _ => "JSONValue".to_string(),
    };
    (t, nullable)
}

fn swift_struct(name: &str, schema: &Value, out: &mut String) {
    let mut nested: Vec<(String, Value)> = Vec::new();
    let required: Vec<&str> = schema.get("required").and_then(|r| r.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    let props = schema.get("properties").and_then(|p| p.as_object()).cloned().unwrap_or_default();
    let mut fields = Vec::new();
    let mut keys = Vec::new();
    let mut params = Vec::new();
    let mut assigns = Vec::new();
    for (k, sub) in &props {
        let (t, nullable) = swift_type(sub, &format!("{name}{}", camel(k, true)), &mut nested);
        let field = camel(k, false);
        let field = if matches!(field.as_str(), "default" | "repeat" | "protocol" | "where" | "in" | "is" | "case") { format!("`{field}`") } else { field };
        let optional = nullable || !required.contains(&k.as_str());
        if let Some(d) = sub.get("description").and_then(|d| d.as_str()) {
            fields.push(format!("    /// {d}"));
        }
        fields.push(format!("    public var {field}: {t}{}", if optional { "?" } else { "" }));
        keys.push(format!("        case {field} = \"{k}\""));
        params.push(format!("{field}: {t}{}", if optional { "? = nil" } else { "" }));
        assigns.push(format!("        self.{} = {field}", field.trim_matches('`')));
    }
    out.push_str(&format!("public struct {name}: Codable, Hashable, Sendable {{\n"));
    for f in &fields {
        out.push_str(f);
        out.push('\n');
    }
    out.push_str(&format!("\n    public init({}) {{\n", params.join(", ")));
    for a in &assigns {
        out.push_str(a);
        out.push('\n');
    }
    out.push_str("    }\n");
    if !keys.is_empty() {
        out.push_str("\n    enum CodingKeys: String, CodingKey {\n");
        for k in &keys {
            out.push_str(k);
            out.push('\n');
        }
        out.push_str("    }\n");
    }
    out.push_str("}\n\n");
    for (n, s) in nested {
        swift_struct(&n, &s, out);
    }
}

/// Codable models for every `$defs` type, method params/result and event payload.
pub fn swift_types() -> String {
    let p = proto_schema();
    let mut out = String::from(
        "// Generated by `thc schema --proto --swift`. Do not edit: regenerate, and\n\
         // crates/thc/tests/schema_contract.rs fails if this file drifts from the binary.\n\
         // Fields are only ever added (the JSON contract), so everything not required is optional.\n\n\
         import Foundation\n\n",
    );
    out.push_str(&format!("public enum Proto {{\n    public static let version = {}\n}}\n\n", p["proto"]));
    let mut defs: Vec<(&String, &Value)> = p["$defs"].as_object().unwrap().iter().collect();
    defs.sort_by_key(|(k, _)| k.as_str());
    for (name, schema) in defs {
        swift_struct(&swift_name(name), schema, &mut out);
    }
    let mut methods: Vec<(&String, &Value)> = p["methods"].as_object().unwrap().iter().collect();
    methods.sort_by_key(|(k, _)| k.as_str());
    for (m, v) in methods {
        let base = camel(m, true);
        for (part, suffix) in [("params", "Params"), ("result", "Result")] {
            let s = &v[part];
            if s.get("properties").is_some() {
                swift_struct(&format!("{base}{suffix}"), s, &mut out);
            }
        }
    }
    let mut events: Vec<(&String, &Value)> = p["events"].as_object().unwrap().iter().collect();
    events.sort_by_key(|(k, _)| k.as_str());
    for (e, v) in events {
        if v["data"].get("properties").is_some() {
            swift_struct(&format!("{}Event", camel(e, true)), &v["data"], &mut out);
        }
    }
    out.trim_end().to_string() + "\n"
}
