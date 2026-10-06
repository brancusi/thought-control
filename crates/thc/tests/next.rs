//! The queue: `sort:order` is the outline order, and `thc next` is the first
//! ready, unblocked, unowned task in that order under the vault's capture target.

mod common;

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

struct V {
    root: PathBuf,
}

impl V {
    fn new(name: &str) -> V {
        let root = std::env::temp_dir().join(format!("thc-next-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let v = V { root };
        assert!(v.cmd("human").args(["init", "vault"]).output().unwrap().status.success());
        v
    }

    fn cmd(&self, actor: &str) -> Command {
        let mut c = common::thc();
        c.current_dir(&self.root).env("THC_VAULT", self.root.join("vault")).env("THC_CACHE_DIR", self.root.join("cache")).env("THC_ACTOR", actor).env("THC_NOW", "2026-10-06T09:00");
        c
    }

    fn run(&self, actor: &str, args: &[&str]) -> Output {
        self.cmd(actor).arg("--json").args(args).output().unwrap()
    }

    fn json(&self, actor: &str, args: &[&str]) -> Value {
        let o = self.run(actor, args);
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap()
    }

    fn id(&self, args: &[&str]) -> String {
        self.json("human", args)["nodes"][0]["id"].as_str().unwrap().to_string()
    }

    /// `thc next` as `actor`: the shorts it lists.
    fn next(&self, actor: &str, args: &[&str]) -> Vec<String> {
        let mut a = vec!["next"];
        a.extend_from_slice(args);
        let v = self.json(actor, &a);
        v["items"].as_array().unwrap().iter().map(|n| n["text"].as_str().unwrap().to_string()).collect()
    }
}

#[test]
fn sort_order_is_the_outline_order() {
    let v = V::new("order");
    let page = v.id(&["page", "new", "Issues"]);
    let a = v.id(&["todo", "a", "--under", &page]);
    v.id(&["todo", "a1", "--under", &a]);
    let b = v.id(&["todo", "b", "--under", &page]);
    v.id(&["todo", "a2", "--under", &a]);
    // c goes first, before a: the outline is c, a, a1, a2, b.
    v.id(&["todo", "c", "--under", &page]);
    let c = v.json("human", &["q", "text:c", "--fields", "id"])["items"][0]["id"].as_str().unwrap().to_string();
    v.json("human", &["mv", &c, "--before", &a]);
    let _ = b;
    let q = v.json("human", &["q", &format!("under:{page} sort:order")]);
    let texts: Vec<&str> = q["items"].as_array().unwrap().iter().map(|n| n["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["c", "a", "a1", "a2", "b"]);
    // Reversed: last first.
    let q = v.json("human", &["q", &format!("under:{page} sort:order-")]);
    let texts: Vec<&str> = q["items"].as_array().unwrap().iter().map(|n| n["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["b", "a2", "a1", "a", "c"]);
    let e = v.json("human", &["q", "sort:order", "--explain"]);
    assert!(e.to_string().contains("outline order"), "{e}");
}

#[test]
fn next_skips_blocked_owned_and_unready_tasks() {
    let v = V::new("next");
    let page = v.id(&["page", "new", "Issues"]);
    std::fs::write(v.root.join("vault/settings.toml"), "[capture]\ntarget = \"¶ Issues\"\n").unwrap();
    let first = v.id(&["todo", "first, owned", "--under", &page]);
    let blocked = v.id(&["todo", "blocked by first", "--under", &page]);
    v.id(&["todo", "later, not yet", "--under", &page, "--sched", "+3d"]);
    v.id(&["todo", "free one", "--under", &page]);
    v.id(&["todo", "free two", "--under", &page]);
    // Elsewhere in the vault: not in the queue.
    v.id(&["todo", "in the journal", "--journal", "today"]);
    v.json("human", &["set", &first, "owner=codex"]);
    v.json("human", &["link", &first, &blocked, "--rel", "blocks"]);

    assert_eq!(v.next("claude", &[]), ["free one"]);
    assert_eq!(v.next("claude", &["-n", "5"]), ["free one", "free two"]);
    let j = v.json("claude", &["next"]);
    assert_eq!(j["queue"]["title"], "Issues");
    assert!(j["claim"].as_str().unwrap().contains("owner=claude' --expect status=todo"), "{j}");
    // --mine: what you own, in order.
    assert_eq!(v.next("codex", &["--mine"]), ["first, owned"]);
    // Claimed: the next one moves up.
    v.json("claude", &["set", &v.json("claude", &["next"])["items"][0]["id"].as_str().unwrap().to_string(), "status=doing", "owner=claude", "--expect", "status=todo"]);
    assert_eq!(v.next("claude", &[]), ["free two"]);
    assert_eq!(v.next("claude", &["--mine"]), ["free one"]);
    // Done: its blocked task is free.
    v.json("codex", &["done", &first]);
    assert_eq!(v.next("claude", &[]), ["blocked by first"]);
    // --under names another queue.
    let other = v.id(&["page", "new", "Other"]);
    v.id(&["todo", "other work", "--under", &other]);
    assert_eq!(v.next("claude", &["--under", &other]), ["other work"]);
}

#[test]
fn an_empty_queue_exits_3() {
    let v = V::new("empty");
    let page = v.id(&["page", "new", "Issues"]);
    std::fs::write(v.root.join("vault/settings.toml"), "[capture]\ntarget = \"¶ Issues\"\n").unwrap();
    let t = v.id(&["todo", "only one", "--under", &page]);
    v.json("human", &["set", &t, "owner=someone"]);
    let o = v.run("claude", &["next"]);
    assert_eq!(o.status.code(), Some(3), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stderr).contains("no unowned ready task in ¶ Issues"));
    let o = v.run("claude", &["next", "--mine"]);
    assert_eq!(o.status.code(), Some(3));
    // No capture target: name the board explicitly to use the whole vault.
    std::fs::remove_file(v.root.join("vault/settings.toml")).unwrap();
    v.id(&["todo", "loose task"]);
    assert_eq!(v.run("claude", &["next"]).status.code(), Some(6));
    assert_eq!(v.next("claude", &["--board", v.root.join("vault").to_str().unwrap()]), ["loose task"]);
}

#[test]
fn next_filters_roles_without_skipping_unrouted_work() {
    let v = V::new("roles");
    let page = v.id(&["page", "new", "Issues"]);
    std::fs::write(v.root.join("vault/settings.toml"), "[capture]\ntarget = \"¶ Issues\"\n").unwrap();
    let designer = v.id(&["todo", "design", "--under", &page]);
    v.json("human", &["set", &designer, "role=designer"]);
    let engineer = v.id(&["todo", "build", "--under", &page]);
    v.json("human", &["set", &engineer, "role=engineer"]);
    v.id(&["todo", "anyone", "--under", &page]);
    assert_eq!(v.next("codex", &["--role", "engineer", "-n", "5"]), ["build", "anyone"]);
    assert_eq!(v.next("codex", &["--role", "reviewer"]), ["anyone"]);
    assert_eq!(v.next("codex", &["-n", "5"]), ["design", "build", "anyone"]);
    v.json("human", &["set", &designer, "owner=codex"]);
    v.json("human", &["set", &engineer, "owner=codex"]);
    assert_eq!(v.next("codex", &["--mine", "--role", "engineer"]), ["build"]);
    assert_eq!(v.run("codex", &["next", "--mine", "--role", "reviewer"]).status.code(), Some(3));
}

#[test]
fn coordination_is_available_in_every_generated_agent_file() {
    let v = V::new("coordination");
    let topic = v.run("codex-engineer-2", &["instructions", "coordination"]);
    assert!(topic.status.success());
    let topic = String::from_utf8(topic.stdout).unwrap().trim_end().to_string();
    assert!(topic.contains("Keep no private task"));
    assert!(topic.contains("thc next --json"));
    let all = v.run("codex-engineer-2", &["instructions", "all"]);
    assert!(all.status.success());
    let all = String::from_utf8(all.stdout).unwrap();
    assert!(all.contains(&topic.replace("## ", "### ")));
    assert!(all.contains("sort:order"));
    v.json("codex-engineer-2", &["setup", "claude", "--yes"]);
    let skill = std::fs::read_to_string(v.root.join(".claude/skills/thc/SKILL.md")).unwrap();
    assert!(skill.contains("coordinate with thc"));
    assert!(skill.contains(&topic));
    let agents = std::fs::read_to_string(v.root.join("AGENTS.md")).unwrap();
    assert!(agents.contains(&topic.replace("## ", "### ")));
    // The user-level Codex block is the short skill body, including the same protocol.
    assert!(v.cmd("codex-engineer-2").env("HOME", &v.root).args(["setup", "codex", "--user", "--yes"]).output().unwrap().status.success());
    let codex = std::fs::read_to_string(v.root.join(".codex/AGENTS.md")).unwrap();
    assert!(codex.contains(&topic));
}
