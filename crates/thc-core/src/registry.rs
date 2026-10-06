//! Vaults by name (vaults.md §1–2, vaults-architecture.md §1): each vault's identity in its
//! `thc-vault.toml`, and this device's registry in `~/.config/thought/config.toml`:
//!
//! ```toml
//! home = "personal"
//! current = "acme"          # `thc vault use acme` (device-local)
//!
//! [vaults.personal]
//! path = "~/thought"
//! id = "k7q2m…"
//! ```
//!
//! A config from before vaults has `vault = "~/thought"` instead: it reads as the home vault
//! (named from its identity, else `personal`) and becomes the form above on the first write.

use crate::error::usage;
use crate::vault::{VAULT_MARKER, global_config_path, tilde};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// A vault's synced identity (the marker file's `id`, `name`, `created`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub id: Option<String>,
    pub name: Option<String>,
    pub created: Option<String>,
}

pub fn read_identity(vault: &Path) -> Identity {
    let Ok(text) = fs::read_to_string(vault.join(VAULT_MARKER)) else { return Identity::default() };
    let Ok(t) = text.parse::<toml::Table>() else { return Identity::default() };
    let s = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_string);
    Identity { id: s("id"), name: s("name"), created: s("created") }
}

/// Give a vault an identity if its marker has none yet: an `id`, a `name` (`default_name`) and
/// `created`. The id of a vault that already has events is derived from its oldest event, so
/// two devices that both add it before syncing agree. Other lines of the marker are kept.
pub fn ensure_identity(vault: &Path, default_name: &str) -> Result<Identity> {
    let have = read_identity(vault);
    if have.id.is_some() && have.name.is_some() {
        return Ok(have);
    }
    let marker = vault.join(VAULT_MARKER);
    crate::sandbox::check(&marker);
    let text = fs::read_to_string(&marker).with_context(|| format!("reading {}", marker.display()))?;
    let id = have.id.clone().unwrap_or_else(|| match oldest_eid(vault) {
        Some(e) => crate::id::from_key(&format!("vault:{e}")),
        None => crate::id::new_id(),
    });
    let name = have.name.clone().unwrap_or_else(|| default_name.to_string());
    let created = have.created.clone().unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string());
    let mut lines: Vec<String> = text.lines().filter(|l| !["id", "name", "created"].iter().any(|k| key_of(l) == Some(*k))).map(str::to_string).collect();
    lines.push(format!("id = {id:?}"));
    lines.push(format!("name = {name:?}"));
    lines.push(format!("created = {created:?}"));
    write_atomic(&marker, &(lines.join("\n") + "\n"))?;
    Ok(Identity { id: Some(id), name: Some(name), created: Some(created) })
}

/// Rename a vault (its synced name).
pub fn set_name(vault: &Path, name: &str) -> Result<()> {
    let marker = vault.join(VAULT_MARKER);
    crate::sandbox::check(&marker);
    let text = fs::read_to_string(&marker)?;
    let mut lines: Vec<String> = text.lines().filter(|l| key_of(l) != Some("name")).map(str::to_string).collect();
    lines.push(format!("name = {name:?}"));
    write_atomic(&marker, &(lines.join("\n") + "\n"))
}

/// The smallest event id in the vault's logs (ULIDs sort by time): the same on every device
/// that has synced the vault. Reads only each file's first line.
fn oldest_eid(vault: &Path) -> Option<String> {
    let mut best: Option<String> = None;
    for dev in fs::read_dir(vault.join("log")).ok()?.flatten() {
        let Ok(files) = fs::read_dir(dev.path()) else { continue };
        for f in files.flatten() {
            let Ok(text) = fs::read_to_string(f.path()) else { continue };
            let Some(first) = text.lines().find(|l| !l.trim().is_empty()) else { continue };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(first) else { continue };
            if let Some(e) = v.get("eid").and_then(|e| e.as_str()) {
                if best.as_deref().is_none_or(|b| e < b) {
                    best = Some(e.to_string());
                }
            }
        }
    }
    best
}

