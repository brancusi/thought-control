//! Team contracts T1–T3: boards, role briefings and attribution use only scratch data.
mod common;
use serde_json::Value;
use std::{path::PathBuf, process::{Command, Output}};

struct V { root: PathBuf }
impl V {
    fn new(name: &str) -> Self {
        let root = thc_core::scratch::dir(&format!("thc-team-roles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home")).unwrap();
        let v = Self { root };
        v.json(&["init", "vault"]);
        v.json(&["vault", "add", v.root.join("vault").to_str().unwrap(), "--as", "capture"]);
        v.json(&["vault", "new", "board", v.root.join("board").to_str().unwrap()]);
        v
    }
    fn cmd(&self) -> Command {
        let mut c = common::thc();
        c.current_dir(&self.root).env("HOME", self.root.join("home"))
            .env("THC_CONFIG_DIR", self.root.join("config"))
            .env("THC_VAULT", self.root.join("vault"))
            // Caches must belong to the selected vault (avoid reusing one store for two logs).
            .env_remove("THC_CACHE_DIR")
            .env("THC_ACTOR", "human").env("THC_NOW", "2026-10-06T09:00");
        c
    }
    fn run(&self, args: &[&str]) -> Output { self.cmd().arg("--json").args(args).output().unwrap() }
    fn json(&self, args: &[&str]) -> Value { decode(self.run(args)) }
    fn id(&self, args: &[&str]) -> String { self.json(args)["nodes"][0]["id"].as_str().unwrap().into() }
    fn page(&self, title: &str) -> String { self.id(&["--vault", "board", "page", "new", title]) }
    fn board_config(&self, raw: &str) {
        std::fs::write(self.root.join(".thc.toml"), format!("vault = \"vault\"\nboard = {raw:?}\n")).unwrap();
    }
}
fn decode(o: Output) -> Value {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).unwrap()
}

#[test]
fn board_resolution_is_independent_of_capture_and_obeys_precedence() {
    let v = V::new("precedence");
    let issues = v.page("Issues");
    v.page("Alternate");
    v.board_config("board:¶ Issues");
    let b = v.json(&["board"]);
    assert_eq!(b["vault"], "board");
    assert_eq!(b["page"]["id"], issues);
    assert_eq!(b["source"], "project");
    assert_eq!(v.json(&["vault"])["name"], "capture");
    let note = v.id(&["add", "still in capture"]);
    assert_eq!(v.json(&["show", &note])["vault"], "capture");
    let env = decode(v.cmd().env("THC_BOARD", "board:¶ Alternate").args(["--json", "board"]).output().unwrap());
    assert_eq!(env["source"], "env");
    assert_eq!(env["page"]["title"], "Alternate");
    let flag = decode(v.cmd().env("THC_BOARD", "board:¶ Alternate").args(["--json", "board", "--board", "board:¶ Issues"]).output().unwrap());
    assert_eq!(flag["source"], "flag");
    assert_eq!(flag["page"]["title"], "Issues");
    // A board key found from a subdirectory is still the nearest project's board.
    std::fs::create_dir_all(v.root.join("src")).unwrap();
    let sub = decode(v.cmd().current_dir(v.root.join("src")).args(["--json", "board"]).output().unwrap());
    assert_eq!(sub["page"]["id"], issues);
}

