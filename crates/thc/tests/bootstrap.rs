//! Machine setup (docs/design/setup.md): `thc setup`, `--status`, `--undo`, and the first run of
//! thc. Every test runs with a scratch HOME, a login zsh reading that HOME, no app
//! (THC_SETUP_APP=none) and no login item (THC_SETUP_LOGIN=skip): nothing here touches launchd,
//! the real home or the real PATH.

mod common;

use serde_json::Value;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn home(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("thc-setup-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

fn thc(home: &Path) -> Command {
    let mut c = common::thc();
    c.current_dir("/")
        .env_clear()
        .env("HOME", home)
        .env("SHELL", "/bin/zsh")
        .env("PATH", "/usr/bin:/bin")
        .env("THC_SETUP_APP", "none")
        .env("THC_SETUP_LOGIN", "skip");
    c
}

fn json(home: &Path, args: &[&str]) -> Value {
    let o = thc(home).args(args).arg("--json").output().unwrap();
    assert!(o.status.success(), "thc {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).unwrap()
}

fn step<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["steps"].as_array().unwrap().iter().find(|s| s["step"] == name).unwrap()
}

/// What a new terminal window finds: `command -v thc` in a fresh login zsh with this HOME.
fn new_terminal_finds(home: &Path) -> String {
    let o = Command::new("/bin/zsh")
        .args(["-lic", "command -v thc"])
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

#[test]
fn setup_all_then_undo_leaves_only_the_vault() {
    let h = home("all");
    std::fs::create_dir_all(h.join(".claude")).unwrap();
    std::fs::create_dir_all(h.join(".codex")).unwrap();
    std::fs::write(h.join(".codex/AGENTS.md"), "# my own codex notes\n").unwrap();
    std::fs::write(h.join(".zprofile"), "export EDITOR=vim\n").unwrap();

    let v = json(&h, &["setup", "--all", "--yes"]);
    assert_eq!(step(&v, "vault")["state"], "done");
    assert_eq!(step(&v, "vault")["text"], "Your notes live in ~/thought");
    assert!(h.join("thought/thc-vault.toml").exists());
    assert!(std::fs::read_to_string(h.join(".config/thought/config.toml")).unwrap().contains("thought"));

    // The thc command: a link in ~/.local/bin, a marked line in ~/.zprofile, and a new terminal finds it.
    assert_eq!(step(&v, "path")["state"], "done", "{}", step(&v, "path"));
    assert_eq!(step(&v, "path")["detail"], "open a new terminal window to use it");
    let link = h.join(".local/bin/thc");
    assert_eq!(std::fs::read_link(&link).unwrap(), PathBuf::from(env!("CARGO_BIN_EXE_thc")).canonicalize().unwrap());
    let profile = std::fs::read_to_string(h.join(".zprofile")).unwrap();
    assert!(profile.starts_with("export EDITOR=vim\n# added by Thought Central"), "{profile}");
    assert_eq!(new_terminal_finds(&h), link.display().to_string());

    assert_eq!(step(&v, "login")["state"], "skipped");
    assert_eq!(step(&v, "agents")["text"], "Claude Code and Codex know thc");
    assert!(std::fs::read_to_string(h.join(".claude/skills/thc/SKILL.md")).unwrap().contains("name: thc"));
    let codex = std::fs::read_to_string(h.join(".codex/AGENTS.md")).unwrap();
    assert!(codex.starts_with("# my own codex notes\n") && codex.contains("<!-- thc:begin") && codex.contains("thc prime"), "{codex}");

    // Everything is recorded.
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(h.join(".config/thought/setup.json")).unwrap()).unwrap();
    let pieces: Vec<&str> = rec["items"].as_array().unwrap().iter().map(|i| i["piece"].as_str().unwrap()).collect();
    assert_eq!(pieces, ["vault", "link", "profile", "agent", "agent"]);

    // Again: nothing new, and the copy says so.
    let again = json(&h, &["setup", "--all", "--yes"]);
    assert_eq!(step(&again, "vault")["text"], "Using your vault in ~/thought");
    assert_eq!(step(&again, "path")["text"], "thc already works in your terminal");
    assert_eq!(step(&again, "agents")["text"], "Your agents already know thc");
    assert_eq!(std::fs::read_to_string(h.join(".zprofile")).unwrap().matches("added by Thought Central").count(), 1);

    let st = json(&h, &["setup", "--status"]);
    assert_eq!(st["agents"][0]["state"], "current");
    assert_eq!(st["thc_on_path"], link.display().to_string());

    // Undo removes exactly what setup added, and never the vault.
    let u = json(&h, &["setup", "--undo"]);
    assert_eq!(u["errors"].as_array().unwrap().len(), 0, "{u}");
    assert!(std::fs::symlink_metadata(&link).is_err());
    assert_eq!(std::fs::read_to_string(h.join(".zprofile")).unwrap(), "export EDITOR=vim\n");
    assert_eq!(std::fs::read_to_string(h.join(".codex/AGENTS.md")).unwrap(), "# my own codex notes\n");
    assert!(!h.join(".claude/skills/thc").exists());
    assert!(!h.join(".config/thought/setup.json").exists());
    assert!(h.join("thought/thc-vault.toml").exists(), "the vault stays");
    assert!(h.join(".config/thought/config.toml").exists(), "config.toml stays (a re-install finds the vault)");
}

#[test]
fn another_thc_on_path_is_never_shadowed() {
    let h = home("shadow");
    // A Homebrew- or cargo-style thc that a login shell already finds, older than this one.
    let bin = h.join("brew/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("thc"), "#!/bin/sh\necho thc 0.0.1\n").unwrap();
    std::process::Command::new("chmod").args(["+x", bin.join("thc").to_str().unwrap()]).status().unwrap();
    std::fs::write(h.join(".zprofile"), format!("export PATH=\"{}:$PATH\"\n", bin.display())).unwrap();

    let v = json(&h, &["setup", "--step", "path"]);
    assert_eq!(v["state"], "already");
    assert_eq!(v["text"], format!("thc already works in your terminal (~/brew/bin/thc 0.0.1) · older than Thought Central's {}", env!("CARGO_PKG_VERSION")));
    assert!(std::fs::symlink_metadata(h.join(".local/bin/thc")).is_err(), "no link was made");
    assert!(!std::fs::read_to_string(h.join(".zprofile")).unwrap().contains("Thought Central"), "no PATH line either");
}

#[test]
fn a_real_file_in_local_bin_is_left_alone() {
    let h = home("occupied");
    std::fs::create_dir_all(h.join(".local/bin")).unwrap();
    std::fs::write(h.join(".local/bin/thc"), "not ours").unwrap();
    let o = thc(&h).args(["setup", "--step", "path", "--json"]).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["state"], "failed");
    assert_eq!(v["text"], "Couldn't add the thc command");
    assert!(v["error"].as_str().unwrap().contains("is another program"), "{v}");
    assert_eq!(std::fs::read_to_string(h.join(".local/bin/thc")).unwrap(), "not ours");
}

#[test]
fn bash_keeps_an_existing_profile() {
    let h = home("bash");
    std::fs::write(h.join(".profile"), "# mine\n").unwrap();
    let o = thc(&h).env("SHELL", "/bin/bash").args(["setup", "--step", "path", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_ne!(v["state"], "failed", "{v}");
    // bash reads .profile when there's no .bash_profile: writing one would hide it.
    assert!(!h.join(".bash_profile").exists());
    assert!(std::fs::read_to_string(h.join(".profile")).unwrap().contains("added by Thought Central"));
}

#[test]
fn an_existing_vault_is_used_not_replaced() {
    let h = home("existing");
    let o = thc(&h).current_dir(&h).args(["init", "Dropbox/thought", "--global"]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v = json(&h, &["setup", "--step", "vault"]);
    assert_eq!(v["state"], "already");
    assert_eq!(v["text"], "Using your vault in ~/Dropbox/thought");
    assert!(!h.join("thought").exists(), "no second vault");
}

#[test]
fn undo_one_agent_keeps_the_rest() {
    let h = home("undo-one");
    std::fs::create_dir_all(h.join(".claude")).unwrap();
    std::fs::create_dir_all(h.join(".codex")).unwrap();
    json(&h, &["setup", "--all", "--yes"]);
    let u = json(&h, &["setup", "claude", "--user", "--undo"]);
    assert_eq!(u["removed"].as_array().unwrap().len(), 1, "{u}");
    assert!(!h.join(".claude/skills/thc/SKILL.md").exists());
    assert!(h.join(".codex/AGENTS.md").exists(), "codex untouched");
    assert!(std::fs::symlink_metadata(h.join(".local/bin/thc")).is_ok(), "the link untouched");
    let rec = std::fs::read_to_string(h.join(".config/thought/setup.json")).unwrap();
    assert!(!rec.contains("\"claude\"") && rec.contains("\"codex\""), "{rec}");
}

#[test]
fn a_recorded_outside_link_is_undone() {
    let h = home("record");
    let sys = h.join("sysbin");
    std::fs::create_dir_all(&sys).unwrap();
    let link = sys.join("thc");
    std::os::unix::fs::symlink(PathBuf::from(env!("CARGO_BIN_EXE_thc")).canonicalize().unwrap(), &link).unwrap();
    json(&h, &["setup", "--record-link", link.to_str().unwrap()]);
    // Something else entirely is refused.
    std::os::unix::fs::symlink("/bin/ls", sys.join("ls")).unwrap();
    assert_eq!(thc(&h).args(["setup", "--record-link", sys.join("ls").to_str().unwrap()]).output().unwrap().status.code(), Some(6));
    let u = json(&h, &["setup", "--undo"]);
    assert!(u["removed"][0].as_str().unwrap().contains("removed the thc link"), "{u}");
    assert!(std::fs::symlink_metadata(&link).is_err());
}

#[test]
fn user_scope_names_the_agent() {
    let h = home("user");
    // Named explicitly, an agent is set up even before its folder exists.
    let v = json(&h, &["setup", "codex", "--user"]);
    assert_eq!(v["text"], "Codex knows thc");
    assert!(h.join(".codex/AGENTS.md").exists());
    let o = thc(&h).args(["setup", "claude", "--user", "--check"]).output().unwrap();
    assert_eq!(o.status.code(), Some(1), "missing skill is stale");
    assert!(thc(&h).args(["setup", "codex", "--user", "--check"]).output().unwrap().status.success());
}

#[test]
fn no_first_run_for_pipes_agents_ci_or_json() {
    let h = home("gated");
    for extra in [vec![], vec![("THC_ACTOR", "claude")], vec![("CI", "1")], vec![("THC_NO_SETUP", "1")]] {
        let mut c = thc(&h);
        for (k, v) in &extra {
            c.env(k, v);
        }
        let o = c.arg("today").output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{extra:?}");
        assert!(String::from_utf8_lossy(&o.stderr).contains("no vault found"));
    }
    assert!(!h.join("thought").exists(), "nothing was set up");
}

/// Run thc in a pseudo-terminal (a person at a terminal), answering the one question.
fn in_terminal(home: &Path, args: &[&str], answer: &str) -> String {
    let (mut master, slave) = unsafe {
        let (mut m, mut s) = (0, 0);
        assert_eq!(libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()), 0);
        (std::fs::File::from(OwnedFd::from_raw_fd(m)), OwnedFd::from_raw_fd(s))
    };
    let mut child = thc(home)
        .args(args)
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave))
        .spawn()
        .unwrap();
    // Read until the question, answer it, then read to the end.
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut answered = false;
    let mut buf = [0u8; 4096];
    loop {
        assert!(Instant::now() < deadline, "timed out; saw: {}", String::from_utf8_lossy(&seen));
        match master.read(&mut buf) {
            Ok(0) | Err(_) => break, // the child closed the terminal
            Ok(n) => seen.extend_from_slice(&buf[..n]),
        }
        if !answered && String::from_utf8_lossy(&seen).contains("[Y/n]") {
            master.write_all(answer.as_bytes()).unwrap();
            answered = true;
        }
        if let Ok(Some(_)) = child.try_wait() {
            // Drain what's left without blocking forever.
            unsafe { libc::fcntl(std::os::fd::AsRawFd::as_raw_fd(&master), libc::F_SETFL, libc::O_NONBLOCK) };
            while let Ok(n) = master.read(&mut buf) {
                if n == 0 {
                    break;
                }
                seen.extend_from_slice(&buf[..n]);
            }
            break;
        }
    }
    child.wait().unwrap();
    String::from_utf8_lossy(&seen).to_string()
}

#[test]
fn first_run_in_a_terminal_sets_up_once_and_asks_once() {
    let h = home("first");
    std::fs::create_dir_all(h.join(".claude")).unwrap();
    let out = in_terminal(&h, &["today"], "\n");
    assert!(out.contains("Teach Claude Code to use thc? [Y/n]"), "{out}");
    assert!(out.contains("set up: vault ~/thought · daemon not started · skill installed for claude · undo: thc setup --undo"), "{out}");
    assert!(out.contains("Today"), "then the command runs: {out}");
    assert!(h.join("thought/thc-vault.toml").exists());
    assert!(h.join(".claude/skills/thc/SKILL.md").exists());
    // Once: the next command just runs.
    let again = in_terminal(&h, &["today"], "\n");
    assert!(!again.contains("set up:") && !again.contains("[Y/n]"), "{again}");
}

#[test]
fn first_run_no_means_no_skill() {
    let h = home("first-no");
    std::fs::create_dir_all(h.join(".claude")).unwrap();
    let out = in_terminal(&h, &["today"], "n\n");
    assert!(out.contains("set up: vault ~/thought · daemon not started · undo"), "{out}");
    assert!(!h.join(".claude/skills").exists());
}

/// `thc setup wezterm --yes` (0.9.43): writes thc's module and adds one marked block to the
/// config WezTerm loads ($WEZTERM_CONFIG_FILE, ~/.config/wezterm/wezterm.lua, ~/.wezterm.lua),
/// backed up first and never twice; `--undo --yes` takes both out, leaving the file as it was.
#[test]
fn setup_wezterm_adds_one_block_and_undo_removes_it() {
    let original = "local wezterm = require 'wezterm'\nlocal config = wezterm.config_builder()\nconfig.font_size = 14\nreturn config\n";
    for (case, rel, env_file) in [("xdg", ".config/wezterm/wezterm.lua", false), ("dot", ".wezterm.lua", false), ("env", "custom/wez.lua", true)] {
        let home = std::env::temp_dir().join(format!("thc-wezterm-{case}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let file = home.join(rel);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, original).unwrap();
        let run = |args: &[&str]| {
            let mut c = common::thc();
            c.args(args).env("HOME", &home).env_remove("XDG_CONFIG_HOME").env_remove("WEZTERM_CONFIG_FILE");
            if env_file {
                c.env("WEZTERM_CONFIG_FILE", &file);
            }
            c.output().unwrap()
        };
        // Without --yes: nothing written.
        assert!(run(&["setup", "wezterm"]).status.success());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), original, "{case}");
        // --yes twice: one block, before `return config`, and the module.
        for _ in 0..2 {
            let o = run(&["--yes", "setup", "wezterm"]);
            assert!(o.status.success(), "{case}: {}", String::from_utf8_lossy(&o.stderr));
        }
        let now = std::fs::read_to_string(&file).unwrap();
        assert_eq!(now.matches("-- thc: begin").count(), 1, "{case}: {now}");
        assert!(now.find("require('thc_keys').apply(config)").unwrap() < now.rfind("return config").unwrap(), "{case}: {now}");
        assert!(now.contains("config.font_size = 14"), "{case}: the rest is untouched");
        assert!(home.join(".config/wezterm/thc_keys.lua").exists());
        let backups = std::fs::read_dir(file.parent().unwrap()).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().contains(".bak-")).count();
        assert!(backups >= 1, "{case}: backed up");
        // --undo --yes: the file as it was, the module gone.
        assert!(run(&["--yes", "setup", "wezterm", "--undo"]).status.success());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), original, "{case}");
        assert!(!home.join(".config/wezterm/thc_keys.lua").exists());
        let _ = std::fs::remove_dir_all(&home);
    }
    // No `return config` to put it before: the block is printed, the file untouched.
    let home = std::env::temp_dir().join(format!("thc-wezterm-noreturn-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join(".wezterm.lua"), "return { font_size = 14 }\n").unwrap();
    let o = common::thc().args(["--yes", "setup", "wezterm"]).env("HOME", &home).env_remove("XDG_CONFIG_HOME").env_remove("WEZTERM_CONFIG_FILE").output().unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).contains("add this yourself"), "{}", String::from_utf8_lossy(&o.stdout));
    assert_eq!(std::fs::read_to_string(home.join(".wezterm.lua")).unwrap(), "return { font_size = 14 }\n");
    let _ = std::fs::remove_dir_all(&home);
}

