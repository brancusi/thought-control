//! Atomic task claims (docs/design/agents.md §3.1): agents racing to claim the
//! same task get exactly one winner. `--expect` / `--if-match` are checked inside the write's
//! transaction, under the device's writer lock, after catching up, so the check and the write
//! are one step. The losers exit 4 and write nothing. With and without the daemon.

mod common;

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct V {
    root: PathBuf,
}

impl V {
    fn new(name: &str) -> V {
        let root = std::env::temp_dir().join(format!("thc-claims-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let v = V { root };
        assert!(v.cmd("human").args(["init", "vault"]).output().unwrap().status.success());
        v
    }

    fn cmd(&self, actor: &str) -> Command {
        let mut c = common::thc();
        self.env(&mut c, actor);
        c
    }

    fn env(&self, c: &mut Command, actor: &str) {
        c.current_dir(&self.root).env("THC_VAULT", self.root.join("vault")).env("THC_CACHE_DIR", self.root.join("cache")).env("THC_ACTOR", actor);
    }

    fn json(&self, args: &[&str]) -> Value {
        let o = self.cmd("human").arg("--json").args(args).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap()
    }

    fn task(&self, text: &str) -> (String, String) {
        let n = &self.json(&["todo", text])["nodes"][0];
        (n["id"].as_str().unwrap().to_string(), n["rev"].as_str().unwrap().to_string())
    }

    /// The node's sets that wrote an owner: one per committed claim.
    fn claims(&self, id: &str) -> Vec<Value> {
        let h = self.json(&["history", id]);
        h["events"].as_array().unwrap().iter().filter(|e| e["op"] == "node.set" && e["body"]["props"].get("owner").is_some()).cloned().collect()
    }

    /// `n` agents claim `id` at once (each its own process, released together); exit codes by agent.
    fn race(&self, id: &str, n: usize, guard: &[&str]) -> Vec<(String, Option<i32>, String)> {
        let go = self.root.join(format!("go-{id}"));
        let _ = std::fs::remove_file(&go);
        let thc = env!("CARGO_BIN_EXE_thc");
        let kids: Vec<(String, std::process::Child)> = (0..n)
            .map(|i| {
                let agent = format!("agent{i}");
                // Each waits for the starting gun, then claims: owner, status and session together.
                let mut args = vec!["set".to_string(), id.to_string(), "status=doing".into(), format!("owner={agent}"), format!("session_id=s-{i}")];
                args.extend(guard.iter().map(|s| s.to_string()));
                let script = format!("while [ ! -e '{}' ]; do :; done; exec '{thc}' --json {}", go.display(), args.iter().map(|a| format!("'{a}'")).collect::<Vec<_>>().join(" "));
                let mut sh = Command::new("/bin/sh");
                common::sandbox(&mut sh);
                self.env(&mut sh, &agent);
                sh.arg("-c").arg(script).stdout(Stdio::piped()).stderr(Stdio::piped());
                (agent, sh.spawn().unwrap())
            })
            .collect();
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(&go, "").unwrap();
        kids.into_iter()
            .map(|(a, k)| {
                let o = k.wait_with_output().unwrap();
                (a, o.status.code(), String::from_utf8_lossy(&o.stderr).to_string())
            })
            .collect()
    }
}

impl Drop for V {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn one_winner(v: &V, label: &str, guard: impl Fn(&str) -> Vec<String>) {
    for round in 0..5 {
        let (id, rev) = v.task(&format!("{label} task {round}"));
        let g = guard(&rev);
        let g: Vec<&str> = g.iter().map(String::as_str).collect();
        let r = v.race(&id, 8, &g);
        let won: Vec<&(String, Option<i32>, String)> = r.iter().filter(|x| x.1 == Some(0)).collect();
        assert_eq!(won.len(), 1, "{label} round {round}: exactly one winner: {r:#?}");
        for (a, code, err) in &r {
            if *code != Some(0) {
                assert_eq!(*code, Some(4), "{label}: {a} lost with exit 4: {err}");
                assert!(err.contains("\"stale\""), "{label}: {a}: {err}");
            }
        }
        let claims = v.claims(&id);
        assert_eq!(claims.len(), 1, "{label} round {round}: one claim tx: {claims:#?}");
        let n = &v.json(&["show", &id]);
        assert_eq!(n["props"]["owner"], won[0].0.as_str(), "the winner owns it");
        assert_eq!(n["props"]["session_id"], format!("s-{}", won[0].0.trim_start_matches("agent")), "owner and session in one write");
    }
}

fn claim_contract(v: &V) {
    // Racing on status, and on the revision read.
    one_winner(v, "expect", |_| vec!["--expect".into(), "status=todo".into()]);
    one_winner(v, "if-match", |rev| vec!["--if-match".into(), rev.into()]);
    // A stale revision: nothing written, exit 4.
    let (id, rev) = v.task("stale");
    v.json(&["set", &id, "priority=high"]);
    let o = v.cmd("agent9").args(["--json", "set", &id, "status=doing", "owner=agent9", "--if-match", &rev]).output().unwrap();
    assert_eq!(o.status.code(), Some(4));
    assert!(v.claims(&id).is_empty());
    // Not eligible (done, or already someone's): exit 4, nothing written.
    let (id, _) = v.task("done already");
    v.json(&["done", &id]);
    let o = v.cmd("agent9").args(["--json", "set", &id, "status=doing", "owner=agent9", "--expect", "status=todo"]).output().unwrap();
    assert_eq!(o.status.code(), Some(4));
    assert!(v.claims(&id).is_empty());
    // A crashed winner retrying its claim doesn't claim twice: the retry is stale (exit 4) and
    // it recovers by reading that it already owns the task.
    let (id, _) = v.task("retry");
    let claim = ["--json", "set", &id, "status=doing", "owner=agent1", "--expect", "status=todo"];
    assert!(v.cmd("agent1").args(claim).output().unwrap().status.success());
    assert_eq!(v.cmd("agent1").args(claim).output().unwrap().status.code(), Some(4));
    assert_eq!(v.claims(&id).len(), 1);
    assert_eq!(v.json(&["show", &id])["props"]["owner"], "agent1");
    // Release checks the claim it holds: after a reassignment the old worker's release (on its
    // claim's rev) is refused.
    let a_rev = v.json(&["show", &id])["rev"].as_str().unwrap().to_string();
    v.json(&["set", &id, "owner=agent2"]);
    let o = v.cmd("agent1").args(["--json", "set", &id, "status=todo", "owner=", "--if-match", &a_rev]).output().unwrap();
    assert_eq!(o.status.code(), Some(4), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(v.json(&["show", &id])["props"]["owner"], "agent2");
}

#[test]
fn racing_claims_have_one_winner() {
    claim_contract(&V::new("plain"));
}

#[test]
fn racing_claims_have_one_winner_with_the_daemon_running() {
    let v = V::new("daemon");
    let mut c = v.cmd("human");
    c.args(["daemon", "run"]).stdout(Stdio::null()).stderr(Stdio::null());
    let _d = common::Guard::spawn(&mut c);
    let deadline = Instant::now() + Duration::from_secs(15);
    while v.cmd("human").args(["--json", "daemon", "status"]).output().map(|o| String::from_utf8_lossy(&o.stdout).contains("\"live\"")).unwrap_or(false) == false {
        assert!(Instant::now() < deadline, "the daemon never went live");
        std::thread::sleep(Duration::from_millis(200));
    }
    claim_contract(&v);
    let _ = v.cmd("human").args(["daemon", "stop"]).output();
}