#[test]
fn fallback_requires_a_capture_page_but_named_board_allows_root() {
    let v = V::new("fallback");
    for args in [vec!["board"], vec!["next"], vec!["prime", "--role", "engineer"]] {
        let o = v.run(&args);
        assert_eq!(o.status.code(), Some(6), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        assert!(String::from_utf8_lossy(&o.stderr).contains("no board for this project"));
    }
    // Plain prime retains its ordinary session briefing.
    assert!(v.json(&["prime"])["counts"].is_object());
    assert!(v.json(&["board", "--board", "board"])["page"].is_null());
    let page = v.id(&["page", "new", "Capture tasks"]);
    std::fs::write(v.root.join("vault/settings.toml"), "[capture]\ntarget = \"¶ Capture tasks\"\n").unwrap();
    let b = v.json(&["board"]);
    assert_eq!(b["source"], "capture");
    assert_eq!(b["page"]["id"], page);
    std::fs::write(v.root.join("board/settings.toml"), "[capture]\ntarget = \"¶ Missing\"\n").unwrap();
    assert_eq!(v.run(&["board", "--board", "board"]).status.code(), Some(6));
}

#[test]
fn role_prime_appends_project_rules_and_uses_the_board_queue() {
    let v = V::new("prime");
    let issues = v.page("Issues");
    let roles = v.page("Roles");
    let card = v.id(&["--vault", "board", "add", "--plain", "--under", &roles, "engineer"]);
    v.id(&["--vault", "board", "add", "--plain", "--under", &card, "Use scratch HOME for every test."]);
    let a = v.id(&["--vault", "board", "todo", "Build the board", "--under", &issues]);
    v.json(&["--vault", "board", "set", &a, "role=engineer"]);
    let d = v.id(&["--vault", "board", "todo", "Design a card", "--under", &issues]);
    v.json(&["--vault", "board", "set", &d, "role=designer"]);
    v.id(&["--vault", "board", "todo", "Unrouted", "--under", &issues]);
    v.id(&["todo", "Elsewhere in capture"]);
    v.board_config("board:¶ Issues");
    let prime = decode(v.cmd().env("THC_ACTOR", "codex-engineer-2").args(["prime", "--role", "engineer", "--json"]).output().unwrap());
    assert_eq!(prime["actor"], "codex-engineer-2");
    assert_eq!(prime["board"]["vault"], "board");
    assert!(prime["card"]["builtin"].as_str().unwrap().contains("Builds what the tasks ask"));
    assert_eq!(prime["card"]["project"][0], "Use scratch HOME for every test.");
    let next: Vec<_> = prime["next"].as_array().unwrap().iter().map(|n| n["text"].as_str().unwrap()).collect();
    assert_eq!(next, ["Build the board", "Unrouted"]);
    // The registered name, not the board's absolute path.
    assert!(prime["claim"].as_str().unwrap().ends_with("--vault board"), "{}", prime["claim"]);
    for k in ["messages", "team", "rules"] { assert!(prime[k].is_array()); }
    let human = v.cmd().env("THC_ACTOR", "codex-engineer-2").args(["prime", "--role", "engineer"]).output().unwrap();
    assert!(human.status.success());
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(text.contains("on the board board · ¶ Issues"));
    assert!(text.contains("Use scratch HOME"));
    assert_eq!(v.json(&["next", "--role", "engineer"])["items"][0]["id"], a);
    assert_eq!(v.json(&["next", "--under", "¶ Issues", "--role", "designer"])["items"][0]["id"], d);
    assert_eq!(v.run(&["prime", "--role", "Engineer"]).status.code(), Some(6));
}

#[test]
fn builtin_cards_and_custom_roles_work_without_a_board() {
    let v = V::new("cards");
    for role in ["pm", "lead", "engineer", "merger", "designer", "reviewer", "support"] {
        let a = v.json(&["role", "show", role]);
        let b = v.json(&["instructions", "role", role]);
        assert_eq!(a, b);
    }
    assert_eq!(v.json(&["role", "show", "pm"])["card"], v.json(&["role", "show", "lead"])["card"]);
    // With a merger on the team, the engineer doesn't merge its own work to main.
    let engineer = v.json(&["role", "show", "engineer"])["card"].to_string();
    assert!(engineer.contains("reviewed merges") && !engineer.contains("only role that commits"), "{engineer}");
    assert!(v.json(&["role", "show", "merger"])["card"].to_string().contains("merges"));
    assert_eq!(v.run(&["role", "show", "bad-role"]).status.code(), Some(6));
}

#[test]
fn attribution_only_stamps_created_agent_nodes_and_replay_preserves_it() {
    let v = V::new("attribution");
    let existing = v.id(&["add", "human note"]);
    let run = |args: &[&str]| decode(v.cmd().env("THC_ACTOR", "codex-engineer-2").env("THC_ROLE", "engineer")
        .env("THC_SESSION_ID", "fixture-session-id").arg("--json").args(args).output().unwrap());
    let n = run(&["todo", "agent task"])["nodes"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(v.json(&["show", &n])["props"]["agent_role"], "engineer");
    assert_eq!(v.json(&["show", &n])["props"]["session_id"], "fixture-session-id");
    run(&["text", &existing, "agent edit"]);
    assert!(v.json(&["show", &existing])["props"].get("agent_role").is_none());
    assert!(v.json(&["show", &existing])["props"].get("session_id").is_none());
    // Rebuild with different environment: attribution comes from the log, not replay's env.
    run(&["rebuild"]);
    assert_eq!(v.json(&["show", &n])["props"]["agent_role"], "engineer");
    let no_session = decode(v.cmd().env("THC_ACTOR", "codex-engineer-2").env("THC_ROLE", "engineer")
        .args(["--json", "add", "no session"]).output().unwrap());
    assert!(no_session["nodes"][0]["props"].get("session_id").is_none());
}