/// A vault name: lowercase letters, digits and `-`, starting with a letter or digit.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 40 && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !name.starts_with('-')
}

/// A name from a folder: `~/thought-vaults/Side Project` → `side-project`; a project's `vault`
/// folder takes its parent's name.
pub fn name_from_path(vault: &Path) -> String {
    let base = match vault.file_name().and_then(|n| n.to_str()) {
        Some("vault") | Some(".thought") => vault.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("vault"),
        Some(n) => n,
        None => "vault",
    };
    let mut s: String = base.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "vault".into() } else { s }
}

/// One registered vault.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub id: Option<String>,
}

/// This device's registry.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    pub home: Option<String>,
    /// `thc vault use <name>`: the device's current vault when no flag, env or `.thc.toml` says.
    pub current: Option<String>,
    pub vaults: Vec<Entry>,
    /// Read from a pre-vaults `vault = …` key (rewritten on the next save).
    pub legacy: bool,
    pub file: Option<PathBuf>,
}

impl Registry {
    pub fn load() -> Registry {
        let Some(file) = global_config_path() else { return Registry::default() };
        let text = fs::read_to_string(&file).unwrap_or_default();
        let mut r = Self::parse(&text);
        r.file = Some(file);
        r
    }

    pub fn parse(text: &str) -> Registry {
        let t: toml::Table = text.parse().unwrap_or_default();
        let s = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let mut r = Registry { home: s("home"), current: s("current"), ..Default::default() };
        if let Some(vs) = t.get("vaults").and_then(|v| v.as_table()) {
            for (name, e) in vs {
                let Some(path) = e.get("path").and_then(|p| p.as_str()) else { continue };
                r.vaults.push(Entry { name: name.clone(), path: expand_home(path), id: e.get("id").and_then(|i| i.as_str()).map(str::to_string) });
            }
        }
        if r.vaults.is_empty() {
            if let Some(v) = s("vault") {
                let path = expand_home(&v);
                let name = read_identity(&path).name.filter(|n| valid_name(n)).unwrap_or_else(|| "personal".into());
                r.vaults.push(Entry { name: name.clone(), path, id: None });
                r.home = Some(name);
                r.legacy = true;
            }
        }
        // Home first, then the order of the file.
        if let Some(h) = r.home.clone() {
            r.vaults.sort_by_key(|e| e.name != h);
        }
        r
    }

    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.vaults.iter().find(|e| e.name == name)
    }

    pub fn home_entry(&self) -> Option<&Entry> {
        self.home.as_deref().and_then(|h| self.find(h))
    }

    /// The entry for a vault folder, by path (then by id).
    pub fn by_path(&self, vault: &Path) -> Option<&Entry> {
        // The same spelling first (no syscalls: every command names its vault).
        if let Some(e) = self.vaults.iter().find(|e| e.path == vault) {
            return Some(e);
        }
        let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let want = canon(vault);
        self.vaults.iter().find(|e| canon(&e.path) == want).or_else(|| {
            let id = read_identity(vault).id?;
            self.vaults.iter().find(|e| e.id.as_deref() == Some(id.as_str()))
        })
    }

    pub fn is_home(&self, vault: &Path) -> bool {
        match (self.by_path(vault), &self.home) {
            (Some(e), Some(h)) => &e.name == h,
            _ => false,
        }
    }

    /// The name to show for a vault: its registry name, else its synced name, else its folder's.
    pub fn name_for(&self, vault: &Path) -> String {
        self.by_path(vault).map(|e| e.name.clone()).or_else(|| read_identity(vault).name).unwrap_or_else(|| name_from_path(vault))
    }

    /// Write the registry back: `home`, `current` and every `[vaults.<name>]` (path and id).
    /// Everything else in the file (other tables, comments, `[vaults.<name>.settings]`) stays.
    pub fn save(&self) -> Result<PathBuf> {
        let file = self.file.clone().or_else(global_config_path).ok_or_else(|| anyhow::anyhow!("no config path (HOME isn't set)"))?;
        crate::sandbox::check(&file);
        if let Some(d) = file.parent() {
            fs::create_dir_all(d)?;
        }
        let old = fs::read_to_string(&file).unwrap_or_default();
        write_atomic(&file, &self.render(&old))?;
        Ok(file)
    }

    fn render(&self, old: &str) -> String {
        let mut top: Vec<String> = Vec::new();
        if let Some(h) = &self.home {
            top.push(format!("home = {h:?}"));
        }
        if let Some(c) = &self.current {
            top.push(format!("current = {c:?}"));
        }
        // Drop what we rewrite: the top-level registry keys (and the legacy `vault`), and each
        // exact `[vaults.<name>]` table. Sub-tables (`[vaults.acme.settings]`) are the person's.
        let mut kept: Vec<&str> = Vec::new();
        let mut in_top = true;
        let mut skipping = false;
        for l in old.lines() {
            let t = l.trim();
            if t.starts_with('[') {
                in_top = false;
                skipping = t.strip_prefix("[vaults.").and_then(|r| r.strip_suffix(']')).is_some_and(|name| !name.contains('.') && !name.contains('"'));
                if skipping {
                    continue;
                }
            }
            if skipping {
                continue;
            }
            if in_top && matches!(key_of(l), Some("home" | "current" | "vault")) {
                continue;
            }
            kept.push(l);
        }
        while kept.last().is_some_and(|l| l.trim().is_empty()) {
            kept.pop();
        }
        let mut out = top.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        let body = kept.join("\n");
        if !body.trim().is_empty() {
            if !out.is_empty() && !body.starts_with('\n') {
                out.push('\n');
            }
            out.push_str(body.trim_start_matches('\n'));
            out.push('\n');
        }
        for e in &self.vaults {
            out.push_str(&format!("\n[vaults.{}]\npath = {:?}\n", e.name, tilde(&e.path)));
            if let Some(id) = &e.id {
                out.push_str(&format!("id = {id:?}\n"));
            }
        }
        out
    }
}

