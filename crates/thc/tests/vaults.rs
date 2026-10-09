//! Vaults V1 (vaults.md §1–2, §4): identity, the registry, resolution by name, `thc vault`.

mod common;

use serde_json::Value;
use std::path::{Path, PathBuf};

struct H {
    home: PathBuf,
}

impl H {
    fn new(name: &str) -> H {
        let home = thc_core::scratch::dir(&format!("thc-vaults-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        H { home }
    }

    fn cmd(&self, args: &[&str]) -> std::process::Output {
        let mut c = common::thc();
        c.args(args).current_dir(&self.home).env("HOME", &self.home).env_remove("THC_VAULT").env_remove("THC_CACHE_DIR").env_remove("THC_CONFIG_DIR").env("THC_NOW", "2026-10-03T09:00");
        c.output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let o = self.cmd(args);
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        serde_json::from_str(&self.ok(&a)).unwrap()
    }

    fn config(&self) -> String {
        std::fs::read_to_string(self.home.join(".config/thought/config.toml")).unwrap_or_default()
    }
}

fn marker(p: &Path) -> String {
    std::fs::read_to_string(p.join("thc-vault.toml")).unwrap()
}

#[test]
fn v1_home_new_use_and_names() {
    let h = H::new("v1");
    h.ok(&["init", "thought", "--global"]);
    assert!(h.config().starts_with("home = \"personal\""), "{}", h.config());
    let m = marker(&h.home.join("thought"));
    assert!(m.contains("name = \"personal\"") && m.contains("id = "), "{m}");
    h.ok(&["add", "a personal note"]);
    // A new vault goes in ~/thought-vaults/<name> and is registered.
    let v = h.json(&["vault", "new", "acme"]);
    assert_eq!(v["path"].as_str().unwrap(), h.home.join("thought-vaults/acme").to_str().unwrap());
    let ls = h.json(&["vault", "ls"]);
    let names: Vec<&str> = ls["vaults"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["personal", "acme"]);
    assert_eq!(ls["vaults"][0]["current"], true);
    assert_eq!(ls["vaults"][0]["open"], 0);
    // --vault takes a name; JSON names the vault on nodes and listings.
    h.ok(&["--vault", "acme", "add", "an acme note"]);
    let q = h.json(&["--vault", "acme", "q", "text:note"]);
    assert_eq!(q["items"][0]["text"], "an acme note");
    assert_eq!(q["items"][0]["vault"], "acme");
    assert_eq!(q["vault"]["name"], "acme");
    // `use` makes it current; human listings then say so on their first line.
    h.ok(&["vault", "use", "acme"]);
    assert!(h.ok(&["vault"]).starts_with("acme (from thc vault use)"));
    let first = h.ok(&["today"]).lines().next().unwrap_or_default().to_string();
    assert!(first.starts_with("vault acme · 0 open · thc vault use personal"), "{first}");
    // Back home: quiet again.
    h.ok(&["vault", "use", "personal"]);
    assert!(!h.ok(&["today"]).starts_with("vault "));
    assert!(!h.config().contains("current ="), "{}", h.config());
    // A .thc.toml can name a vault.
    std::fs::create_dir_all(h.home.join("code/acme")).unwrap();
    std::fs::write(h.home.join("code/acme/.thc.toml"), "vault = \"acme\"\n").unwrap();
    let mut c = common::thc();
    let o = c.args(["--json", "vault"]).current_dir(h.home.join("code/acme")).env("HOME", &h.home).env_remove("THC_VAULT").env_remove("THC_CACHE_DIR").env_remove("THC_CONFIG_DIR").output().unwrap();
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["name"], "acme");
    assert_eq!(v["source"], "project");
    // `acme/<id>` works from anywhere: the command runs in acme. With --vault set to another
    // vault, it points there instead.
    let id = q["items"][0]["id"].as_str().unwrap().to_string();
    h.ok(&["--vault", "acme", "show", &format!("acme/{id}")]);
    assert_eq!(h.json(&["show", &format!("acme/{id}")])["text"], "an acme note");
    let o = h.cmd(&["--vault", "personal", "show", &format!("acme/{id}")]);
    assert_eq!(o.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&o.stderr).contains("--vault acme"));
    let _ = std::fs::remove_dir_all(&h.home);
}

