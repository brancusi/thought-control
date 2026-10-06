//! Settings in layers (vaults.md §9): built-in defaults < your config < the vault's
//! `settings.toml` (synced, a diff, whitelisted) < your `[vaults.<name>.settings]`. Env vars and
//! flags beat all four, in their consumers (`THC_THEME`, `THC_TUI_*`, …).
//!
//! A key in a higher layer replaces the same key below, one key at a time; tables merge. A vault
//! may change how thc looks and feels, never what it runs or trusts: everything else in its file
//! is refused with a notice, and a file that doesn't parse is ignored with one.

use crate::registry::Registry;
use crate::vault::{global_config_path, tilde};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

pub const VAULT_FILE: &str = "settings.toml";

/// What a vault's `settings.toml` may set (vaults.md §9.2): key paths, or prefixes ending `.`.
const ALLOWED: &[&str] = &["tui.", "keys.", "theme.accent", "theme.theme", "views.", "capture.", "vault.name_short", "attachments.max_mb", "attachments.max_dimension"];

/// The accents a vault can wear (vaults.md §10.1). `ember` is the home vault's, always.
pub const ACCENTS: [&str; 5] = ["ember", "rose", "sea", "iris", "graphite"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    You,
    Vault(String),
    Override(String),
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Source::You => "you".into(),
            Source::Vault(n) => format!("vault {n}"),
            Source::Override(n) => format!("vaults.{n} override"),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Source::You => "user",
            Source::Vault(_) => "vault",
            Source::Override(_) => "override",
        }
    }
}

/// One setting as some layer had it.
#[derive(Clone, Debug)]
pub struct Entry {
    pub value: toml::Value,
    pub source: Source,
    pub file: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct Effective {
    /// The merged settings (the registry's own keys left out).
    pub table: toml::Table,
    /// Every leaf key (dotted) with every layer that set it, lowest first: the last one wins.
    pub layers: BTreeMap<String, Vec<Entry>>,
    /// What was ignored, in words for the bar and `thc doctor`.
    pub notices: Vec<String>,
    pub vault_name: Option<String>,
}

impl Effective {
    pub fn get(&self, dotted: &str) -> Option<&toml::Value> {
        let mut cur: Option<&toml::Value> = None;
        let mut table = &self.table;
        for part in dotted.split('.') {
            cur = table.get(part);
            match cur {
                Some(toml::Value::Table(t)) => table = t,
                Some(_) => {}
                None => return None,
            }
        }
        cur
    }

    pub fn str(&self, dotted: &str) -> Option<&str> {
        self.get(dotted).and_then(|v| v.as_str())
    }

    /// The vault's accent: `[theme] accent`, else ember.
    pub fn accent(&self) -> &str {
        self.str("theme.accent").filter(|a| ACCENTS.contains(a)).unwrap_or("ember")
    }
}

/// The registry's keys, which aren't settings.
const REGISTRY_KEYS: &[&str] = &["home", "current", "vault", "cache", "vaults"];

fn read_table(path: &Path) -> Result<Option<toml::Table>, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => s.parse::<toml::Table>().map(Some).map_err(|e| e.message().to_string()),
        Err(_) => Ok(None),
    }
}

fn allowed(dotted: &str) -> bool {
    ALLOWED.iter().any(|a| if a.ends_with('.') { dotted.starts_with(a) } else { dotted == *a || dotted.starts_with(&format!("{a}.")) })
}

/// Why a key is refused, in the notice's words.
fn refusal(dotted: &str) -> &'static str {
    let top = dotted.split('.').next().unwrap_or("");
    match top {
        "hooks" => "hooks aren't allowed in a vault",
        "policy" | "tiers" | "readonly" | "actor" | "me" => "trust settings aren't allowed in a vault",
        "daemon" | "update" | "vault" | "vaults" | "home" | "current" | "cache" => "machine settings aren't allowed in a vault",
        "agents" => "agent setup isn't allowed in a vault",
        _ => "not a setting a vault can change",
    }
}

/// Merge `src` into `dst` per key (tables merge, everything else replaces), recording the leaf
/// keys under `prefix`.
fn merge(dst: &mut toml::Table, src: &toml::Table, prefix: &str, source: &Source, file: &Path, layers: &mut BTreeMap<String, Vec<Entry>>) {
    for (k, v) in src {
        let dotted = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        match (dst.get_mut(k), v) {
            (Some(toml::Value::Table(d)), toml::Value::Table(s)) => merge(d, s, &dotted, source, file, layers),
            (_, toml::Value::Table(s)) => {
                let mut fresh = toml::Table::new();
                merge(&mut fresh, s, &dotted, source, file, layers);
                dst.insert(k.clone(), toml::Value::Table(fresh));
            }
            _ => {
                dst.insert(k.clone(), v.clone());
                layers.entry(dotted).or_default().push(Entry { value: v.clone(), source: source.clone(), file: file.to_path_buf() });
            }
        }
    }
}

/// Keep only what a vault may set (by leaf key), noting each refusal with its line.
fn whitelist(t: &toml::Table, prefix: &str, text: &str, vault: &str, file: &Path, notices: &mut Vec<String>) -> toml::Table {
    let mut out = toml::Table::new();
    for (k, v) in t {
        let dotted = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        match v {
            toml::Value::Table(sub) if !allowed(&dotted) && ALLOWED.iter().any(|a| a.starts_with(&format!("{dotted}."))) => {
                let kept = whitelist(sub, &dotted, text, vault, file, notices);
                if !kept.is_empty() {
                    out.insert(k.clone(), toml::Value::Table(kept));
                }
            }
            _ if allowed(&dotted) => {
                out.insert(k.clone(), v.clone());
            }
            _ => {
                let line = text.lines().position(|l| {
                    let l = l.trim_start();
                    l.starts_with(&format!("[{dotted}")) || l.starts_with(&format!("{k} ")) || l.starts_with(&format!("{k}="))
                });
                let at = line.map(|n| format!(" line {}", n + 1)).unwrap_or_default();
                notices.push(format!("vault {vault}: {}{at} ignored ({}) · thc config --effective", tilde(file), refusal(&dotted)));
            }
        }
    }
    out
}