/// Create a vault and register it (`thc vault new`, the TUI picker's `n`): checked name, the
/// default folder `~/thought-vaults/<name>` unless one is given, never inside another vault, its
/// identity, and the next unused accent in its `settings.toml`. Saves the registry. Returns the
/// entry and the accent.
pub fn create_vault(reg: &mut Registry, name: &str, path: Option<PathBuf>) -> Result<(Entry, &'static str)> {
    use crate::error::invalid;
    if !valid_name(name) {
        return Err(invalid(format!("\"{name}\" isn't a vault name · lowercase letters, digits and -")));
    }
    if reg.find(name).is_some() {
        return Err(invalid(format!("there's already a vault named {name} · thc vault ls")));
    }
    let path = match path {
        Some(p) if p.is_absolute() => p,
        Some(p) => std::env::current_dir()?.join(p),
        None => default_path(name)?,
    };
    if let Some(outer) = path.parent().and_then(enclosing_vault) {
        return Err(invalid(format!("{} is inside the vault at {} · vaults never nest: pick a folder outside it", tilde(&path), tilde(&outer))));
    }
    if path.join(VAULT_MARKER).exists() {
        return Err(invalid(format!("{} is already a vault · thc vault add {}", tilde(&path), tilde(&path))));
    }
    crate::vault::init(&path, None, None)?;
    let id = ensure_identity(&path, name)?.id;
    // Its own colour, so two vaults never look alike unless you choose (vaults.md §10.1).
    let accent = crate::settings::next_accent(reg);
    let sf = path.join(crate::settings::VAULT_FILE);
    if !sf.exists() {
        fs::write(&sf, format!("# Settings for this vault: synced with it, shared by everyone who has it, and layered\n# over each person's own config (thc config --effective). Look and feel only.\n\n[theme]\naccent = {accent:?}   # ember, rose, sea, iris or graphite\n"))?;
    }
    let e = Entry { name: name.to_string(), path, id };
    reg.vaults.push(e.clone());
    for x in reg.vaults.iter_mut() {
        if x.id.is_none() {
            x.id = read_identity(&x.path).id;
        }
    }
    reg.save()?;
    Ok((e, accent))
}