/// thc_keys.lua scopes every key to thc (0.9.43): in thc a key sends thc's sequence; in zsh, an
/// editor or ssh it passes through untouched. Checked in WezTerm's own Lua (a config that
/// asserts at load time), where WezTerm is installed; skipped elsewhere (CI).
#[test]
fn wezterm_keys_pass_through_outside_thc() {
    let Ok(o) = std::process::Command::new("wezterm").arg("--version").output() else { return };
    if !o.status.success() {
        return;
    }
    let home = std::env::temp_dir().join(format!("thc-wezkeys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let mut c = common::thc();
    assert!(c.args(["--yes", "setup", "wezterm"]).env("HOME", &home).env_remove("XDG_CONFIG_HOME").env_remove("WEZTERM_CONFIG_FILE").status().unwrap().success());
    let module = home.join(".config/wezterm/thc_keys.lua");
    let cfg = home.join("check.lua");
    std::fs::write(
        &cfg,
        format!(
            r#"local wezterm = require 'wezterm'
local M = dofile('{}')
local function check(ok, what) if not ok then error('thc_keys: ' .. what) end end
for _, k in ipairs({{ {{'LeftArrow','CMD'}}, {{'LeftArrow','OPT'}}, {{'Backspace','CMD'}}, {{'Backspace','OPT'}}, {{'v','CMD'}}, {{'UpArrow','CMD|SHIFT'}} }}) do
  check(M.decide(k[1], k[2], '/bin/zsh').pass, k[1] .. ' passes through in zsh')
  check(M.decide(k[1], k[2], '/usr/bin/ssh').pass, k[1] .. ' passes through in ssh')
  check(M.decide(k[1], k[2], '/opt/homebrew/bin/micro').pass, k[1] .. ' passes through in micro')
  check(M.decide(k[1], k[2], '/Users/x/.local/bin/thc').send, k[1] .. ' is thc\'s in thc')
end
check(M.decide('LeftArrow', 'OPT', '/Users/x/.local/bin/thc').send == '\x1b[1;3D', 'word left in thc')
check(M.decide('v', 'CMD', 'thc').send == '\x1b[118;9u', 'cmd-v in thc')
check(M.decide('c', 'CMD', 'thc').send == '\x1b[99;9u', 'cmd-c in thc')
check(M.decide('x', 'CMD', 'thc').send == '\x1b[120;9u', 'cmd-x in thc')
check(M.decide('z', 'CMD|SHIFT', 'thc').send == '\x1b[122;10u', 'shift-cmd-z in thc')
check(M.decide('a', 'CMD', '/bin/zsh').pass, 'cmd-a passes through in zsh')
check(M.decide('LeftArrow', 'OPT', '/Users/x/bin/thcx').pass, 'only thc itself')
local config = wezterm.config_builder()
M.apply(config)
return config
"#,
            module.display()
        ),
    )
    .unwrap();
    let o = std::process::Command::new("wezterm").args(["--config-file", cfg.to_str().unwrap(), "show-keys"]).output().unwrap();
    let all = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(!all.contains("thc_keys:") && !all.to_lowercase().contains("error"), "{all}");
    let _ = std::fs::remove_dir_all(&home);
}

/// A newer thc keeps its own WezTerm module current (⌘[ ⌘] came after 0.9.43): opening the TUI
/// rewrites an older thc_keys.lua, never creates one that was never set up, and never touches
/// the user's wezterm.lua.
#[test]
fn the_tui_refreshes_an_older_wezterm_module() {
    let home = std::env::temp_dir().join(format!("thc-wezterm-refresh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join("p")).unwrap();
    let tui = || {
        let mut c = common::thc();
        c.args(["tui"]).current_dir(home.join("p")).env("HOME", &home).env("THC_VAULT", home.join("p/vault")).env("THC_CACHE_DIR", home.join("cache")).env("THC_TUI_SNAPSHOT", "100x24").env("THC_TUI_KEYS", "");
        let o = c.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    };
    let mut init = common::thc();
    assert!(init.args(["init", "vault"]).current_dir(home.join("p")).env("HOME", &home).env("THC_VAULT", home.join("p/vault")).output().unwrap().status.success());
    let module = home.join(".config/wezterm/thc_keys.lua");
    tui();
    assert!(!module.exists(), "never set up: nothing written");
    std::fs::create_dir_all(module.parent().unwrap()).unwrap();
    std::fs::write(&module, "-- thc_keys.lua: an older one\nreturn {}\n").unwrap();
    std::fs::write(home.join(".config/wezterm/wezterm.lua"), "-- mine\n").unwrap();
    tui();
    let now = std::fs::read_to_string(&module).unwrap();
    assert!(now.contains("'\\x1b[91;9u'"), "rewritten with ⌘[: {now}");
    assert_eq!(std::fs::read_to_string(home.join(".config/wezterm/wezterm.lua")).unwrap(), "-- mine\n");
    let _ = std::fs::remove_dir_all(&home);
}
