//! `thc vault`: the registry of vaults on this device (vaults.md §4).

use crate::cli::VaultCmd;
use crate::out::Out;
use anyhow::Result;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use thc_core::error::{invalid as validation, not_found, usage};
use thc_core::event::Actor;
use thc_core::registry::{self, Entry, Registry};
use thc_core::vault::{self, Paths, Vault, VaultSource, tilde};

/// The current vault's folder and why, if one resolves.
fn current(flag: Option<&Path>) -> Option<(PathBuf, VaultSource)> {
    Paths::resolve_with_source(flag).ok().map(|(p, s)| (p.vault, s))
}

/// Open and closed counts for a vault: (open tasks, inbox items). None when it can't be read.
fn counts(path: &Path) -> Option<(usize, usize)> {
    if !path.join(vault::VAULT_MARKER).exists() {
        return None;
    }
    let v = Vault::open(Paths { vault: path.to_path_buf(), cache: vault::default_cache(path) }, Actor { kind: "human".into(), name: None }, "cli").ok()?;
    let today = thc_core::dates::today();
    let open = v.store.query("status:open", today, 1_000_000).ok()?.len();
    let inbox = v.store.query("is:inbox", today, 1_000_000).ok()?.len();
    Some((open, inbox))
}

fn entry_json(reg: &Registry, e: &Entry, cur: Option<&(PathBuf, VaultSource)>) -> Value {
    let is_current = cur.is_some_and(|(p, _)| reg.by_path(p).is_some_and(|c| c.name == e.name));
    let c = counts(&e.path);
    let mut v = json!({
        "name": e.name,
        "id": e.id.clone().or_else(|| registry::read_identity(&e.path).id),
        "path": e.path,
        "home": reg.home.as_deref() == Some(e.name.as_str()),
        "current": is_current,
        "sync": registry::sync_of(&e.path),
        "exists": e.path.join(vault::VAULT_MARKER).exists(),
    });
    if let Some((open, inbox)) = c {
        v["open"] = json!(open);
        v["inbox"] = json!(inbox);
    }
    if is_current {
        v["why"] = json!(cur.map(|(_, s)| s.describe()));
    }
    v
}