#[test]
fn v1_refusals_rename_and_rm() {
    let h = H::new("v1r");
    h.ok(&["init", "thought", "--global"]);
    // Vaults never nest; names are checked; typos get a suggestion.
    let o = h.cmd(&["vault", "new", "inner", h.home.join("thought/inner").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(6), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(h.cmd(&["vault", "new", "Bad Name"]).status.code(), Some(6));
    h.ok(&["vault", "new", "side"]);
    let o = h.cmd(&["vault", "use", "sid"]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("did you mean side?"));
    // Rename changes the synced name too.
    h.ok(&["vault", "rename", "side", "sidequest"]);
    assert!(marker(&h.home.join("thought-vaults/side")).contains("name = \"sidequest\""));
    // rm unregisters and deletes nothing; home can't be removed.
    h.ok(&["vault", "rm", "sidequest"]);
    assert!(h.home.join("thought-vaults/side/thc-vault.toml").exists());
    assert_eq!(h.cmd(&["vault", "rm", "personal"]).status.code(), Some(6));
    // add brings it back, under its synced name.
    let v = h.json(&["vault", "add", h.home.join("thought-vaults/side").to_str().unwrap()]);
    assert_eq!(v["name"], "sidequest");
    let _ = std::fs::remove_dir_all(&h.home);
}

#[test]
fn v1_a_pre_vaults_config_still_works_and_migrates_on_write() {
    let h = H::new("v1m");
    h.ok(&["init", "thought", "--global"]);
    // Rewrite the config the way 0.9.22 wrote it.
    let v = h.home.join("thought");
    std::fs::write(h.home.join(".config/thought/config.toml"), format!("vault = {:?}\n\n[tui]\njournal = \"focus\"\n", v.to_str().unwrap())).unwrap();
    h.ok(&["add", "still works"]);
    let me = h.json(&["vault"]);
    assert_eq!(me["name"], "personal");
    h.ok(&["vault", "new", "acme"]);
    let cfg = h.config();
    assert!(cfg.starts_with("home = \"personal\"") && !cfg.contains("\nvault = ") && cfg.contains("[tui]\njournal = \"focus\""), "{cfg}");
    let _ = std::fs::remove_dir_all(&h.home);
}

#[test]
fn v2_settings_layer_and_vaults_wear_their_colour() {
    let h = H::new("v2");
    h.ok(&["init", "thought", "--global"]);
    // New vaults get the next unused accent.
    assert_eq!(h.json(&["vault", "new", "acme"])["accent"], "rose");
    assert_eq!(h.json(&["vault", "new", "side"])["accent"], "sea");
    let acme = h.home.join("thought-vaults/acme");
    // Your config, the vault's settings (a diff, whitelisted), your per-vault override.
    std::fs::write(h.home.join(".config/thought/config.toml"), format!("{}\n[tui]\nleader_popup = \"immediate\"\n\n[tui.focus]\nwidth = 70\n\n[vaults.acme.settings.tui]\nwhich_key_ms = 500\n", h.config())).unwrap();
    std::fs::write(acme.join("settings.toml"), "[theme]\naccent = \"rose\"\n\n[tui.focus]\npreset = \"planner\"\n\n[tui]\nwhich_key_ms = 100\n\n[hooks]\non_add = \"echo hi\"\n").unwrap();
    let e = h.json(&["--vault", "acme", "config", "--effective"]);
    let get = |k: &str| e["settings"].as_array().unwrap().iter().find(|s| s["key"] == k).cloned().unwrap_or_default();
    assert_eq!(get("tui.focus.preset")["value"], "planner");
    assert_eq!(get("tui.focus.preset")["source"], "vault");
    assert_eq!(get("tui.focus.width")["value"], 70, "tables merge: your width stays");
    assert_eq!(get("tui.which_key_ms")["value"], 500, "your override beats the vault");
    assert_eq!(get("tui.which_key_ms")["source"], "override");
    assert!(get("hooks.on_add").is_null(), "a vault never sets hooks");
    assert!(e["notices"][0].as_str().unwrap().contains("hooks aren't allowed in a vault"), "{e}");
    let why = h.json(&["--vault", "acme", "config", "--why", "tui.which_key_ms"]);
    let srcs: Vec<&str> = why["layers"].as_array().unwrap().iter().map(|l| l["source"].as_str().unwrap()).collect();
    assert_eq!(srcs, ["vault", "override"]);
    // doctor lists the same notice.
    assert!(h.ok(&["--vault", "acme", "doctor"]).contains("hooks aren't allowed"));
    // The TUI's header names the vault, in its accent (rose: #E8769F).
    let mut c = common::thc();
    let o = c
        .args(["--vault", "acme", "tui"])
        .current_dir(&h.home)
        .env("HOME", &h.home)
        .env_remove("THC_VAULT")
        .env_remove("THC_CACHE_DIR")
        .env_remove("THC_CONFIG_DIR")
        .env("THC_TUI_SNAPSHOT", "120x30")
        .env("THC_TUI_SNAPSHOT_FORMAT", "ansi")
        .env("THC_THEME", "ember-dark")
        .output()
        .unwrap();
    let head = String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or_default().to_string();
    assert!(head.contains("232;118;159") && head.contains("acme"), "{head:?}");
    // Home stays ember and says its name.
    let o = common::thc().args(["tui"]).current_dir(&h.home).env("HOME", &h.home).env_remove("THC_VAULT").env_remove("THC_CACHE_DIR").env_remove("THC_CONFIG_DIR").env("THC_TUI_SNAPSHOT", "120x30").output().unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or_default().contains("personal"));
    let _ = std::fs::remove_dir_all(&h.home);
}

impl H {
    fn tui(&self, args: &[&str], size: &str, keys: &str) -> String {
        let mut c = common::thc();
        let o = c
            .args(args)
            .current_dir(&self.home)
            .env("HOME", &self.home)
            .env_remove("THC_VAULT")
            .env_remove("THC_CACHE_DIR")
            .env_remove("THC_CONFIG_DIR")
            .env("THC_TUI_SNAPSHOT", size)
            .env("THC_TUI_KEYS", keys)
            // A colour theme: in 16 colours the name is a chip (`[•]  acme `), spaced apart.
            .env("THC_THEME", "ember-dark")
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }
}

#[test]
fn v3_the_picker_switches_and_names_fit() {
    let h = H::new("v3");
    h.ok(&["init", "thought", "--global"]);
    h.ok(&["vault", "new", "acme"]);
    h.ok(&["vault", "new", "thought-central-research-lab"]);
    h.ok(&["--vault", "acme", "todo", "the acme task", "--due", "today"]);
    // V lists the vaults: the current one marked, home last.
    let f = h.tui(&["tui"], "100x24", "V");
    let rows: Vec<&str> = f.lines().filter(|l| l.contains("local only")).collect();
    assert_eq!(rows.len(), 3, "{f}");
    assert!(rows[0].contains("acme") && rows[2].contains("* personal") && rows[2].contains("home"), "{f}");
    // Enter on acme: this session is in acme now (its rows, its name), saved as it went.
    let f = h.tui(&["tui"], "100x24", "V<up><up><cr>");
    assert!(f.lines().next().unwrap().contains("[•] acme"), "{f}");
    assert!(f.contains("the acme task") && f.contains("now in acme"), "{f}");
    // With the mouse: one click on acme's row switches, and `+ new vault`
    // asks for a name.
    let f = h.tui(&["tui"], "100x24", "V");
    let (y, line) = f.lines().enumerate().find(|(_, l)| l.contains("acme") && l.contains("local only")).unwrap();
    let x = line[..line.find("acme").unwrap()].chars().count() + 1;
    let f = h.tui(&["tui"], "100x24", &format!("V<click:{x},{y}>"));
    assert!(f.lines().next().unwrap().contains("[•] acme"), "{f}");
    let f = h.tui(&["tui"], "100x24", "V");
    let (y, line) = f.lines().enumerate().find(|(_, l)| l.contains("+ new vault")).unwrap();
    let x = line[..line.find("+ new vault").unwrap()].chars().count() + 2;
    let f = h.tui(&["tui"], "100x24", &format!("V<click:{x},{y}>"));
    assert!(f.contains("new vault ›") || f.contains("new vault >"), "{f}");
    // The same from the palette.
    let f = h.tui(&["tui"], "100x24", ":vault: acme<cr>");
    assert!(f.lines().next().unwrap().contains("[•] acme"), "{f}");
    // Outside home the crumb leads with the vault.
    let f = h.tui(&["--vault", "acme", "j", "--no-focus"], "100x24", "");
    assert!(f.contains("acme › § Journal ›"), "{f}");
    // A long name: whole when it fits, else the vault's short name, else middle-truncated.
    let f = h.tui(&["--vault", "thought-central-research-lab", "tui"], "80x24", "");
    assert!(f.lines().next().unwrap().contains("[•] thought…-lab "), "{f}");
    std::fs::write(h.home.join("thought-vaults/thought-central-research-lab/settings.toml"), "[vault]\nname_short = \"tcrl\"\n").unwrap();
    let f = h.tui(&["--vault", "thought-central-research-lab", "tui"], "80x24", "");
    assert!(f.lines().next().unwrap().contains("[•] tcrl "), "{f}");
    let _ = std::fs::remove_dir_all(&h.home);
}

#[test]
fn v4_queries_across_vaults() {
    let h = H::new("v4");
    h.ok(&["init", "thought", "--global"]);
    h.ok(&["vault", "new", "acme"]);
    h.ok(&["vault", "new", "side"]);
    h.ok(&["todo", "home task", "--due", "2026-10-05"]);
    h.ok(&["--vault", "acme", "todo", "acme task", "--due", "2026-10-04"]);
    h.ok(&["--vault", "side", "todo", "side task", "--due", "2026-10-06"]);
    // Every vault, in the query's own order (due), each row naming its vault.
    let v = h.json(&["q", "vault:* status:open sort:due"]);
    let rows: Vec<(String, String)> = v["items"].as_array().unwrap().iter().map(|n| (n["text"].as_str().unwrap().to_string(), n["vault"].as_str().unwrap().to_string())).collect();
    assert_eq!(rows, [("acme task".into(), "acme".into()), ("home task".into(), "personal".into()), ("side task".into(), "side".into())]);
    assert_eq!(v["vault"]["names"], serde_json::json!(["personal", "acme", "side"]));
    // Human rows say which vault, when more than one answered.
    let out = h.ok(&["q", "vault:(acme or side) status:open sort:due"]);
    assert!(out.starts_with("vault acme, side (2 of 3 registered)"), "{out}");
    assert!(out.contains("acme task  acme · due") && out.contains("side task  side · due"), "{out}");
    // Leaving one out; an unknown name; group:vault.
    let v = h.json(&["q", "vault:* -vault:side status:open"]);
    assert_eq!(v["count"], 2);
    let o = h.cmd(&["q", "vault:acm status:open"]);
    assert_eq!(o.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&o.stderr).contains("did you mean acme?"));
    let g = h.json(&["q", "vault:* status:open group:vault"]);
    let keys: Vec<&str> = g["groups"].as_array().unwrap().iter().map(|x| x["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["personal", "acme", "side"]);
    // A vault this device doesn't have: the rest still answer, and it's named.
    std::fs::rename(h.home.join("thought-vaults/side"), h.home.join("thought-vaults/side-gone")).unwrap();
    let v = h.json(&["q", "vault:* status:open"]);
    assert_eq!(v["count"], 2);
    assert_eq!(v["vault"]["missing"], serde_json::json!(["side"]));
    let _ = std::fs::remove_dir_all(&h.home);
}

#[test]
fn v5_today_and_agenda_are_about_you_across_vaults() {
    let h = H::new("v5");
    h.ok(&["init", "thought", "--global"]);
    h.ok(&["todo", "home task", "--due", "today"]);
    // One vault: Today as it always was (no vault names on rows).
    let one = h.ok(&["today"]);
    assert!(one.contains("home task") && !one.contains("personal ·"), "{one}");
    h.ok(&["vault", "new", "acme"]);
    h.ok(&["--vault", "acme", "todo", "acme task", "--due", "today"]);
    // A person's Today reads every vault; rows name theirs.
    let v = h.json(&["today"]);
    let rows: Vec<(String, String)> = v["today"].as_array().unwrap().iter().map(|n| (n["text"].as_str().unwrap().to_string(), n["vault"].as_str().unwrap().to_string())).collect();
    assert!(rows.contains(&("home task".into(), "personal".into())) && rows.contains(&("acme task".into(), "acme".into())), "{rows:?}");
    assert_eq!(v["vaults"], serde_json::json!(["personal", "acme"]));
    let human = h.ok(&["today"]);
    assert!(human.contains("2 vaults · personal, acme") && human.contains("acme task  acme · "), "{human}");
    let ag = h.json(&["agenda", "--days", "2"]);
    let first: Vec<&str> = ag["days"][0]["items"].as_array().unwrap().iter().map(|n| n["vault"].as_str().unwrap()).collect();
    assert!(first.contains(&"acme") && first.contains(&"personal"), "{ag}");
    // An agent reads its own vault only (vaults.md §3.2).
    let mut c = common::thc();
    let o = c.args(["--json", "today"]).current_dir(&h.home).env("HOME", &h.home).env_remove("THC_VAULT").env_remove("THC_CACHE_DIR").env_remove("THC_CONFIG_DIR").env("THC_NOW", "2026-10-03T09:00").env("THC_ACTOR", "claude").output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let texts: Vec<&str> = v["today"].as_array().unwrap().iter().map(|n| n["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["home task"]);
    let _ = std::fs::remove_dir_all(&h.home);
}

impl H {
    fn tui_env(&self, args: &[&str], keys: &str, extra: &[(&str, &str)]) -> String {
        let mut c = common::thc();
        c.args(args)
            .current_dir(&self.home)
            .env("HOME", &self.home)
            .env_remove("THC_VAULT")
            .env_remove("THC_CACHE_DIR")
            .env_remove("THC_CONFIG_DIR")
            .env("THC_NOW", "")
            .env("THC_THEME", "ember-dark")
            .env("THC_TUI_SNAPSHOT", "110x24")
            .env("THC_TUI_KEYS", keys);
        for (k, v) in extra {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into()
    }
}

#[test]
fn v5_the_tui_today_spans_vaults_and_routes_writes() {
    let h = H::new("v5t");
    h.ok(&["init", "thought", "--global"]);
    h.ok(&["vault", "new", "acme"]);
    let mut c = common::thc();
    // Due today (unpinned clock: the TUI's toasts aren't hidden by the pinned-clock warning).
    for (vault, text) in [("personal", "home task"), ("acme", "Book the venue")] {
        let o = c.args(["--vault", vault, "todo", text, "--due", "today"]).current_dir(&h.home).env("HOME", &h.home).env_remove("THC_VAULT").env_remove("THC_CACHE_DIR").env_remove("THC_CONFIG_DIR").env("THC_NOW", "").output().unwrap();
        assert!(o.status.success());
        c = common::thc();
    }
    // today-cross-vault: both vaults' rows; acme's names acme; the bar says 2 vaults.
    let f = h.tui_env(&["tui"], "", &[]);
    assert!(f.lines().next().unwrap().contains("[•] personal"), "{f}");
    let venue = f.lines().find(|l| l.contains("Book the venue")).expect("acme's row");
    // Every row names its vault, here included (view-explain.md §1 retired "no name means here").
    assert!(venue.trim_end().ends_with("acme"), "{f}");
    let home = f.lines().find(|l| l.contains("home task")).unwrap();
    assert!(home.trim_end().ends_with("personal"), "{f}");
    assert!(f.lines().last().unwrap().contains("today · 2 vaults"), "{f}");
    // today-row-other-vault-done: done goes to acme, and says so.
    let row = f.lines().filter(|l| l.contains("[ ]")).position(|l| l.contains("Book the venue")).unwrap();
    let keys = format!("{}x", "j".repeat(row));
    let f = h.tui_env(&["tui"], &keys, &[("THC_TUI_SNAPSHOT_WRITE", "1")]);
    assert!(f.lines().last().unwrap().contains("Book the venue · acme · u undo"), "{f}");
    let acme = h.json(&["--vault", "acme", "q", "text:venue"]);
    assert_eq!(acme["items"][0]["status"], "done", "written in acme");
    h.ok(&["--vault", "acme", "reopen", acme["items"][0]["id"].as_str().unwrap()]);
    // today-open-other-vault: Enter moves into acme; Esc comes back to personal.
    let f = h.tui_env(&["tui"], &format!("{}<cr>", "j".repeat(row)), &[]);
    assert!(f.lines().next().unwrap().contains("[•] acme") && f.contains("acme › "), "{f}");
    assert!(f.lines().last().unwrap().contains("Esc returns to personal"), "{f}");
    let f = h.tui_env(&["tui"], &format!("{}<cr><esc>", "j".repeat(row)), &[]);
    assert!(f.lines().next().unwrap().contains("[•] personal"), "{f}");
    // today-capture-target: the drawer names the vault.
    let f = h.tui_env(&["tui"], "a", &[]);
    assert!(f.contains("capture → personal · § today"), "{f}");
    // space t v: a section per vault.
    let f = h.tui_env(&["tui"], "<space>tv", &[]);
    assert!(f.lines().any(|l| l.trim_start().starts_with("personal")) && f.lines().any(|l| l.trim_start().starts_with("acme")), "{f}");
    let _ = std::fs::remove_dir_all(&h.home);
}

#[test]
fn v6_one_daemon_serves_every_vault() {
    let h = H::new("v6");
    h.ok(&["init", "thought", "--global"]);
    h.ok(&["vault", "new", "acme"]);
    let mut c = common::thc();
    c.args(["daemon", "run"])
        .current_dir(&h.home)
        .env("HOME", &h.home)
        .env_remove("THC_VAULT")
        .env_remove("THC_CACHE_DIR")
        .env_remove("THC_CONFIG_DIR")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // A failing assert must not leave the daemon running.
    let mut daemon = common::Guard::spawn(&mut c);
    let status = |v: &str| -> Option<serde_json::Value> {
        let o = h.cmd(&["--json", "--vault", v, "daemon", "status"]);
        serde_json::from_slice::<serde_json::Value>(&o.stdout).ok().filter(|s| s["state"] == "live")
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let (home, acme) = loop {
        if let (Some(a), Some(b)) = (status("personal"), status("acme")) {
            break (a, b);
        }
        assert!(std::time::Instant::now() < deadline, "both vaults never went live");
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    assert_eq!(home["pid"], acme["pid"], "one process for both: {home} {acme}");
    h.ok(&["daemon", "stop"]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while daemon.0.try_wait().unwrap().is_none() {
        assert!(std::time::Instant::now() < deadline, "the daemon didn't stop");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = std::fs::remove_dir_all(&h.home);
}

/// Two vaults may hold different notes with the same keyed id (an agent's `--key` in each):
/// identity across vaults is (vault, id), so both show everywhere and act in their own vault.
#[test]
fn v5_the_same_key_in_two_vaults_is_two_notes() {
    let h = H::new("v5k");
    h.ok(&["init", "thought", "--global"]);
    h.ok(&["vault", "new", "acme"]);
    let mut c = common::thc();
    for (vault, text) in [("personal", "standup notes (home)"), ("acme", "standup notes (acme)")] {
        let o = c.args(["--vault", vault, "todo", text, "--due", "today", "--key", "standup-notes"]).current_dir(&h.home).env("HOME", &h.home).env_remove("THC_VAULT").env_remove("THC_CACHE_DIR").env_remove("THC_CONFIG_DIR").env("THC_NOW", "").output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        c = common::thc();
    }
    let a = h.json(&["--vault", "personal", "q", "text:standup"]);
    let b = h.json(&["--vault", "acme", "q", "text:standup"]);
    assert_eq!(a["items"][0]["id"], b["items"][0]["id"], "the same keyed id in both");
    // thc today and vault:* show both.
    let mut c = common::thc();
    let o = c.args(["--json", "today"]).current_dir(&h.home).env("HOME", &h.home).env_remove("THC_VAULT").env_remove("THC_CACHE_DIR").env_remove("THC_CONFIG_DIR").env("THC_NOW", "").output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let texts: Vec<&str> = v["today"].as_array().unwrap().iter().map(|n| n["text"].as_str().unwrap()).collect();
    assert!(texts.contains(&"standup notes (home)") && texts.contains(&"standup notes (acme)"), "{texts:?}");
    let q = h.json(&["q", "vault:* text:standup"]);
    assert_eq!(q["count"], 2);
    // The TUI shows both rows; done on acme's row lands in acme only.
    let f = h.tui_env(&["tui"], "", &[]);
    let rows: Vec<&str> = f.lines().filter(|l| l.contains("standup notes")).collect();
    assert_eq!(rows.len(), 2, "{f}");
    let row = f.lines().filter(|l| l.contains("[ ]")).position(|l| l.contains("(acme)")).unwrap();
    h.tui_env(&["tui"], &format!("{}x", "j".repeat(row)), &[("THC_TUI_SNAPSHOT_WRITE", "1")]);
    assert_eq!(h.json(&["--vault", "acme", "q", "text:standup status:any"])["items"][0]["status"], "done");
    assert_eq!(h.json(&["--vault", "personal", "q", "text:standup status:any"])["items"][0]["status"], "todo");
    let _ = std::fs::remove_dir_all(&h.home);
}

#[test]
fn sectioned_outline_order_keeps_filtered_ancestors_and_reverses() {
    let h = H::new("section-order");
    h.ok(&["init", "thought", "--global"]);
    let page = h.json(&["page", "new", "Issues"])["nodes"][0]["id"].as_str().unwrap().to_string();
    let a = h.json(&["add", "group", "--under", &page])["nodes"][0]["id"].as_str().unwrap().to_string();
    h.ok(&["todo", "child one", "--under", &a]);
    h.ok(&["todo", "sibling", "--under", &page]);
    h.ok(&["todo", "child two", "--under", &a]);
    h.ok(&["view", "set", "issues", "--section", "Top", "parent:\"¶ Issues\" sort:order", "--section", "Tasks", "under:\"¶ Issues\" status:open sort:order", "--section", "Reverse", "under:\"¶ Issues\" status:open sort:order-"]);
    let q = h.json(&["q", "@issues"]);
    let texts = |section: usize| q["sections"][section]["items"].as_array().unwrap().iter().map(|n| n["text"].as_str().unwrap()).collect::<Vec<_>>();
    assert_eq!(texts(0), ["group", "sibling"]);
    assert_eq!(texts(1), ["child one", "child two", "sibling"]);
    assert_eq!(texts(2), ["sibling", "child two", "child one"]);
    let limited = h.json(&["--limit", "1", "q", "@issues"]);
    assert_eq!(limited["sections"][1]["items"][0]["text"], "child one");
    assert_eq!(limited["sections"][2]["items"][0]["text"], "sibling");
    let _ = std::fs::remove_dir_all(&h.home);
}

#[test]
fn federated_and_sectioned_outline_order_keep_vaults_separate() {
    let h = H::new("federated-order");
    h.ok(&["init", "thought", "--global"]);
    h.ok(&["vault", "new", "acme"]);
    // Pages and keyed tasks deliberately have identical IDs in both vaults.
    for vault in ["personal", "acme"] {
        let page = h.json(&["--vault", vault, "page", "new", "Issues"])["nodes"][0]["id"].as_str().unwrap().to_string();
        let a = h.json(&["--vault", vault, "todo", &format!("{vault} first"), "--under", &page, "--key", "shared-first"])["nodes"][0]["id"].as_str().unwrap().to_string();
        h.ok(&["--vault", vault, "todo", &format!("{vault} child"), "--under", &a, "--key", "shared-child"]);
        h.ok(&["--vault", vault, "todo", &format!("{vault} second"), "--under", &page, "--key", "shared-second"]);
    }
    // Distinct ancestry for the shared child ID: only acme's second task moves ahead.
    let q = h.json(&["--vault", "acme", "q", "text:second"]);
    let second = q["items"][0]["id"].as_str().unwrap();
    let q = h.json(&["--vault", "acme", "q", "text:first"]);
    let first = q["items"][0]["id"].as_str().unwrap();
    h.ok(&["--vault", "acme", "mv", second, "--before", first]);
    let query = "under:\"¶ Issues\" status:open sort:order";
    h.ok(&["view", "set", "issues", "--scope", "vault:*", "--section", "Queue", query]);
    let flat = h.json(&["q", &format!("vault:* {query}")]);
    let sectioned = h.json(&["q", "@issues"]);
    assert_eq!(sectioned["sections"][0]["items"], flat["items"]);
    let texts = |v: &Value| v["items"].as_array().unwrap().iter().map(|n| n["text"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(texts(&flat), ["acme second", "personal first", "acme first", "personal child", "acme child", "personal second"]);
    let reverse = h.json(&["q", &format!("vault:* {query}-")]);
    assert_eq!(texts(&reverse), ["personal second", "personal child", "acme child", "personal first", "acme first", "acme second"]);
    let limited = h.json(&["--limit", "2", "q", &format!("vault:* {query}")]);
    assert_eq!(texts(&limited), ["acme second", "personal first"]);
    let child = h.json(&["--vault", "personal", "q", "text:child"])["items"][0]["id"].as_str().unwrap().to_string();
    h.ok(&["--vault", "personal", "set", &child, "priority=high"]);
    let mixed = h.json(&["q", &format!("vault:* under:\"¶ Issues\" status:open sort:priority sort:order")]);
    assert_eq!(texts(&mixed), ["personal child", "acme second", "personal first", "acme first", "acme child", "personal second"]);
    let _ = std::fs::remove_dir_all(&h.home);
}

#[test]
fn sectioned_query_errors_exit_nonzero_without_partial_results() {
    let h = H::new("section-error");
    h.ok(&["init", "thought", "--global"]);
    h.ok(&["todo", "one task"]);
    h.ok(&["view", "add", "filter", "status:open"]);
    h.ok(&["view", "set", "issues", "--section", "Valid", "status:open sort:order", "--section", "Stale", "@filter sort:order"]);
    h.ok(&["view", "rm", "filter"]);
    for json in [false, true] {
        let args = if json { vec!["--json", "q", "@issues"] } else { vec!["q", "@issues"] };
        let o = h.cmd(&args);
        assert!(!o.status.success(), "a section error must not exit 0");
        assert!(o.stdout.is_empty(), "no partial results: {}", String::from_utf8_lossy(&o.stdout));
        if json {
            let error: Value = serde_json::from_slice(&o.stderr).unwrap();
            assert!(error["error"]["message"].as_str().unwrap().contains("no view @filter"), "{error}");
        } else {
            assert!(String::from_utf8_lossy(&o.stderr).contains("no view @filter"));
        }
    }
    let _ = std::fs::remove_dir_all(&h.home);
}

#[test]
fn v5b_sectioned_views_edit_reset_copy() {
    let h = H::new("v5b");
    h.ok(&["init", "thought", "--global"]);
    h.ok(&["vault", "new", "acme"]);
    h.ok(&["todo", "home task", "--due", "today", "-p", "high"]);
    h.ok(&["--vault", "acme", "todo", "acme task", "--due", "today"]);
    // The built-ins are listed with their sections.
    let ls = h.json(&["view", "ls"]);
    let names: Vec<&str> = ls["sectioned"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["today", "agenda", "inbox", "tasks"]);
    assert_eq!(ls["sectioned"][0]["edited"], false);
    // Edit Today: two sections; thc today runs them across every vault.
    h.ok(&["view", "set", "today", "--section", "Due", "due<=today status:open", "--section", "High", "priority:high status:open"]);
    let t = h.json(&["today"]);
    assert_eq!(t["sections"][0]["title"], "Due");
    assert_eq!(t["sections"][0]["items"].as_array().unwrap().len(), 2);
    assert_eq!(t["sections"][1]["items"][0]["text"], "home task");
    // A copy narrowed to one vault lives in that vault, and is found from anywhere.
    h.ok(&["view", "copy", "today", "acme-today"]);
    let out = h.ok(&["view", "set", "acme-today", "--scope", "vault:acme"]);
    assert!(out.contains("kept in acme"), "{out}");
    let q = h.json(&["q", "@acme-today"]);
    assert_eq!(q["vaults"], serde_json::json!(["acme"]));
    assert_eq!(q["sections"][0]["items"][0]["text"], "acme task");
    // A sectioned view can't go inside a query; reset brings the shipped Today back.
    let o = h.cmd(&["--vault", "acme", "q", "@acme-today #x"]);
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("has sections"), "{}", String::from_utf8_lossy(&o.stderr));
    h.ok(&["view", "reset", "today"]);
    let t = h.json(&["today"]);
    assert!(t.get("sections").is_none() && t["today"].as_array().unwrap().len() == 2, "{t}");
    let _ = std::fs::remove_dir_all(&h.home);
}

#[test]
fn v5b_an_unregistered_vault_never_writes_to_home() {
    // A scratch vault named by path (as a test or THC_VAULT names it) is a world of its own:
    // editing a spanning view there must not land in the registered home vault.
    let h = H::new("v5b-scratch");
    h.ok(&["init", "thought", "--global"]);
    let work = h.home.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let o = common::thc().args(["init", "scratch"]).current_dir(&work).env("HOME", &h.home).env_remove("THC_VAULT").env_remove("THC_CONFIG_DIR").output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let scratch = work.join("scratch");
    let sp = scratch.to_str().unwrap();
    h.ok(&["--vault", sp, "add", "a scratch note"]);
    assert!(!h.config().contains("work/scratch"), "the scratch vault got registered: {}", h.config());
    let before = h.ok(&["log"]);
    h.ok(&["--vault", sp, "view", "set", "today", "--section", "Waiting", "status:waiting"]);
    h.ok(&["--vault", sp, "view", "copy", "today", "mine"]);
    // It lives in the scratch vault and works there.
    let t = h.json(&["--vault", sp, "today"]);
    assert_eq!(t["sections"][0]["title"], "Waiting", "{t}");
    h.ok(&["--vault", sp, "view", "reset", "today"]);
    // Home is untouched: no new transactions, and its Today is the shipped one.
    assert_eq!(h.ok(&["log"]), before);
    let ls = h.json(&["view", "ls"]);
    assert!(ls["sectioned"].as_array().unwrap().iter().all(|s| s["edited"] == false && s["name"] != "mine"), "{ls}");
    let _ = std::fs::remove_dir_all(&h.home);
}

#[test]
fn a_vaults_capture_target_and_under_by_title() {
    // `[capture] target = "¶ Issues"` in a vault's settings.toml: captures that name no place
    // land on that page; one that names a place goes there. `under:"¶ Issues"` finds them.
    let h = H::new("capture-target");
    h.ok(&["init", "thought", "--global"]);
    h.ok(&["vault", "new", "dev"]);
    h.ok(&["--vault", "dev", "page", "new", "Issues"]);
    let info = h.json(&["--vault", "dev", "vault", "info"]);
    let path = info["path"].as_str().or(info["vault"]["path"].as_str()).expect("vault path").to_string();
    std::fs::write(std::path::Path::new(&path).join("settings.toml"), "[capture]\ntarget = \"¶ Issues\"\n").unwrap();
    let out = h.ok(&["--vault", "dev", "todo", "a bug !high"]);
    assert!(out.contains("the vault's capture target"), "{out}");
    h.ok(&["--vault", "dev", "add", "--plain", "a finding about #x"]);
    h.ok(&["--vault", "dev", "add", "--journal", "today", "a journal line"]);
    let q = h.json(&["--vault", "dev", "q", "under:\"¶ Issues\""]);
    let texts: Vec<&str> = q["items"].as_array().unwrap().iter().map(|i| i["text"].as_str().unwrap()).collect();
    assert!(texts.contains(&"a bug") && texts.contains(&"a finding about #x") && !texts.contains(&"a journal line"), "{texts:?}");
    assert_eq!(h.json(&["--vault", "dev", "q", "under:Issues is:task"])["count"], 1);
    // Home has no target: its captures go to the journal as before.
    assert!(!h.ok(&["todo", "home task"]).contains("capture target"));
    let _ = std::fs::remove_dir_all(&h.home);
}

/// Vaults are siloed while you're in one. Only the picker, `--vault`, a project's
/// `.thc.toml` or a row that belongs to another vault may change the vault. Enter on a page in
/// devv's Pages used to move you to personal, because Today's row 0 (a personal task) left its
/// vault tag behind for the next view.
#[test]
fn vaults_stay_siloed_in_every_view() {
    let h = H::new("silo");
    h.ok(&["init", "thought", "--global"]);
    h.ok(&["vault", "new", "devv"]);
    // Personal: a task due today (Today spans vaults, so it's row 0 there), a page, an inbox note.
    h.ok(&["todo", "home task due today", "--due", "today"]);
    h.ok(&["page", "new", "Home Page"]);
    h.ok(&["add", "--inbox", "home inbox note"]);
    // devv: one of everything.
    h.ok(&["--vault", "devv", "page", "new", "Dev Page"]);
    h.ok(&["--vault", "devv", "add", "--inbox", "dev inbox note"]);
    h.ok(&["--vault", "devv", "todo", "dev task"]);
    h.ok(&["--vault", "devv", "add", "dev journal note"]);
    let tui = |extra: &[&str], keys: &str, write: bool| {
        let mut c = common::thc();
        c.args(extra).arg("tui").current_dir(&h.home).env("HOME", &h.home).env_remove("THC_VAULT").env_remove("THC_CACHE_DIR").env_remove("THC_CONFIG_DIR").env("THC_NOW", "2026-10-03T09:00").env("THC_TUI_SNAPSHOT", "110x30").env("THC_TUI_KEYS", keys);
        if write {
            c.env("THC_TUI_SNAPSHOT_WRITE", "1");
        }
        String::from_utf8_lossy(&c.output().unwrap().stdout).to_string()
    };
    let vault_of = |frame: &str| frame.lines().next().unwrap_or_default().split_whitespace().nth(1).unwrap_or_default().to_string();
    let into_devv = "V<up><cr>";
    assert_eq!(vault_of(&tui(&[], into_devv, false)), "devv");
    // Every view but Today (whose row 0 is personal's, and Enter there moves you, by design):
    // Enter on the first row, Esc, Tab, never leave devv.
    for k in ["2", "3", "4", "5", "6", "7"] {
        for tail in ["<cr>", "<cr><esc>", "<cr><esc><tab>", "<tab><s-tab><cr>"] {
            let keys = format!("{into_devv}1{k}{tail}");
            let f = tui(&[], &keys, false);
            assert_eq!(vault_of(&f), "devv", "{keys}:\n{f}");
        }
    }
    // Today's personal row: Enter moves to personal, Esc comes back to devv.
    assert_eq!(vault_of(&tui(&[], &format!("{into_devv}1<cr>"), false)), "personal");
    assert_eq!(vault_of(&tui(&[], &format!("{into_devv}1<cr><esc>"), false)), "devv");
    // Writes land in devv: a line typed in the journal, a capture.
    tui(&[], &format!("{into_devv}5siloed line<esc>"), true);
    assert_eq!(h.json(&["--vault", "devv", "search", "siloed"])["count"], 1);
    assert_eq!(h.json(&["search", "siloed"])["count"], 0);
    // Started with --vault, and from a project folder whose .thc.toml names devv.
    assert_eq!(vault_of(&tui(&["--vault", "devv"], "4<cr><esc><tab>", false)), "devv");
    let info = h.json(&["--vault", "devv", "vault", "info"]);
    let dpath = info["path"].as_str().or(info["vault"]["path"].as_str()).unwrap().to_string();
    let proj = h.home.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join(".thc.toml"), format!("vault = \"{dpath}\"\n")).unwrap();
    let mut c = common::thc();
    c.arg("tui").current_dir(&proj).env("HOME", &h.home).env_remove("THC_VAULT").env_remove("THC_CACHE_DIR").env_remove("THC_CONFIG_DIR").env("THC_TUI_SNAPSHOT", "110x30").env("THC_TUI_KEYS", "4<cr><esc>3<cr>");
    let f = String::from_utf8_lossy(&c.output().unwrap().stdout).to_string();
    assert_eq!(vault_of(&f), "devv", "{f}");
    let _ = std::fs::remove_dir_all(&h.home);
}

/// view-explain.md §1-2, V1-V4: across vaults every row names its vault in one column;
/// `*` scopes Today per device (an override, underlined with the default), `s` saves it.
#[test]
fn v1_to_v4_today_names_each_rows_vault_and_scopes() {
    let h = H::new("scope");
    h.ok(&["init", "thought", "--global"]);
    h.ok(&["vault", "new", "acme"]);
    h.ok(&["vault", "new", "team"]);
    h.ok(&["todo", "Water plants", "--due", "today"]);
    h.ok(&["--vault", "acme", "todo", "Draft Q4 OKRs", "--due", "today"]);
    h.ok(&["--vault", "team", "todo", "Fix the login test", "--due", "today"]);
    let tui = |keys: &str| {
        let o = common::thc()
            .args(["tui"])
            .current_dir(&h.home)
            .env("HOME", &h.home)
            .env_remove("THC_VAULT")
            .env_remove("THC_CACHE_DIR")
            .env_remove("THC_CONFIG_DIR")
            .env("THC_NOW", "2026-10-03T09:00")
            .env("THC_TUI_SNAPSHOT", "100x24")
            .env("THC_TUI_KEYS", keys)
            .env("THC_TUI_SNAPSHOT_WRITE", "1")
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).to_string()
    };
    // V1: every row ends with its vault, this one's too, right-aligned in one column.
    let f = tui("1");
    assert!(f.contains("Today · all vaults (3)"), "{f}");
    let ends: Vec<usize> = ["Water plants", "Draft Q4 OKRs", "Fix the login test"]
        .iter()
        .map(|t| {
            let l = f.lines().find(|l| l.contains(t)).unwrap_or_else(|| panic!("{t}: {f}"));
            assert!(l.trim_end().ends_with("personal") || l.trim_end().ends_with("acme") || l.trim_end().ends_with("team"), "{l}");
            l.trim_end().chars().count()
        })
        .collect();
    assert!(ends.iter().all(|e| *e == ends[0]), "one column: {f}");
    // V2: `*` `.`: this vault only, no column; `*` `a`: all again.
    let f = tui("1*.<cr>");
    assert!(f.contains("Today · personal") && !f.contains("Draft Q4 OKRs"), "{f}");
    assert!(!f.lines().find(|l| l.contains("Water plants")).unwrap().trim_end().ends_with("personal"), "no column: {f}");
    let f = tui("1*a<cr>");
    assert!(f.contains("Today · all vaults (3)") && !f.contains("default"), "{f}");
    // V3: uncheck acme: two vaults, overridden (the default named); and it stays on relaunch.
    let f = tui("1*<down><down><down><space><cr>");
    assert!(f.contains("Today · personal, team") && f.contains("default all vaults"), "{f}");
    let f = tui("1");
    assert!(f.contains("Today · personal, team") && !f.contains("Draft Q4 OKRs"), "kept on this device: {f}");
    // V4: `s` saves it as the default: no override any more.
    let f = tui("1*s");
    assert!(f.contains("Today · personal, team") && !f.contains("default"), "{f}");
    let v = h.json(&["view", "ls"]);
    assert!(v.to_string().contains("personal or team"), "saved: {v}");
}

/// view-explain.md §3, V5-V8: `?` on a view shows how it's built; `e` edits it in
/// $EDITOR (a section per line), `r` resets a built-in; Inbox, Tasks and saved views too.
#[test]
fn v5_to_v8_a_view_explains_and_edits_itself() {
    let h = H::new("recipe");
    h.ok(&["init", "thought", "--global"]);
    h.ok(&["vault", "new", "acme"]);
    h.ok(&["todo", "Water plants", "--due", "today"]);
    h.ok(&["--vault", "acme", "todo", "Draft Q4 OKRs", "--due", "2026-10-01"]);
    let run = |keys: &str, env: &[(&str, &str)]| {
        let mut c = common::thc();
        c.args(["tui"])
            .current_dir(&h.home)
            .env("HOME", &h.home)
            .env_remove("THC_VAULT")
            .env_remove("THC_CACHE_DIR")
            .env_remove("THC_CONFIG_DIR")
            .env("THC_NOW", "")
            .env("THC_TUI_SNAPSHOT", "120x30")
            .env("THC_TUI_KEYS", keys)
            .env("THC_TUI_SNAPSHOT_WRITE", "1");
        for (k, v) in env {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).to_string()
    };
    // V5: every section with its count, query and reading; the scope line.
    let f = run("1?", &[]);
    assert!(f.contains("@today · built-in") && f.contains("scope  all vaults (2): personal, acme"), "{f}");
    let overdue = f.lines().find(|l| l.contains("Overdue") && l.contains("is:overdue status:open")).unwrap_or_else(|| panic!("{f}"));
    assert!(overdue.contains("  2  ") && overdue.contains("overdue"), "count (both vaults) and reading: {overdue}");
    assert!(f.contains("Next 7 days") && f.contains("Done today"), "{f}");
    // `?` again: the keys.
    assert!(run("1??", &[]).contains("every key") || run("1??", &[]).contains("keys"));
    // V6: e edits it in $EDITOR: Overdue's query changes, and Today runs it.
    let script = h.home.join("ed.sh");
    std::fs::write(&script, "#!/bin/sh\nsed -i '' -e 's/^Overdue · is:overdue status:open$/Overdue · is:overdue status:open #nothing/' \"$1\"\n").unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let ed = [("VISUAL", script.to_str().unwrap()), ("THC_TUI_SNAPSHOT_EDITOR", "1")];
    let f = run("1?e", &ed);
    assert!(f.contains("@today saved"), "{f}");
    let f = run("1?", &[]);
    assert!(f.contains("built-in · edited") && f.contains("#nothing"), "edited: {f}");
    assert!(!run("1", &[]).contains("Draft Q4 OKRs"), "Today runs the new Overdue (nothing tagged #nothing)");
    // A line that doesn't read is named (the editor runs again with it on top; this one gives up).
    let bad = h.home.join("bad.sh");
    std::fs::write(&bad, "#!/bin/sh\necho 'just words' >> \"$1\"\n").unwrap();
    std::fs::set_permissions(&bad, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let f = run("1?e", &[("VISUAL", bad.to_str().unwrap()), ("THC_TUI_SNAPSHOT_EDITOR", "1")]);
    assert!(f.contains("wants Title · query"), "{f}");
    // V7: r: the shipped one again.
    let f = run("1?r", &[]);
    assert!(f.contains("shipped one again"), "{f}");
    assert!(run("1?", &[]).contains("@today · built-in ") || !run("1?", &[]).contains("edited"));
    // V8: Inbox and Tasks explain themselves too.
    assert!(run("2?", &[]).contains("@inbox · built-in"));
    assert!(run("3?", &[]).contains("@tasks · built-in"));
}

#[test]
fn registry_reads_preserve_default_today_and_vault_routing() {
    let h = H::new("registry-reads");
    h.ok(&["init", "thought", "--global"]);
    h.ok(&["vault", "new", "acme"]);
    let task = h.json(&["--vault", "acme", "todo", "registry read fixture", "--due", "today"]);
    let id = task["nodes"][0]["id"].as_str().unwrap();
    let qualified = format!("acme/{id}");

    // No subcommand in a pipe or with --json is the registered Today command.
    assert_eq!(h.json(&["--vault", "acme"]), h.json(&["--vault", "acme", "today"]));
    assert_eq!(h.ok(&["--vault", "acme"]), h.ok(&["--vault", "acme", "today"]));
    for args in [vec!["today"], vec!["agenda"], vec!["inbox"], vec!["journal"], vec!["search", "registry"]] {
        let mut scoped = vec!["--vault", "acme"];
        scoped.extend(args);
        assert!(h.ok(&scoped).starts_with("vault acme"), "{scoped:?}");
    }
    // Pages retains its page-ls output, without the listing banner.
    assert_eq!(h.ok(&["--vault", "acme", "pages"]), h.ok(&["--vault", "acme", "page", "ls"]));
    assert!(!h.ok(&["--vault", "acme", "pages"]).starts_with("vault acme"));

    // Qualified IDs select acme even though the current vault is personal.
    for command in ["show", "history", "diff"] {
        let result = h.json(&[command, &qualified]);
        if command == "show" { assert_eq!(result["vault"], "acme"); }
        assert_eq!(result, h.json(&["--vault", "acme", command, id]), "{command}");
    }
    let _ = std::fs::remove_dir_all(&h.home);
}