/// Where a vault's files are synced, from its path: `iCloud Drive`, `Dropbox`, `Google Drive`,
/// `OneDrive`, `Syncthing`, `git`, or `local only`.
pub fn sync_of(vault: &Path) -> &'static str {
    let s = vault.to_string_lossy();
    if s.contains("Library/Mobile Documents") {
        return "iCloud Drive";
    }
    if s.contains("CloudStorage/Dropbox") || s.contains("/Dropbox/") {
        return "Dropbox";
    }
    if s.contains("CloudStorage/GoogleDrive") || s.contains("/Google Drive/") {
        return "Google Drive";
    }
    if s.contains("CloudStorage/OneDrive") || s.contains("/OneDrive/") {
        return "OneDrive";
    }
    let mut d = Some(vault);
    while let Some(p) = d {
        if p.join(".stfolder").exists() {
            return "Syncthing";
        }
        if p.join(".git").exists() {
            return "git";
        }
        d = p.parent();
    }
    "local only"
}

/// The vault a path is inside, if any (vaults never nest).
pub fn enclosing_vault(path: &Path) -> Option<PathBuf> {
    let mut d = Some(path);
    while let Some(p) = d {
        if p.join(VAULT_MARKER).exists() {
            return Some(p.to_path_buf());
        }
        d = p.parent();
    }
    None
}

/// The default folder for a new vault: `~/thought-vaults/<name>` (beside the home vault, never
/// inside it).
pub fn default_path(name: &str) -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| usage("HOME isn't set: pass a path".to_string()))?;
    Ok(PathBuf::from(home).join("thought-vaults").join(name))
}

fn key_of(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if t.starts_with('#') {
        return None;
    }
    let (k, _) = t.split_once('=')?;
    Some(k.trim())
}

pub fn expand_home(p: &str) -> PathBuf {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(p),
    }
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_legacy_config_reads_as_home_and_saves_as_a_registry() {
        let r = Registry::parse("vault = \"/nowhere/thought\"\n\n[tui]\njournal = \"focus\"\n");
        assert!(r.legacy);
        assert_eq!(r.home.as_deref(), Some("personal"));
        assert_eq!(r.vaults[0].path, PathBuf::from("/nowhere/thought"));
        let out = r.render("vault = \"/nowhere/thought\"\n\n[tui]\njournal = \"focus\"\n");
        assert!(out.starts_with("home = \"personal\"\n"), "{out}");
        assert!(!out.contains("vault = "), "{out}");
        assert!(out.contains("[tui]\njournal = \"focus\"") && out.contains("[vaults.personal]\npath = \"/nowhere/thought\""), "{out}");
        let back = Registry::parse(&out);
        assert!(!back.legacy);
        assert_eq!(back.home_entry().map(|e| e.path.clone()), Some(PathBuf::from("/nowhere/thought")));
    }

    #[test]
    fn save_keeps_settings_tables_and_comments() {
        let old = "home = \"personal\"\n# mine\n[vaults.personal]\npath = \"/a\"\n\n[vaults.acme.settings.tui]\nleader_popup = \"delay\"\n\n[vaults.acme]\npath = \"/b\"\n";
        let mut r = Registry::parse(old);
        r.vaults.retain(|e| e.name != "acme");
        let out = r.render(old);
        assert!(out.contains("# mine") && out.contains("[vaults.acme.settings.tui]\nleader_popup = \"delay\""), "{out}");
        assert!(!out.contains("path = \"/b\""), "{out}");
        assert_eq!(out.matches("[vaults.personal]").count(), 1, "{out}");
    }

    #[test]
    fn names() {
        assert!(valid_name("acme") && valid_name("side-2") && !valid_name("Acme") && !valid_name("-x") && !valid_name(""));
        assert_eq!(name_from_path(Path::new("/x/Side Project")), "side-project");
        assert_eq!(name_from_path(Path::new("/code/acme/vault")), "acme");
    }
}