pub fn run(out: &mut Out, flag: Option<&Path>, cmd: Option<VaultCmd>) -> Result<()> {
    let mut reg = Registry::load();
    match cmd {
        None => {
            let Some((path, src)) = current(flag) else { return Err(usage("no vault here · thc vault ls, or thc vault new <name>".to_string())) };
            let name = reg.name_for(&path);
            if out.json {
                let id = registry::read_identity(&path).id;
                out.json(&json!({ "name": name, "id": id, "path": path, "home": reg.is_home(&path), "source": src.kind(), "why": src.describe() }));
            } else {
                let why = out.dim(&format!(" ({})", src.describe()));
                out.line(format!("{name}{why}"));
            }
        }
        Some(VaultCmd::Ls) => {
            let cur = current(flag);
            let rows: Vec<Value> = reg.vaults.iter().map(|e| entry_json(&reg, e, cur.as_ref())).collect();
            if out.json {
                out.json(&json!({ "vaults": rows }));
                return Ok(());
            }
            if rows.is_empty() {
                out.line("no vaults registered · thc vault new <name>, or thc vault add <path>");
                return Ok(());
            }
            let nw = reg.vaults.iter().map(|e| e.name.len()).max().unwrap_or(4).max(4);
            let pw = reg.vaults.iter().map(|e| tilde(&e.path).chars().count()).max().unwrap_or(4).clamp(4, 44);
            out.line(out.dim(&format!("  {:<nw$}  {:<pw$}  OPEN  INBOX  SYNC", "NAME", "PATH")));
            for r in &rows {
                let mark = if r["current"] == true { "*" } else { " " };
                let path = thc_tui_free_truncate(&tilde(Path::new(r["path"].as_str().unwrap_or(""))), pw);
                let num = |k: &str| r.get(k).and_then(|v| v.as_u64()).map_or("  --".to_string(), |n| format!("{n:>4}"));
                let mut sync = r["sync"].as_str().unwrap_or("").to_string();
                if r["home"] == true {
                    sync.push_str(" · home");
                }
                if r["exists"] == false {
                    sync = "missing · thc vault rm, or put the folder back".into();
                }
                let name = r["name"].as_str().unwrap_or("");
                let open = format!("{:0>2}", num("open").trim());
                let inbox = format!("{:0>2}", num("inbox").trim());
                out.line(format!("{mark} {name:<nw$}  {path:<pw$}  {open:>4}  {inbox:>5}  {sync}"));
            }
            if let Some((_, src)) = cur {
                out.line(out.dim(&format!("* current ({})", src.describe())));
            }
        }
        Some(VaultCmd::New { name, path }) => {
            let (e, accent) = registry::create_vault(&mut reg, &name, path)?;
            let path = e.path.clone();
            let id = e.id.clone();
            if out.json {
                out.json(&json!({ "ok": true, "name": name, "id": id, "path": path, "sync": registry::sync_of(&path), "accent": accent }));
            } else {
                let sync = registry::sync_of(&path);
                let where_ = if sync == "local only" { String::new() } else { format!(" ({sync})") };
                let tail = if sync == "local only" {
                    format!(" · thc vault use {name}, or add vault = \"{name}\" to a project's .thc.toml")
                } else {
                    " · share that folder with your team".to_string()
                };
                out.line(format!("created vault {name} at {}{where_}{}", tilde(&path), out.dim(&tail)));
            }
        }
        Some(VaultCmd::Add { path, as_name }) => {
            let path = if path.is_absolute() { path } else { std::env::current_dir()?.join(path) };
            let path = path.canonicalize().unwrap_or(path);
            if !path.join(vault::VAULT_MARKER).exists() {
                return Err(not_found(format!("no vault at {} (no {}) · thc vault new <name> {} creates one", tilde(&path), vault::VAULT_MARKER, tilde(&path))));
            }
            if let Some(e) = reg.by_path(&path) {
                return Err(validation(format!("{} is already registered as {}", tilde(&path), e.name)));
            }
            let ident = registry::read_identity(&path);
            let synced = ident.name.clone().filter(|n| registry::valid_name(n)).unwrap_or_else(|| registry::name_from_path(&path));
            let name = as_name.unwrap_or(synced);
            if !registry::valid_name(&name) {
                return Err(validation(format!("\"{name}\" isn't a vault name · lowercase letters, digits and -")));
            }
            if reg.find(&name).is_some() {
                return Err(validation(format!("there's already a vault named {name} here · thc vault add {} --as <another-name>", tilde(&path))));
            }
            let id = registry::ensure_identity(&path, &name)?.id;
            reg.vaults.push(Entry { name: name.clone(), path: path.clone(), id: id.clone() });
            normalize(&mut reg);
            reg.save()?;
            let c = counts(&path);
            if out.json {
                out.json(&json!({ "ok": true, "name": name, "id": id, "path": path, "open": c.map(|c| c.0) }));
            } else {
                let created = ident.created.map(|c| format!("created {c} · ")).unwrap_or_default();
                let n = c.map(|c| format!("{} open", c.0)).unwrap_or_default();
                out.line(format!("registered vault {name}{}", out.dim(&format!(" ({created}{n})"))));
            }
        }
        Some(VaultCmd::Use { name }) => {
            let Some(e) = reg.find(&name).cloned() else { return Err(unknown(&reg, &name)) };
            normalize(&mut reg);
            reg.current = if reg.home.as_deref() == Some(name.as_str()) { None } else { Some(name.clone()) };
            reg.save()?;
            if out.json {
                out.json(&json!({ "ok": true, "current": name, "path": e.path }));
            } else {
                out.line(format!("now using {name} on this device{}", out.dim(" · .thc.toml files still win inside their projects")));
            }
        }
        Some(VaultCmd::Info { name }) => {
            let path = match &name {
                Some(n) => reg.find(n).map(|e| e.path.clone()).ok_or_else(|| unknown(&reg, n))?,
                None => current(flag).map(|(p, _)| p).ok_or_else(|| usage("no vault here · thc vault ls".to_string()))?,
            };
            let ident = registry::read_identity(&path);
            let shown = reg.name_for(&path);
            let sync = registry::sync_of(&path);
            let v = Vault::open(Paths { vault: path.clone(), cache: vault::default_cache(&path) }, Actor { kind: "human".into(), name: None }, "cli")?;
            let today = thc_core::dates::today();
            let notes: i64 = v.store.conn.query_row("SELECT count(*) FROM nodes WHERE deleted = 0", [], |r| r.get(0))?;
            let open = v.store.query("status:open", today, 1_000_000)?.len();
            let last = v.store.history_where("1 ORDER BY okey DESC LIMIT 1", &[])?.into_iter().next();
            if out.json {
                out.json(&json!({
                    "name": shown, "id": ident.id, "synced_name": ident.name, "path": path, "sync": sync, "created": ident.created,
                    "home": reg.is_home(&path), "notes": notes, "open": open,
                    "last_change": last.as_ref().map(|e| json!({ "ms": e.ms, "actor": e.actor, "dev": e.dev })),
                }));
            } else {
                out.line(format!("{shown} · {} · {sync}", tilde(&path)));
                if let Some(c) = &ident.created {
                    out.line(out.dim(&format!("created {c}")));
                }
                let when = last.as_ref().and_then(|e| chrono::DateTime::from_timestamp_millis(e.ms)).map(|d| d.with_timezone(&chrono::Local).format("%b %-d %H:%M").to_string());
                let who = last.as_ref().map(|e| if e.actor.starts_with("agent:") { format!("◆ {}", e.actor.trim_start_matches("agent:")) } else { e.actor.clone() });
                let mut line = format!("{notes} notes · {open} open");
                if let (Some(w), Some(a)) = (when, who) {
                    line.push_str(&format!(" · last change {w} by {a}"));
                }
                out.line(line);
            }
        }
        Some(VaultCmd::Rename { old, new }) => {
            if !registry::valid_name(&new) {
                return Err(validation(format!("\"{new}\" isn't a vault name · lowercase letters, digits and -")));
            }
            if reg.find(&new).is_some() {
                return Err(validation(format!("there's already a vault named {new}")));
            }
            let Some(i) = reg.vaults.iter().position(|e| e.name == old) else { return Err(unknown(&reg, &old)) };
            registry::set_name(&reg.vaults[i].path, &new)?;
            reg.vaults[i].name = new.clone();
            if reg.home.as_deref() == Some(old.as_str()) {
                reg.home = Some(new.clone());
            }
            if reg.current.as_deref() == Some(old.as_str()) {
                reg.current = Some(new.clone());
            }
            normalize(&mut reg);
            reg.save()?;
            if out.json {
                out.json(&json!({ "ok": true, "old": old, "name": new }));
            } else {
                out.line(format!("renamed {old} → {new}{}", out.dim(" (synced to everyone who has it)")));
            }
        }
        Some(VaultCmd::Rm { name }) => {
            let Some(e) = reg.find(&name).cloned() else { return Err(unknown(&reg, &name)) };
            if reg.home.as_deref() == Some(name.as_str()) {
                return Err(validation(format!("{name} is the home vault · make another one home first (thc init <path> --global)")));
            }
            reg.vaults.retain(|x| x.name != name);
            if reg.current.as_deref() == Some(name.as_str()) {
                reg.current = None;
            }
            normalize(&mut reg);
            reg.save()?;
            if out.json {
                out.json(&json!({ "ok": true, "name": name, "path": e.path, "deleted": false }));
            } else {
                out.line(format!("unregistered {name}{}", out.dim(&format!(" · nothing deleted · its files stay at {} · thc vault add brings it back", tilde(&e.path)))));
            }
        }
    }
    Ok(())
}

/// Before a save: a pre-vaults config's home gets its id recorded.
fn normalize(reg: &mut Registry) {
    for e in reg.vaults.iter_mut() {
        if e.id.is_none() {
            e.id = registry::read_identity(&e.path).id;
        }
    }
}

fn unknown(reg: &Registry, name: &str) -> anyhow::Error {
    let names: Vec<&str> = reg.vaults.iter().map(|e| e.name.as_str()).collect();
    let near = names.iter().find(|n| strsim_close(n, name)).map(|n| format!(" · did you mean {n}?")).unwrap_or_default();
    validation(format!("no vault \"{name}\"{near} · thc vault ls"))
}

/// A one- or two-letter slip (`acm` for `acme`).
fn strsim_close(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
        prev = cur;
    }
    prev[b.len()] <= 2
}

fn thc_tui_free_truncate(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        return s.to_string();
    }
    let keep = w.saturating_sub(1);
    let head = keep / 3;
    let tail = keep - head;
    let chars: Vec<char> = s.chars().collect();
    format!("{}…{}", chars[..head].iter().collect::<String>(), chars[chars.len() - tail..].iter().collect::<String>())
}
