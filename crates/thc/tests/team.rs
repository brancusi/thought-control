//! Every terminal host here is a fake executable on PATH. Never touches real panes or vaults.
mod common;
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output},
};

struct Project {
    root: PathBuf,
    home: PathBuf,
    config: PathBuf,
    shims: PathBuf,
}
impl Project {
    fn profile(&self, text: &str) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(self.root.join(".thc.toml"))
            .unwrap();
        writeln!(file, "{text}").unwrap();
    }
    fn new(name: &str) -> Self {
        let root = common::root().join(name);
        let home = root.join("home");
        let config = root.join("config");
        let shims = root.join("shims");
        for p in [&root, &home, &config, &shims] {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(
            config.join("config.toml"),
            "[actors.codex-engineer-4]\nallow = [\"team\"]\n",
        )
        .unwrap();
        let p = Self {
            root,
            home,
            config,
            shims,
        };
        p.fake_hosts();
        p
    }
    fn cmd(&self) -> Command {
        let mut c = common::thc();
        let path = format!(
            "{}:{}",
            self.shims.display(),
            std::env::var("PATH").unwrap()
        );
        c.current_dir(&self.root)
            .env("HOME", &self.home)
            .env("THC_CONFIG_DIR", &self.config)
            .env("THC_ACTOR", "codex-engineer-4")
            .env("THC_TEST_ROOT", &self.root)
            .env("PATH", path)
            .env("TEAM_SHIM_ROOT", &self.root)
            .env_remove("THC_VAULT")
            .env_remove("THC_CACHE_DIR")
            .env_remove("THC_BOARD")
            .env_remove("THC_ROLE")
            .env_remove("HERDR_ENV")
            .env_remove("WEZTERM_PANE")
            .env_remove("HERDR_SOCKET_PATH");
        c
    }
    fn run(&self, args: &[&str]) -> Value {
        good(self.cmd().arg("--json").args(args).output().unwrap())
    }
    fn board(&self) {
        self.run(&["init", "capture"]);
        std::fs::create_dir_all(self.root.join("board")).unwrap();
        good(
            self.cmd()
                .current_dir(self.root.join("board"))
                .args(["--json", "init", "vault"])
                .output()
                .unwrap(),
        );
        good(
            self.cmd()
                .args([
                    "--json",
                    "--vault",
                    self.root.join("board/vault").to_str().unwrap(),
                    "page",
                    "new",
                    "Issues",
                ])
                .output()
                .unwrap(),
        );
        std::fs::write(
            self.root.join(".thc.toml"),
            format!(
                "vault = 'capture'\nboard = '{}:¶ Issues'\n",
                self.root.join("board/vault").display()
            ),
        )
        .unwrap();
    }
    fn herdr(&self) -> Command {
        let mut c = self.cmd();
        c.env("HERDR_ENV", "1")
            .env("WEZTERM_PANE", "77")
            .env("HERDR_SOCKET_PATH", self.root.join("fake.socket"));
        c
    }
    fn log(&self) -> Vec<Value> {
        std::fs::read_to_string(self.root.join("host.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
    fn fake_hosts(&self) {
        let body = r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
root=Path(os.environ['TEAM_SHIM_ROOT']); tool=Path(sys.argv[0]).name; args=sys.argv[1:]
with (root/'host.log').open('a') as f: f.write(json.dumps({'tool':tool,'args':args,'socket':os.environ.get('HERDR_SOCKET_PATH')})+'\n')
p=root/'host-state.json'; state=json.loads(p.read_text()) if p.exists() else {'panes':[],'agents':[],'next':1}
def save(): p.write_text(json.dumps(state))
def out(v): print(json.dumps({'result':v}))
if tool=='herdr':
 if args[:2]==['agent','list']: out({'agents':state['agents']})
 elif args[:2]==['pane','list']: out({'panes':state['panes']+[{'pane_id':'w1:unowned'}]})
 elif args[:2]==['tab','create']:
  pane='w1:p'+str(state['next']);state['next']+=1
  tab=1+max([p.get('tab_id',0) for p in state['panes']]+[0]);created={'pane_id':pane,'terminal_id':'term_'+pane,'tab_id':tab,'left_col':0,'top_row':0,'size':{'rows':120,'cols':180}};state['panes'].append(created);save();out({'root_pane':created,'tab':{'tab_id':'w1:t'+str(tab)}})
 elif args[:2]==['pane','split']:
  pane='w1:p'+str(state['next']);state['next']+=1;created={'pane_id':pane,'terminal_id':'term_'+pane}
  parent=next((p for p in state['panes'] if p['pane_id']==args[2]),None)
  if parent and 'size' in parent:
   created=json.loads(json.dumps(parent));ratio=float(args[args.index('--ratio')+1]) if '--ratio' in args else 0.5
   axis='cols' if args[args.index('--direction')+1]=='right' else 'rows';position='left_col' if axis=='cols' else 'top_row'
   before=parent['size'][axis];parent['size'][axis]=max(1,int((before-1)*ratio));created['size'][axis]=before-1-parent['size'][axis];created[position]=parent[position]+parent['size'][axis]+1;created.update({'pane_id':pane,'terminal_id':'term_'+pane})
  state['panes'].append(created);save();out({'pane':created})
 elif args[:2]==['agent','start']:
  if os.environ.get('TEAM_FAIL_START'): print('start failed',file=sys.stderr);sys.exit(1)
  state['agents'].append({'name':args[2],'pane_id':args[args.index('--pane')+1],'agent_status':'idle'});save();out({})
 elif args[:2]==['pane','close']:
  state['panes']=[p for p in state['panes'] if p['pane_id']!=args[2]];state['agents']=[a for a in state['agents'] if a['pane_id']!=args[2]];save();out({})
 else: out({})
else:
 if args[:2]==['cli','list']: print(json.dumps(state['panes']+[{'pane_id':999}]))
 elif args[1] in ['spawn','split-pane']:
  pane=state['next'];state['next']+=1
  if args[1]=='spawn':
   tab=1+max([p.get('tab_id',0) for p in state['panes']]+[0]);created={'tab_id':tab,'left_col':0,'top_row':0,'size':{'rows':120,'cols':120}}
  else:
   parent=next(p for p in state['panes'] if p['pane_id']==int(args[args.index('--pane-id')+1]));created=json.loads(json.dumps(parent))
   percent=int(args[args.index('--percent')+1]) if '--percent' in args else None
   if '--right' in args:
    n=int(args[args.index('--cells')+1]) if '--cells' in args else max(1,(parent['size']['cols']-1)*percent//100);parent['size']['cols']-=n+1;created['size']['cols']=n;created['left_col']=parent['left_col']+parent['size']['cols']+1
   else:
    n=int(args[args.index('--cells')+1]) if '--cells' in args else max(1,(parent['size']['rows']-1)*percent//100);parent['size']['rows']-=n+1;created['size']['rows']=n;created['top_row']=parent['top_row']+parent['size']['rows']+1
  created.update({'pane_id':pane,'tty_name':'/fake/tty'+str(pane)});state['panes'].append(created);save();print(pane)
 elif args[1]=='adjust-pane-size':
  current=next(p for p in state['panes'] if p['pane_id']==int(args[args.index('--pane-id')+1]));delta=int(args[args.index('--amount')+1])*(1 if args[-1]=='Down' else -1)
  below=sorted([p for p in state['panes'] if p['tab_id']==current['tab_id'] and p['left_col']==current['left_col'] and p['top_row']>current['top_row']],key=lambda p:p['top_row'])
  current['size']['rows']+=delta
  for i in range(abs(delta)): below[i%len(below)]['size']['rows']-=1 if delta>0 else -1
  top=current['top_row']+current['size']['rows']+1
  for item in below: item['top_row']=top;top+=item['size']['rows']+1
  save();print('{}')
 elif args[1]=='kill-pane':
  pane=int(args[args.index('--pane-id')+1]);state['panes']=[p for p in state['panes'] if p['pane_id']!=pane];save();print('{}')
 else: print('{}')
"#;
        for name in ["herdr", "wezterm"] {
            let f = self.shims.join(name);
            std::fs::write(&f, body).unwrap();
            std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
}

#[test]
fn configured_roster_launches_mixed_agents_counts_and_native_models() {
    let p = Project::new("team-profile-models");
    p.board();
    std::fs::write(
        p.config.join("config.toml"),
        "[actors.codex-engineer-4]\nallow=['team']\n[team]\nmax_agents=7\n",
    )
    .unwrap();
    p.profile(
        r#"
[[team.roster]]
role = "pm"
agent = "claude"
model = "claude-opus-5-5"
[[team.roster]]
role = "designer"
agent = "claude"
model = "claude-opus-5-5"
[[team.roster]]
role = "engineer"
agent = "claude"
model = "claude-opus-5-5"
[[team.roster]]
role = "merger"
agent = "claude"
model = "claude-opus-5-5"
[[team.roster]]
role = "reviewer"
agent = "claude"
model = "claude-haiku-4-5-20251001"
[[team.roster]]
role = "engineer"
agent = "codex"
model = "gpt-6.1-sol"
count = 2
"#,
    );
    let result = good(p.herdr().args(["--json", "team", "up"]).output().unwrap());
    let members = result["team"].as_array().unwrap();
    assert_eq!(members.len(), 7);
    assert_eq!(members[5]["actor"], "codex-engineer");
    assert_eq!(members[6]["actor"], "codex-engineer-2");
    assert_eq!(members[4]["model"], "claude-haiku-4-5-20251001");
    assert_eq!(result["model_passed"], true);
    let starts: Vec<_> = p
        .log()
        .into_iter()
        .filter(|c| c["args"][1] == "start")
        .collect();
    assert_eq!(starts.len(), 7);
    assert_eq!(
        &starts[0]["args"].as_array().unwrap()[7..],
        &[json!("--"), json!("--model"), json!("claude-opus-5-5")]
    );
    let codex = starts
        .iter()
        .find(|c| c["args"][2] == "codex-engineer-2")
        .unwrap();
    assert_eq!(
        &codex["args"].as_array().unwrap()[7..],
        &[json!("--"), json!("-m"), json!("gpt-6.1-sol")]
    );
    assert!(
        starts
            .iter()
            .all(|c| !c["args"].to_string().contains("Coordinate only through"))
    );
    assert_eq!(p.run(&["team", "ls"])["team"][6]["model"], "gpt-6.1-sol");
}

#[test]
fn configured_roster_overrides_and_dry_run_do_not_launch_or_save() {
    let p = Project::new("team-profile-overrides");
    p.board();
    p.profile(
        r#"
[[team.roster]]
role="reviewer"
agent="claude"
model="haiku"
[[team.roster]]
role="engineer"
agent="codex"
model="gpt"
count=2
"#,
    );
    let result = p.run(&[
        "--dry-run",
        "team",
        "up",
        "--agent",
        "engineer=claude",
        "--model",
        "m's model",
    ]);
    assert_eq!(
        result["team"][0]["role"], "reviewer",
        "configured outline order is preserved"
    );
    assert_eq!(result["team"][2]["actor"], "claude-engineer-2");
    assert!(
        result["commands"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["command"].as_str().unwrap().contains("'m'\\''s model'"))
    );
    assert!(p.log().is_empty());
    assert!(
        p.run(&["team", "ls"])["team"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let explicit = p.run(&["--dry-run", "team", "up", "pm", "--agent", "codex"]);
    assert_eq!(explicit["team"].as_array().unwrap().len(), 1);
    assert_eq!(explicit["team"][0]["actor"], "codex-pm");
    assert!(explicit["team"][0]["model"].is_null());
}

#[test]
fn invalid_profiles_refuse_before_hosts_or_board_creation() {
    for (label, body, hint) in [
        (
            "zero",
            "[[team.roster]]\nrole='engineer'\nagent='codex'\ncount=0",
            "count must be positive",
        ),
        (
            "limit",
            "[[team.roster]]\nrole='engineer'\nagent='codex'\ncount=99",
            "max_agents",
        ),
        (
            "unsafe",
            "[[team.roster]]\nrole='engineer'\nagent='codex'\nmodel='--yolo'",
            "permission prompts",
        ),
        (
            "typo",
            "[[team.roster]]\nrole='engineer'\nagent='codex'\ncout=2",
            "unknown field",
        ),
        ("empty", "[team]\nroster=[]", "at least one"),
    ] {
        let p = Project::new(&format!("team-profile-invalid-{label}"));
        std::fs::write(p.root.join(".thc.toml"), body).unwrap();
        let output = p
            .herdr()
            .args(["--json", "team", "up", "--new-board", "invalid-profile"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(6),
            "{label}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(hint),
            "{label}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            p.log().is_empty(),
            "{label} contacted host before validating"
        );
        assert!(!p.home.join("thought-vaults").exists());
    }
    let p = Project::new("team-profile-missing");
    p.board();
    let output = p.cmd().args(["--json", "team", "up"]).output().unwrap();
    assert_eq!(output.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no team roster"));
}
fn good(o: Output) -> Value {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).unwrap()
}

#[test]
fn profile_layout_creates_balanced_columns_and_pins_pm_on_both_hosts() {
    for host in ["herdr", "wezterm"] {
        let p = Project::new(&format!("team-profile-layout-{host}"));
        p.board();
        p.profile(
            r#"
[team.layout]
columns=3
pm="bottom-right"
tab="team"
[[team.roster]]
role="pm"
agent="claude"
[[team.roster]]
role="engineer"
agent="codex"
count=5
"#,
        );
        let mut command = if host == "herdr" {
            p.herdr()
        } else {
            let mut command = p.cmd();
            command.env("WEZTERM_PANE", "77");
            command
        };
        let result = good(command.args(["--json", "team", "up"]).output().unwrap());
        let state: Value =
            serde_json::from_slice(&std::fs::read(p.root.join("host-state.json")).unwrap())
                .unwrap();
        let panes = state["panes"].as_array().unwrap();
        assert_eq!(panes.len(), 6);
        let pm = panes
            .iter()
            .find(|p| {
                let id = p["pane_id"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| p["pane_id"].to_string());
                Some(id.as_str()) == result["team"][0]["pane"].as_str()
            })
            .unwrap();
        let columns: std::collections::BTreeSet<_> = panes
            .iter()
            .map(|p| p["left_col"].as_u64().unwrap())
            .collect();
        assert_eq!(columns.len(), 3);
        assert_eq!(pm["left_col"].as_u64().unwrap(), *columns.last().unwrap());
        assert!(pm["top_row"].as_u64().unwrap() > 0, "PM is the bottom row");
        let widths: Vec<_> = panes
            .iter()
            .map(|p| p["size"]["cols"].as_u64().unwrap())
            .collect();
        assert!(
            widths.iter().max().unwrap() - widths.iter().min().unwrap() <= 2,
            "{host}: {widths:?}"
        );
        let heights: Vec<_> = panes
            .iter()
            .map(|p| p["size"]["rows"].as_u64().unwrap())
            .collect();
        assert!(
            heights.iter().max().unwrap() - heights.iter().min().unwrap() <= 1,
            "{host}: {heights:?}"
        );
        let log = p.log();
        if host == "herdr" {
            let tabs: Vec<_> = log
                .iter()
                .filter(|c| c["args"][0] == "tab" && c["args"][1] == "create")
                .collect();
            assert_eq!(tabs.len(), 1);
            assert!(tabs[0]["args"].to_string().contains("--no-focus"));
            assert!(
                tabs[0]["args"]
                    .to_string()
                    .contains("THC_ACTOR=codex-engineer")
            );
            assert!(
                log.iter()
                    .filter(|c| c["args"][0] == "pane" && c["args"][1] == "split")
                    .all(|c| c["args"][2].as_str().unwrap().starts_with("w1:p"))
            );
        } else {
            assert_eq!(log.iter().filter(|c| c["args"][1] == "spawn").count(), 1);
            assert!(
                log.iter()
                    .any(|c| c["args"][1] == "set-tab-title" && c["args"][4] == "team")
            );
        }
        assert_eq!(p.run(&["--yes", "team", "down"])["closed"], 6);
        let closes: Vec<_> = p
            .log()
            .into_iter()
            .filter(|c| matches!(c["args"][1].as_str(), Some("close" | "kill-pane")))
            .collect();
        assert_eq!(closes.len(), 6);
        assert!(
            closes
                .iter()
                .all(|c| !c["args"].to_string().contains("unowned")
                    && !c["args"].to_string().contains("999"))
        );
    }
}

#[test]
fn profile_permission_modes_are_human_only_and_separate_from_prompt() {
    let p = Project::new("team-profile-permissions");
    p.board();
    p.profile(
        r#"
[[team.roster]]
role="reviewer"
agent="claude"
model="haiku"
permission_mode="manual"
"#,
    );
    let denied = p
        .herdr()
        .args(["--json", "--dry-run", "team", "up"])
        .output()
        .unwrap();
    assert_eq!(denied.status.code(), Some(6));
    assert!(
        String::from_utf8_lossy(&denied.stderr).contains("agents cannot choose a permission mode")
    );
    assert!(p.log().is_empty());
    good(
        p.herdr()
            .env("THC_ACTOR", "human")
            .args(["--json", "team", "up"])
            .output()
            .unwrap(),
    );
    let start = p
        .log()
        .into_iter()
        .find(|c| c["args"][1] == "start")
        .unwrap();
    assert_eq!(
        &start["args"].as_array().unwrap()[7..],
        &[
            json!("--"),
            json!("--model"),
            json!("haiku"),
            json!("--permission-mode"),
            json!("manual")
        ]
    );
}

#[test]
fn herdr_wins_launches_with_board_identity_and_down_closes_only_saved_panes() {
    let p = Project::new("team-herdr");
    p.board();
    let a = good(
        p.herdr()
            .args([
                "--json",
                "team",
                "up",
                "pm",
                "designer",
                "engineer",
                "--agent",
                "claude",
                "--agent",
                "engineer=codex",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(a["host"], "herdr");
    assert_eq!(a["team"].as_array().unwrap().len(), 3);
    let log = p.log();
    assert!(log.iter().all(|c| c["tool"] == "herdr"));
    let splits: Vec<_> = log
        .iter()
        .filter(|c| c["args"][0] == "pane" && c["args"][1] == "split")
        .collect();
    assert_eq!(splits.len(), 3);
    let joined = splits[2]["args"].to_string();
    assert!(joined.contains("THC_ACTOR=codex-engineer"));
    assert!(joined.contains("THC_ROLE=engineer"));
    assert!(joined.contains("board/vault:¶ Issues"));
    assert!(!joined.contains("THC_BOARD=capture"));
    let prompts: Vec<_> = log.iter().filter(|c| c["args"][1] == "prompt").collect();
    assert_eq!(prompts.len(), 3);
    assert!(
        prompts[2]["args"][3]
            .as_str()
            .unwrap()
            .contains("thc prime --role engineer")
    );
    assert!(!log.iter().any(|c| c["args"].to_string().contains("--yolo")));
    let listed = p.run(&["team", "ls"]);
    assert_eq!(listed["team"][0]["state"], "idle");
    let nested = p.root.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let denied = p
        .cmd()
        .current_dir(&nested)
        .args(["team", "down"])
        .output()
        .unwrap();
    assert_eq!(denied.status.code(), Some(6));
    let d = good(
        p.cmd()
            .current_dir(&nested)
            .args(["--json", "--yes", "team", "down"])
            .output()
            .unwrap(),
    );
    assert_eq!(d["closed"], 3);
    let closes: Vec<_> = p
        .log()
        .into_iter()
        .filter(|c| c["args"][1] == "close")
        .collect();
    assert_eq!(closes.len(), 3);
    assert!(closes.iter().all(|c| c["args"][2] != "w1:unowned"));
    assert_eq!(p.run(&["team", "ls"])["team"][0]["state"], "stopped");
}

#[test]
fn partial_start_failure_is_remembered_and_replaced_agent_is_not_closed() {
    let p = Project::new("team-partial");
    p.board();
    let o = p
        .herdr()
        .env("TEAM_FAIL_START", "1")
        .args(["team", "up", "engineer", "--agent", "codex"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(6));
    let ls = p.run(&["team", "ls"]);
    assert_eq!(ls["team"][0]["pane"], "w1:p1");
    let state = p.root.join("host-state.json");
    let mut v: Value = serde_json::from_slice(&std::fs::read(&state).unwrap()).unwrap();
    v["agents"] = json!([{"pane_id":"w1:p1","name":"someone-else","agent_status":"working"}]);
    std::fs::write(&state, v.to_string()).unwrap();
    let o = p.cmd().args(["--yes", "team", "down"]).output().unwrap();
    assert_eq!(o.status.code(), Some(6));
    assert!(!p.log().iter().any(|c| c["args"][1] == "close"));
    v["agents"] = json!([]);
    v["panes"][0]["terminal_id"] = json!("replacement");
    std::fs::write(&state, v.to_string()).unwrap();
    let o = p.cmd().args(["--yes", "team", "down"]).output().unwrap();
    assert_eq!(o.status.code(), Some(6));
    assert!(!p.log().iter().any(|c| c["args"][1] == "close"));
    v["panes"][0]["terminal_id"] = json!("term_w1:p1");
    std::fs::write(&state, v.to_string()).unwrap();
    assert_eq!(p.run(&["--yes", "team", "down"])["closed"], 1);
}

#[test]
fn wezterm_layout_and_printed_commands_and_readonly_dry_run() {
    let p = Project::new("team-wezterm");
    p.board();
    let a = good(
        p.cmd()
            .env("WEZTERM_PANE", "77")
            .args([
                "--json",
                "team",
                "up",
                "pm",
                "designer",
                "engineer",
                "reviewer",
                "--agent",
                "codex",
                "--model",
                "test-model",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(a["host"], "wezterm");
    let log = p.log();
    assert!(!log.iter().any(|c| c["tool"] == "herdr"));
    let spawn = log.iter().find(|c| c["args"][1] == "spawn").unwrap();
    assert!(spawn["args"].to_string().contains("THC_BOARD="));
    assert!(spawn["args"].to_string().contains("test-model"));
    let split: Vec<_> = log
        .iter()
        .filter(|c| c["args"][1] == "split-pane")
        .collect();
    assert_eq!(split.len(), 3);
    assert!(split[0]["args"].to_string().contains("--right"));
    assert!(split[1]["args"].to_string().contains("66"));
    assert_eq!(p.run(&["--yes", "team", "down"])["closed"], 4);
    let q = Project::new("team-printed");
    q.board();
    let printed = q.run(&["team", "up", "engineer", "--agent", "codex"]);
    assert_eq!(printed["host"], "printed");
    assert!(
        printed["commands"][0]["command"]
            .as_str()
            .unwrap()
            .contains("thc prime --role engineer")
    );
    assert!(q.log().is_empty());
    let o = q
        .cmd()
        .args(["--readonly", "team", "up", "pm"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(6));
    q.run(&["--dry-run", "team", "up", "pm"]);
    assert!(q.log().is_empty());
}

#[test]
fn new_project_requires_consent_then_seeds_board_and_first_plan_gate() {
    let p = Project::new("team-fresh");
    for args in [
        vec!["team", "ls"],
        vec!["--yes", "team", "down"],
        vec!["--yes", "team", "up", "pm"],
    ] {
        let output = p.cmd().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(6));
    }
    std::fs::write(
        p.root.join("README.md"),
        "# Acme Site\n\nA useful project.\n",
    )
    .unwrap();
    let o = p
        .cmd()
        .args(["team", "up", "pm", "engineer"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(6));
    assert!(!p.root.join(".thc.toml").exists());
    assert!(!p.home.join("thought-vaults").exists());
    p.run(&[
        "--dry-run",
        "team",
        "up",
        "pm",
        "engineer",
        "--new-board",
        "acme-site",
    ]);
    assert!(!p.home.join("thought-vaults").exists());
    let a = p.run(&["team", "up", "pm", "engineer", "--new-board", "acme-site"]);
    assert_eq!(a["board"]["vault"], "acme-site");
    let vault = p.home.join("thought-vaults/acme-site");
    assert!(vault.join("settings.toml").is_file());
    let pages = p.run(&["pages"]);
    let pages = pages["items"].as_array().unwrap();
    for name in ["Issues", "About", "Roles"] {
        assert!(pages.iter().any(|v| v["title"] == name));
    }
    let about_id = pages.iter().find(|v| v["title"] == "About").unwrap()["id"]
        .as_str()
        .unwrap();
    let about = p.run(&["show", about_id, "--depth", "1"]);
    assert!(about.to_string().contains("A useful project."));
    let first = p.run(&["q", "text:First is:task"])["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        p.run(&["next", "--role", "engineer"])["waiting"],
        "first-plan"
    );
    assert_eq!(
        p.run(&["prime", "--role", "engineer"])["waiting"],
        "first-plan"
    );
    assert_eq!(
        good(
            p.cmd()
                .env("THC_ACTOR", "claude-pm")
                .args(["--json", "next", "--role", "pm"])
                .output()
                .unwrap()
        )["items"][0]["id"],
        first
    );
    let done = good(
        p.cmd()
            .env("THC_ACTOR", "claude-pm")
            .args(["--json", "done", &first])
            .output()
            .unwrap(),
    );
    assert_eq!(
        p.run(&["next", "--role", "engineer"])["waiting"],
        "first-plan"
    );
    assert!(
        good(
            p.cmd()
                .env("THC_ACTOR", "claude-pm")
                .args(["--json", "prime", "--role", "pm"])
                .output()
                .unwrap()
        )["first_plan"]
            .as_str()
            .unwrap()
            .contains("accept it in review")
    );
    let accepted = good(
        p.cmd()
            .env("THC_ACTOR", "human")
            .args(["--json", "review", "accept", done["tx"].as_str().unwrap()])
            .output()
            .unwrap(),
    );
    let ready = p
        .cmd()
        .args(["--json", "next", "--role", "engineer"])
        .output()
        .unwrap();
    assert_eq!(
        ready.status.code(),
        Some(3),
        "no tasks, but no waiting gate"
    );
    good(
        p.cmd()
            .env("THC_ACTOR", "human")
            .args(["--json", "undo", "--tx", accepted["tx"].as_str().unwrap()])
            .output()
            .unwrap(),
    );
    assert_eq!(
        p.run(&["next", "--role", "engineer"])["waiting"],
        "first-plan"
    );
    good(
        p.cmd()
            .env("THC_ACTOR", "human")
            .args(["--json", "rm", &first])
            .output()
            .unwrap(),
    );
    assert_eq!(
        p.cmd()
            .args(["--json", "next", "--role", "engineer"])
            .output()
            .unwrap()
            .status
            .code(),
        Some(3)
    );
}

#[test]
fn roster_joins_claims_and_independent_read_receipts_without_mutating_them() {
    let p = Project::new("team-roster");
    p.board();
    p.run(&["team", "up", "engineer", "engineer", "--agent", "codex"]);
    let board = p.root.join("board/vault");
    let board = board.to_str().unwrap();
    let issue = p.run(&["--vault", board, "q", "is:page text:Issues"])["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let task = p.run(&[
        "--vault",
        board,
        "todo",
        "Implement feature",
        "--under",
        &issue,
    ])["nodes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    p.run(&[
        "--vault",
        board,
        "set",
        &task,
        "status=doing",
        "owner=codex-engineer",
    ]);
    let msg = p.run(&["msg", "engineer", "Check the spec", "--on", &task])["nodes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    good(
        p.cmd()
            .env("THC_ACTOR", "codex-engineer")
            .args(["--json", "msg", "read", &msg])
            .output()
            .unwrap(),
    );
    let ls = p.run(&["team", "ls"]);
    assert_eq!(ls["team"][0]["claims"][0]["id"], task);
    assert_eq!(ls["team"][0]["task"], task);
    assert_eq!(ls["team"][0]["unread"], 0);
    assert_eq!(ls["team"][1]["unread"], 1);
    assert!(ls["team"][0]["last_activity"].is_string());
    let prime = p.run(&["prime", "--role", "reviewer"]);
    assert_eq!(prime["team"][0]["task"], task);
    assert_eq!(p.run(&["team", "ls"])["team"][1]["unread"], 1);
}

/// Only stdin needs a terminal; pipe output so a child can't block filling the pty buffer.
fn terminal(p: &Project, args: &[&str], answer: &str) -> Output {
    use std::{
        io::Write,
        os::fd::{FromRawFd, OwnedFd},
        process::Stdio,
    };
    let (mut master, slave) = unsafe {
        let (mut m, mut s) = (0, 0);
        assert_eq!(
            libc::openpty(
                &mut m,
                &mut s,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut()
            ),
            0
        );
        (
            std::fs::File::from(OwnedFd::from_raw_fd(m)),
            OwnedFd::from_raw_fd(s),
        )
    };
    let mut command = p.cmd();
    command
        .env("THC_ACTOR", "human")
        .args(args)
        .stdin(Stdio::from(slave))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().unwrap();
    master.write_all(answer.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn terminal_consent_and_registered_short_name_match_preserve_capture_vault() {
    let p = Project::new("team-offer");
    p.run(&["init", "capture"]);
    good(
        p.cmd()
            .env("THC_ACTOR", "human")
            .args(["--json", "vault", "new", "existing"])
            .output()
            .unwrap(),
    );
    let registered = p.home.join("thought-vaults/existing");
    let settings = registered.join("settings.toml");
    let mut table: toml::Table =
        toml::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    table
        .entry("vault")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .unwrap()
        .insert(
            "name_short".into(),
            toml::Value::String("team-offer".into()),
        );
    std::fs::write(settings, toml::to_string(&table).unwrap()).unwrap();
    let no = terminal(&p, &["team", "up", "pm", "engineer"], "n\n");
    assert_eq!(no.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&no.stderr).contains("use vault existing"));
    let yes = terminal(&p, &["team", "up", "pm", "engineer"], "y\n");
    assert!(
        yes.status.success(),
        "{}",
        String::from_utf8_lossy(&yes.stderr)
    );
    assert!(!p.home.join("thought-vaults/team-offer").exists());
    let project: toml::Table =
        toml::from_str(&std::fs::read_to_string(p.root.join(".thc.toml")).unwrap()).unwrap();
    assert_eq!(project["vault"].as_str(), Some("capture"));
    assert_eq!(project["board"].as_str(), Some("existing:¶ Issues"));
}

#[test]
fn unsafe_launchers_and_missing_lead_fail_before_creation_or_host_mutation() {
    let p = Project::new("team-unsafe");
    std::fs::write(p.config.join("config.toml"),"[actors.codex-engineer-4]\nallow=['team']\n[team.agents.bad]\ncommand=['sh','-c','codex --yolo']\n").unwrap();
    let o = p
        .cmd()
        .args([
            "team",
            "up",
            "pm",
            "--agent",
            "bad",
            "--new-board",
            "unsafe",
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(6));
    assert!(!p.home.join("thought-vaults").exists());
    assert!(p.log().is_empty());
    let o = p
        .cmd()
        .args(["team", "up", "engineer", "--new-board", "no-lead"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(6));
    assert!(!p.home.join("thought-vaults").exists());
}

impl Project {
    fn human(&self) -> Command {
        let mut c = self.cmd();
        c.env("THC_ACTOR", "human");
        c
    }
    fn start(&self) {
        good(
            self.herdr()
                .args(["--json", "team", "up", "pm", "engineer", "--agent", "codex"])
                .output()
                .unwrap(),
        );
    }
    fn task(&self) -> String {
        let issues = self.run(&["board"])["page"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let task = good(
            self.cmd()
                .args([
                    "--json",
                    "--vault",
                    self.root.join("board/vault").to_str().unwrap(),
                    "todo",
                    "A delegated task",
                    "--under",
                    &issues,
                ])
                .output()
                .unwrap(),
        );
        task["nodes"][0]["id"].as_str().unwrap().to_string()
    }
    fn show_board(&self, id: &str) -> Value {
        good(
            self.cmd()
                .args([
                    "--json",
                    "--vault",
                    self.root.join("board/vault").to_str().unwrap(),
                    "show",
                    id,
                    "--depth",
                    "2",
                ])
                .output()
                .unwrap(),
        )
    }
}

#[test]
fn add_uses_current_herdr_pane_separate_prompt_and_audits_board_and_task() {
    let p = Project::new("member-herdr");
    p.board();
    p.start();
    let task = p.task();
    let result = good(
        p.human()
            .args([
                "--json", "team", "add", "engineer", "--agent", "codex", "--model", "o4", "--task",
                &task,
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(result["member"]["actor"], "codex-engineer-2");
    assert_eq!(result["member"]["model"], "o4");
    assert_eq!(p.show_board(&task)["status"], "todo");
    assert!(p.show_board(&task)["props"]["owner"].is_null());
    let log = p.log();
    let split = log
        .iter()
        .filter(|v| v["args"][1] == "split")
        .last()
        .unwrap();
    assert!(
        split["args"]
            .as_array()
            .unwrap()
            .contains(&json!("--current"))
    );
    assert!(split["args"].as_array().unwrap().contains(&json!("down")));
    assert!(!split["args"].as_array().unwrap().contains(&json!("w1:p2")));
    assert_eq!(
        split["socket"],
        p.root.join("fake.socket").to_str().unwrap()
    );
    let start = log
        .iter()
        .filter(|v| v["args"][1] == "start")
        .last()
        .unwrap();
    let args = start["args"].as_array().unwrap();
    assert_eq!(&args[7..], &[json!("--"), json!("-m"), json!("o4")]);
    assert!(!start["args"].to_string().contains("prime"));
    let prompt = log
        .iter()
        .filter(|v| v["args"][1] == "prompt")
        .last()
        .unwrap()["args"][3]
        .as_str()
        .unwrap();
    assert!(prompt.contains(&format!("thc set {task} status=doing")));
    assert!(prompt.contains("--expect status=todo"));
    assert!(prompt.contains("exit 0"));
    let claim = prompt
        .split("then claim it with ")
        .nth(1)
        .unwrap()
        .split(" and start only")
        .next()
        .unwrap();
    assert!(claim.contains("--vault"));
    let template = p.cmd();
    let mut shell = Command::new("/bin/sh");
    for (key, value) in template.get_envs() {
        if let Some(value) = value {
            shell.env(key, value);
        } else {
            shell.env_remove(key);
        }
    }
    let binary = common::thc();
    let program = binary.get_program().to_string_lossy();
    let command = claim.replacen("thc", &format!("'{program}'"), 1);
    good(
        shell
            .current_dir(&p.root)
            .env("THC_ACTOR", "codex-engineer-2")
            .args(["-c", &format!("{command} --json")])
            .output()
            .unwrap(),
    );
    assert_eq!(p.show_board(&task)["status"], "doing");
    assert_eq!(p.show_board(&task)["props"]["owner"], "codex-engineer-2");

    let note = &p.show_board(&task)["children"][0];
    assert!(
        note["text"]
            .as_str()
            .unwrap()
            .contains("human added codex-engineer-2 for this task")
    );
    assert_eq!(note["created_by"], "human");
    let pages = p.run(&[
        "--vault",
        p.root.join("board/vault").to_str().unwrap(),
        "pages",
    ]);
    let about_id = pages["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["title"] == "About")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let about = p.show_board(about_id);
    let heading = about["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["text"] == "## Members")
        .unwrap();
    assert!(
        heading["children"][0]["text"]
            .as_str()
            .unwrap()
            .contains("added codex-engineer-2")
    );
    assert!(
        p.run(&["pages"])["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["title"] != "About")
    );
}

#[test]
fn lead_delegation_requires_device_opt_in_even_with_full_tier() {
    let p = Project::new("member-policy");
    p.board();
    p.start();
    for actor in ["claude-pm", "codex-engineer-4"] {
        let o = p
            .cmd()
            .env("THC_ACTOR", actor)
            .args([
                "--json", "--yes", "team", "add", "engineer", "--agent", "codex",
            ])
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(6));
    }
    std::fs::write(
        p.config.join("config.toml"),
        "[actors.default-agent]
tier='full'
[team]
lead_can_add=true
max_agents=3
",
    )
    .unwrap();
    let denied = p
        .cmd()
        .env("THC_ROLE", "engineer")
        .args(["team", "add", "engineer", "--agent", "codex"])
        .output()
        .unwrap();
    assert_eq!(denied.status.code(), Some(6));
    let a = good(
        p.cmd()
            .env("THC_ACTOR", "claude-pm")
            .args(["--json", "team", "add", "engineer", "--agent", "codex"])
            .output()
            .unwrap(),
    );
    assert_eq!(a["member"]["added_by"], "claude-pm");
    let o = p
        .human()
        .args(["team", "add", "engineer", "--agent", "codex"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&o.stderr).contains("max_agents"));
    good(
        p.cmd()
            .env("THC_ACTOR", "claude-pm")
            .args(["--json", "team", "rm", "codex-engineer-2"])
            .output()
            .unwrap(),
    );
    // Opt-in permits a normal write-tier lead, without allowing unrelated machine commands.
    std::fs::write(
        p.config.join("config.toml"),
        "[team]
lead_can_add=true
",
    )
    .unwrap();
    good(
        p.cmd()
            .env("THC_ACTOR", "claude-pm")
            .args(["--json", "team", "add", "engineer", "--agent", "codex"])
            .output()
            .unwrap(),
    );
}

#[test]
fn rm_preserves_claims_requires_confirmation_and_reuses_lowest_free_name() {
    let p = Project::new("member-rm");
    p.board();
    p.start();
    let task = p.task();
    good(
        p.human()
            .args([
                "--json", "team", "add", "engineer", "--agent", "codex", "--task", &task,
            ])
            .output()
            .unwrap(),
    );
    p.run(&[
        "--vault",
        p.root.join("board/vault").to_str().unwrap(),
        "set",
        &task,
        "status=doing",
        "owner=codex-engineer-2",
    ]);
    let ambiguous = p.human().args(["team", "rm", "engineer"]).output().unwrap();
    assert_eq!(ambiguous.status.code(), Some(6));
    let warned = p
        .human()
        .args(["--json", "team", "rm", "codex-engineer-2"])
        .output()
        .unwrap();
    assert_eq!(warned.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&warned.stderr).contains(&task));
    good(
        p.human()
            .args(["--json", "--yes", "team", "rm", "codex-engineer-2"])
            .output()
            .unwrap(),
    );
    assert_eq!(p.show_board(&task)["status"], "doing");
    assert_eq!(p.show_board(&task)["props"]["owner"], "codex-engineer-2");
    assert!(
        p.show_board(&task)["children"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["text"]
                .as_str()
                .unwrap()
                .contains("removed codex-engineer-2"))
    );
    assert_eq!(p.run(&["team", "ls"])["team"].as_array().unwrap().len(), 2);
    let add = good(
        p.human()
            .args(["--json", "team", "add", "engineer", "--agent", "codex"])
            .output()
            .unwrap(),
    );
    assert_eq!(add["member"]["actor"], "codex-engineer-2");
    assert!(
        p.log()
            .iter()
            .filter(|v| v["args"][1] == "close")
            .all(|v| v["args"][2] != "w1:unowned")
    );
}

#[test]
fn unsafe_flags_and_invalid_tasks_are_refused_before_host_mutation() {
    let p = Project::new("member-validation");
    p.board();
    p.start();
    let count = p.log().len();
    for extra in [
        vec!["--permission-mode", "auto"],
        vec!["--task", "missing-task"],
        vec!["--model", "--dangerously-ignore-permissions"],
    ] {
        let o = p
            .human()
            .args(["team", "add", "engineer", "--agent", "codex"])
            .args(extra)
            .output()
            .unwrap();
        assert!(!o.status.success());
    }
    assert_eq!(
        p.log().iter().filter(|v| v["args"][1] == "split").count(),
        2
    );
    assert!(p.log().len() >= count);
    std::fs::write(
        p.config.join("config.toml"),
        "[team.agents.grok]
command=['grok','--permission-mode','bypassPermissions']
",
    )
    .unwrap();
    let denied = p
        .human()
        .args(["team", "add", "engineer", "--agent", "grok"])
        .output()
        .unwrap();
    assert_eq!(denied.status.code(), Some(6));
    assert_eq!(p.run(&["team", "ls"])["team"].as_array().unwrap().len(), 2);
    let agents = p.run(&["team", "agents"]);
    assert!(
        agents["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["name"] == "grok" && a["source"] == "config" && a["available"] == false)
    );
}

#[test]
fn claude_native_model_and_permission_mode_are_separate_from_prompt() {
    let p = Project::new("member-claude");
    p.board();
    p.start();
    good(
        p.human()
            .args([
                "--json",
                "team",
                "add",
                "reviewer",
                "--agent",
                "claude",
                "--model",
                "sonnet",
                "--permission-mode",
                "auto",
            ])
            .output()
            .unwrap(),
    );
    let log = p.log();
    let start = log
        .iter()
        .filter(|v| v["args"][1] == "start")
        .last()
        .unwrap();
    assert_eq!(
        &start["args"].as_array().unwrap()[7..],
        &[
            json!("--"),
            json!("--model"),
            json!("sonnet"),
            json!("--permission-mode"),
            json!("auto")
        ]
    );
    good(
        p.human()
            .args(["--json", "team", "add", "designer", "--agent", "claude"])
            .output()
            .unwrap(),
    );
    let log = p.log();
    let start = log
        .iter()
        .filter(|v| v["args"][1] == "start")
        .last()
        .unwrap();
    assert_eq!(
        start["args"].as_array().unwrap().len(),
        7,
        "omitted flags retain user defaults"
    );
}

#[test]
fn dry_run_readonly_and_partial_failures_leave_recoverable_members() {
    let p = Project::new("member-partial");
    p.board();
    p.start();
    let before = p.log().iter().filter(|v| v["args"][1] == "split").count();
    good(
        p.human()
            .args([
                "--json",
                "--dry-run",
                "team",
                "add",
                "engineer",
                "--agent",
                "codex",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(p.run(&["team", "ls"])["team"].as_array().unwrap().len(), 2);
    assert_eq!(
        p.log().iter().filter(|v| v["args"][1] == "split").count(),
        before
    );
    let ro = p
        .human()
        .args(["--readonly", "team", "add", "engineer", "--agent", "codex"])
        .output()
        .unwrap();
    assert_eq!(ro.status.code(), Some(6));
    let failed = p
        .human()
        .env("TEAM_FAIL_START", "1")
        .args(["team", "add", "engineer", "--agent", "codex"])
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(6));
    let team = p.run(&["team", "ls"]);
    let member = &team["team"][2];
    assert_eq!(member["actor"], "codex-engineer-2");
    assert!(member["pane"].is_string());
    good(
        p.human()
            .args(["--json", "team", "rm", "codex-engineer-2"])
            .output()
            .unwrap(),
    );
    assert_eq!(p.run(&["team", "ls"])["team"].as_array().unwrap().len(), 2);
}

#[test]
fn printed_custom_launcher_quotes_arguments_and_lists_sources() {
    let p = Project::new("member-printed");
    p.board();
    std::fs::write(
        p.config.join("config.toml"),
        "[actors.codex-engineer-4]
allow=['team']
[team.agents.grok]
command=['grok-cli','--normal']
model=['--choose','{model}']
prompt=['--prompt','{prompt}']
env={GROK_CONFIG='a path with spaces'}
",
    )
    .unwrap();
    p.run(&["team", "up", "pm", "--agent", "codex"]);
    let a = good(
        p.human()
            .args([
                "--json",
                "team",
                "add",
                "engineer",
                "--agent",
                "grok",
                "--model",
                "m's model",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(a["member"]["state"], "printed");
    let command = a["command"].as_str().unwrap();
    assert!(command.contains("'--choose'"));
    assert!(command.contains("'m'\\''s model'"));
    assert!(command.contains("GROK_CONFIG=a path with spaces"));
    let agents = p.run(&["team", "agents"]);
    assert!(
        agents["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["name"] == "grok" && a["source"] == "config")
    );
    assert!(
        agents["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["name"] == "codex" && a["source"] == "built-in")
    );
    good(
        p.human()
            .args(["--json", "team", "rm", "engineer"])
            .output()
            .unwrap(),
    );
}

#[test]
fn wezterm_add_balances_stack_and_opens_next_tab_after_five() {
    let p = Project::new("member-wez");
    p.board();
    std::fs::write(
        p.config.join("config.toml"),
        "[actors.codex-engineer-4]
allow=['team']
[team]
max_agents=9
",
    )
    .unwrap();
    good(
        p.cmd()
            .env("WEZTERM_PANE", "77")
            .args([
                "--json", "team", "up", "pm", "engineer", "engineer", "--agent", "codex",
            ])
            .output()
            .unwrap(),
    );
    for _ in 0..3 {
        good(
            p.human()
                .args(["--json", "team", "add", "engineer", "--agent", "codex"])
                .output()
                .unwrap(),
        );
    }
    let state: Value =
        serde_json::from_slice(&std::fs::read(p.root.join("host-state.json")).unwrap()).unwrap();
    let stack: Vec<_> = state["panes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["left_col"] == 49)
        .map(|p| p["size"]["rows"].as_u64().unwrap())
        .collect();
    assert_eq!(stack.len(), 5);
    assert!(stack.iter().max().unwrap() - stack.iter().min().unwrap() <= 1);
    good(
        p.human()
            .args(["--json", "team", "add", "engineer", "--agent", "codex"])
            .output()
            .unwrap(),
    );
    good(
        p.human()
            .args(["--json", "team", "add", "engineer", "--agent", "codex"])
            .output()
            .unwrap(),
    );
    let log = p.log();
    assert!(log.iter().any(|v| v["args"][1] == "adjust-pane-size"));
    assert!(
        log.iter()
            .any(|v| v["args"][1] == "set-tab-title" && v["args"][4] == "team 2")
    );
    assert!(log.iter().all(|v| v["tool"] == "wezterm"));
    let last = log
        .iter()
        .filter(|v| v["args"][1] == "split-pane")
        .last()
        .unwrap();
    assert!(
        last["args"]
            .as_array()
            .unwrap()
            .contains(&json!("--bottom"))
    );
}

#[test]
fn rm_refuses_reused_host_identity_and_foreign_agent() {
    let p = Project::new("member-replaced");
    p.board();
    p.start();
    let mut state: Value =
        serde_json::from_slice(&std::fs::read(p.root.join("host-state.json")).unwrap()).unwrap();
    state["agents"][1]["name"] = json!("someone-else");
    std::fs::write(
        p.root.join("host-state.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    let o = p
        .human()
        .args(["--yes", "team", "rm", "codex-engineer"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(6));
    assert!(!p.log().iter().any(|v| v["args"][1] == "close"));
    state["agents"][1]["name"] = json!("codex-engineer");
    state["panes"][1]["terminal_id"] = json!("different-terminal");
    std::fs::write(
        p.root.join("host-state.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    let o = p
        .human()
        .args(["--yes", "team", "rm", "codex-engineer"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(6));
    assert!(!p.log().iter().any(|v| v["args"][1] == "close"));
}

#[test]
fn stopped_team_can_remove_members_but_add_requires_restart() {
    let p = Project::new("member-stopped");
    p.board();
    p.start();
    good(
        p.cmd()
            .args(["--json", "--yes", "team", "down"])
            .output()
            .unwrap(),
    );
    let added = p
        .human()
        .args(["team", "add", "engineer", "--agent", "codex"])
        .output()
        .unwrap();
    assert_eq!(added.status.code(), Some(6));
    good(
        p.human()
            .args(["--json", "team", "rm", "engineer"])
            .output()
            .unwrap(),
    );
    assert_eq!(p.run(&["team", "ls"])["team"].as_array().unwrap().len(), 1);
}

#[test]
fn lead_cannot_choose_permission_mode_but_human_can() {
    let p = Project::new("member-permission-default");
    p.board();
    p.start();
    std::fs::write(p.config.join("config.toml"), "[team]\nlead_can_add=true\n").unwrap();
    let before = p.log().len();
    for mode in ["auto", "manual"] {
        let denied = p
            .cmd()
            .env("THC_ACTOR", "claude-pm")
            .args([
                "--json",
                "--yes",
                "team",
                "add",
                "reviewer",
                "--agent",
                "claude",
                "--permission-mode",
                mode,
            ])
            .output()
            .unwrap();
        assert_eq!(denied.status.code(), Some(6));
        assert!(String::from_utf8_lossy(&denied.stderr).contains("omit --permission-mode"));
        assert_eq!(
            p.log().len(),
            before,
            "agent permission choice refused before host access"
        );
    }
    good(
        p.human()
            .args([
                "--json",
                "team",
                "add",
                "reviewer",
                "--agent",
                "claude",
                "--permission-mode",
                "auto",
            ])
            .output()
            .unwrap(),
    );
    let log = p.log();
    let start = log
        .iter()
        .filter(|v| v["args"][1] == "start")
        .last()
        .unwrap();
    assert_eq!(
        &start["args"].as_array().unwrap()[7..],
        &[json!("--"), json!("--permission-mode"), json!("auto")]
    );
    good(
        p.cmd()
            .env("THC_ACTOR", "claude-pm")
            .args(["--json", "team", "add", "designer", "--agent", "claude"])
            .output()
            .unwrap(),
    );
    let log = p.log();
    let start = log
        .iter()
        .filter(|v| v["args"][1] == "start")
        .last()
        .unwrap();
    assert_eq!(
        start["args"].as_array().unwrap().len(),
        7,
        "lead inherits default without flags"
    );
}