/// The settings in effect for a vault (None: your config alone).
pub fn load(vault: Option<&Path>) -> Effective {
    let mut eff = Effective::default();
    let user_file = global_config_path();
    let user = user_file.as_deref().map(read_table).unwrap_or(Ok(None));
    let user = match user {
        Ok(t) => t.unwrap_or_default(),
        Err(e) => {
            if let Some(f) = &user_file {
                eff.notices.push(format!("{}: {e} · your settings are ignored until it parses", tilde(f)));
            }
            toml::Table::new()
        }
    };
    let mut mine = user.clone();
    for k in REGISTRY_KEYS {
        mine.remove(*k);
    }
    let uf = user_file.clone().unwrap_or_default();
    merge(&mut eff.table, &mine, "", &Source::You, &uf, &mut eff.layers);
    let Some(vault) = vault else { return eff };
    let reg = Registry::parse(&toml::to_string(&user).unwrap_or_default());
    let name = reg.name_for(vault);
    eff.vault_name = Some(name.clone());
    let vf = vault.join(VAULT_FILE);
    match read_table(&vf) {
        Ok(Some(t)) => {
            let text = std::fs::read_to_string(&vf).unwrap_or_default();
            let kept = whitelist(&t, "", &text, &name, &vf, &mut eff.notices);
            merge(&mut eff.table, &kept, "", &Source::Vault(name.clone()), &vf, &mut eff.layers);
        }
        Ok(None) => {}
        Err(e) => eff.notices.push(format!("vault {name}: {} ignored ({e}) · thc config --effective", tilde(&vf))),
    }
    if let Some(ov) = user.get("vaults").and_then(|v| v.get(&name)).and_then(|v| v.get("settings")).and_then(|v| v.as_table()) {
        merge(&mut eff.table, ov, "", &Source::Override(name.clone()), &uf, &mut eff.layers);
    }
    eff
}

static CURRENT: RwLock<Option<Arc<Effective>>> = RwLock::new(None);

/// Load the settings for the vault this process uses (and keep them for `current`).
pub fn init(vault: Option<&Path>) -> Arc<Effective> {
    let e = Arc::new(load(vault));
    if let Ok(mut c) = CURRENT.write() {
        *c = Some(e.clone());
    }
    e
}

/// The settings in effect: what `init` loaded, else your config alone.
pub fn current() -> Arc<Effective> {
    if let Some(e) = CURRENT.read().ok().and_then(|c| c.clone()) {
        return e;
    }
    init(None)
}

/// Where captures land when nothing names a place (`[capture] target = "¶ Issues"` in a vault's
/// settings.toml, vaults.md §9): that page's id and title, if it exists.
pub fn capture_target(eff: &Effective, store: &crate::store::Store) -> Option<(String, String)> {
    let raw = eff.str("capture.target")?.trim();
    let title = raw.trim_start_matches('¶').trim();
    if title.is_empty() {
        return None;
    }
    let id = store.find_root_by_title(title, false).ok().flatten()?;
    Some((id, title.to_string()))
}

/// The next accent for a new vault: the first of rose, sea, iris, graphite that no registered
/// vault wears yet (then round again).
pub fn next_accent(reg: &Registry) -> &'static str {
    let used: Vec<String> = reg
        .vaults
        .iter()
        .filter_map(|e| read_table(&e.path.join(VAULT_FILE)).ok().flatten())
        .filter_map(|t| t.get("theme").and_then(|t| t.get("accent")).and_then(|a| a.as_str()).map(str::to_string))
        .collect();
    let pool = &ACCENTS[1..];
    pool.iter().find(|a| !used.iter().any(|u| u == *a)).copied().unwrap_or(pool[reg.vaults.len() % pool.len()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_merge_per_key_and_vault_files_are_whitelisted() {
        let mut layers = BTreeMap::new();
        let mut t = toml::Table::new();
        let you: toml::Table = "[tui]\nleader_popup = \"immediate\"\n[tui.focus]\nwidth = 70\n".parse().unwrap();
        merge(&mut t, &you, "", &Source::You, Path::new("/u"), &mut layers);
        let text = "[tui.focus]\npreset = \"planner\"\n[hooks]\non_add = \"rm -rf /\"\n[theme]\naccent = \"sea\"\n";
        let v: toml::Table = text.parse().unwrap();
        let mut notices = vec![];
        let kept = whitelist(&v, "", text, "acme", Path::new("/v/settings.toml"), &mut notices);
        assert!(kept.get("hooks").is_none());
        assert_eq!(notices.len(), 1);
        assert!(notices[0].contains("line 3") && notices[0].contains("hooks aren't allowed"), "{notices:?}");
        merge(&mut t, &kept, "", &Source::Vault("acme".into()), Path::new("/v"), &mut layers);
        let e = Effective { table: t, layers, notices, vault_name: None };
        // The vault's preset joins your width: tables merge.
        assert_eq!(e.str("tui.focus.preset"), Some("planner"));
        assert_eq!(e.get("tui.focus.width").and_then(|v| v.as_integer()), Some(70));
        assert_eq!(e.accent(), "sea");
        assert_eq!(e.layers["tui.focus.preset"].last().unwrap().source, Source::Vault("acme".into()));
    }
}
