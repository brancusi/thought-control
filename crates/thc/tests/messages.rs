//! Message routing, independent actor receipts, plain capture and guarded acknowledgements.
mod common;
use serde_json::Value;
use std::process::Output;

struct V {
    root: std::path::PathBuf,
}
impl V {
    fn new(name: &str) -> Self {
        let root = common::root().join(format!("messages-{name}"));
        std::fs::create_dir_all(&root).unwrap();
        let v = Self { root };
        v.json("human", &["init", "vault"]);
        let config = v.root.join(".thc.toml");
        let text = std::fs::read_to_string(&config).unwrap();
        std::fs::write(config, format!("{text}\nboard = \"vault\"\n")).unwrap();
        v
    }
    fn run(&self, actor: &str, args: &[&str]) -> Output {
        common::thc()
            .current_dir(&self.root)
            .env("THC_VAULT", self.root.join("vault"))
            .env_remove("THC_CACHE_DIR")
            .env("THC_ACTOR", actor)
            .env_remove("THC_ROLE")
            .env_remove("THC_BOARD")
            .env("THC_NOW", "2026-10-06T09:00")
            .args(["--json"])
            .args(args)
            .output()
            .unwrap()
    }
    fn json(&self, actor: &str, args: &[&str]) -> Value {
        let o = self.run(actor, args);
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn id(&self, args: &[&str]) -> String {
        self.json("claude-pm", args)["nodes"][0]["id"].as_str().unwrap().into()
    }
    fn unread(&self, actor: &str) -> usize {
        self.json(actor, &["msgs", "--unread"])["count"].as_u64().unwrap() as usize
    }
}

#[test]
fn send_plain_under_task_or_messages_and_retry_is_noop() {
    let v = V::new("plain");
    let task = v.id(&["todo", "Build it"]);
    let text = "due:fri #tag !high [[Do not create]]";
    let sent = v.json("claude-pm", &["msg", "engineer", text, "--on", &task, "--key", "msg-plain"]);
    let n = &sent["nodes"][0];
    assert_eq!(n["parent"], task);
    assert_eq!(n["props"]["to"], "engineer");
    assert_eq!(n["props"]["from"], "claude-pm");
    assert_eq!(n["text"], format!("to: engineer (from claude-pm): {text}"));
    assert!(n.get("due").is_none() && n.get("status").is_none());
    assert!(n["tags"].as_array().unwrap().is_empty());
    let again = v.json("claude-pm", &["msg", "engineer", text, "--about", &task, "--key", "msg-plain"]);
    assert_eq!(again["events"], 0);
    assert_eq!(again["nodes"][0]["id"], n["id"]);
    let id = v.id(&["msg", "engineer", "loose message"]);
    let note = v.json("human", &["show", &id]);
    let page = v.json("human", &["show", note["parent"].as_str().unwrap()]);
    assert_eq!(page["title"], "Messages");
    let pages = v.json("human", &["pages"]);
    assert!(!pages["items"].as_array().unwrap().iter().any(|p| p["title"] == "Do not create"));
}

#[test]
fn roles_and_all_broadcast_have_independent_actor_receipts() {
    let v = V::new("receipts");
    let role = v.id(&["msg", "engineer", "build"]);
    let broadcast = v.id(&["msg", "all", "release"]);
    let direct = v.id(&["msg", "codex-engineer-2", "for you"]);
    v.id(&["msg", "designer", "design"]);
    assert_eq!(v.unread("codex-engineer-2"), 3);
    assert_eq!(v.unread("claude-engineer"), 2);
    assert_eq!(v.unread("claude-designer"), 2);
    assert_eq!(v.json("codex-engineer-2", &["q", "to:engineer"])["count"], 3);
    assert_eq!(v.json("codex-engineer-2", &["q", "is:unread"])["count"], 3);
    // Listing never acknowledges, and --actor overrides the environment for unread queries.
    assert_eq!(v.unread("codex-engineer-2"), 3);
    assert_eq!(v.json("claude-designer", &["--actor", "codex-engineer-2", "q", "is:unread"])["count"], 3);
    v.json("codex-engineer-2", &["msg", "read", &role]);
    assert_eq!(v.unread("codex-engineer-2"), 2);
    assert_eq!(v.unread("claude-engineer"), 2);
    // --all ignores the listing limit, and acknowledgements are repeatable no-ops.
    v.json("codex-engineer-2", &["--limit", "1", "msg", "read", "--all"]);
    assert_eq!(v.unread("codex-engineer-2"), 0);
    assert_eq!(v.unread("claude-engineer"), 2);
    assert_eq!(v.unread("claude-designer"), 2);
    assert_eq!(v.json("codex-engineer-2", &["msg", "read", &direct])["events"], 0);
    let props = v.json("human", &["show", &broadcast])["props"].clone();
    assert_eq!(props["read_codex-engineer-2"], true);
    assert!(props.get("read_claude-engineer").is_none());
    assert_eq!(v.json("codex-engineer-2", &["q", "to:engineer -is:unread"])["count"], 3);
}

#[test]
fn invalid_or_unaddressed_read_is_atomic_and_guarded() {
    let v = V::new("guards");
    let a = v.id(&["msg", "engineer", "one"]);
    let b = v.id(&["msg", "designer", "other role"]);
    assert_eq!(v.run("codex-engineer-2", &["msg", "read", &a, &b]).status.code(), Some(6));
    assert_eq!(v.unread("codex-engineer-2"), 1);
    let n = v.json("codex-engineer-2", &["show", &a]);
    let rev = n["rev"].as_str().unwrap();
    v.json("claude-pm", &["set", &a, "extra=changed"]);
    assert_eq!(v.run("codex-engineer-2", &["--if-match", rev, "msg", "read", &a]).status.code(), Some(4));
    assert_eq!(v.unread("codex-engineer-2"), 1);
    let plain = v.id(&["add", "ordinary note"]);
    for args in [vec!["msg", "read", &plain], vec!["msg", "read"], vec!["msg", "engineer"], vec!["msg", "bad_name", "text"]] {
        assert_eq!(v.run("codex-engineer-2", &args).status.code(), Some(6));
    }
    v.json("codex-engineer-2", &["--dry-run", "msg", "read", &a]);
    assert_eq!(v.unread("codex-engineer-2"), 1);
    assert_eq!(v.run("codex-engineer-2", &["--readonly", "msg", "read", &a]).status.code(), Some(6));
}

#[test]
fn query_role_matching_is_exact_including_numbered_actors() {
    let v = V::new("matching");
    for to in ["engineer", "claude-engineer", "codex-engineer-2", "grok-engineer-30", "codex-engineering", "codex-engineer-2junk", "designer", "all"] {
        v.id(&["msg", to, "text"]);
    }
    assert_eq!(v.json("human", &["q", "to:engineer"])["count"], 5);
    assert_eq!(v.json("claude-pm", &["msgs", "--for", "engineer"])["count"], 5);
    assert_eq!(v.json("human", &["q", "to:codex-engineer-2"])["count"], 3);
    assert_eq!(v.json("human", &["q", "to:designer"])["count"], 2);
    let explain = v.json("codex-engineer-2", &["q", "to:engineer is:unread", "--explain"]);
    assert!(explain.to_string().contains("unread messages"));
    for cmd in ["msg", "msgs"] {
        assert!(v.json("human", &["schema", cmd]).is_object());
    }
}

#[test]
fn role_prime_acknowledges_only_displayed_messages_for_its_actor() {
    let v = V::new("prime");
    for text in ["first", "second", "third"] { v.id(&["msg", "engineer", text]); }
    let p = v.json("codex-engineer-2", &["--limit", "1", "prime", "--role", "engineer"]);
    assert_eq!(p["messages"].as_array().unwrap().len(), 1);
    assert_eq!(v.unread("codex-engineer-2"), 2);
    assert_eq!(v.unread("claude-engineer"), 3);
    let p = v.json("codex-engineer-2", &["--readonly", "prime", "--role", "engineer"]);
    assert_eq!(p["messages"].as_array().unwrap().len(), 2);
    assert_eq!(v.unread("codex-engineer-2"), 2);
    let dry = v.json("codex-engineer-2", &["--dry-run", "prime", "--role", "engineer"]);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(v.unread("codex-engineer-2"), 2);
    v.json("codex-engineer-2", &["prime", "--role", "engineer"]);
    assert_eq!(v.unread("codex-engineer-2"), 0);
    // An explicit prime role works even for an actor without a role suffix.
    let p = v.json("codex", &["prime", "--role", "engineer"]);
    assert_eq!(p["messages"].as_array().unwrap().len(), 3);
}

#[test]
fn message_commands_resolve_the_board_independently_of_capture() {
    let v = V::new("board");
    v.json("human", &["init", "board-vault"]);
    std::fs::write(v.root.join(".thc.toml"), "vault = \"vault\"\nboard = \"board-vault\"\n").unwrap();
    let board_path = v.root.join("board-vault").to_string_lossy().to_string();
    let task = v.id(&["--vault", &board_path, "todo", "board task"]);
    let sent = v.json("claude-pm", &["msg", "engineer", "on the board", "--on", &task]);
    assert_eq!(sent["nodes"][0]["parent"], task);
    assert_eq!(v.unread("codex-engineer-2"), 1);
    assert_eq!(v.json("human", &["q", "to:engineer"])["count"], 0);
    assert_eq!(v.json("human", &["--vault", &board_path, "q", "to:engineer"])["count"], 1);
    v.json("codex-engineer-2", &["msg", "read", "--all"]);
    assert_eq!(v.unread("codex-engineer-2"), 0);
    std::fs::write(v.root.join(".thc.toml"), "vault = \"vault\"\n").unwrap();
    for args in [vec!["msg", "engineer", "missing board"], vec!["msgs"], vec!["msg", "read", "--all"]] {
        let o = v.run("codex-engineer-2", &args);
        assert_eq!(o.status.code(), Some(6));
        assert!(String::from_utf8_lossy(&o.stderr).contains("no board for this project"));
    }
}

#[test]
fn generated_agent_guides_teach_message_commands_and_receipts() {
    let v = V::new("guide");
    v.json("codex-engineer-3", &["setup", "claude", "--yes"]);
    for path in ["AGENTS.md", ".claude/skills/thc/SKILL.md"] {
        let text = std::fs::read_to_string(v.root.join(path)).unwrap();
        for term in ["thc msg engineer", "thc msgs", "thc msg read", "is:unread", "read_<actor>"] {
            assert!(text.contains(term), "{path} does not teach {term}");
        }
    }
}
