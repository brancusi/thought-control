//! What the last release check found (`thc update`, the daemon's 6-hourly check): read by
//! `thc today` and the TUI to show `update 0.7.1 · :update` without touching the network.

use serde_json::Value;
use std::path::PathBuf;

/// `0.7.1` → (0, 7, 1); a pre-release suffix (`-dev`) is ignored.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next().flatten().unwrap_or(0)))
}

pub fn newer(candidate: &str, than: &str) -> bool {
    matches!((parse_version(candidate), parse_version(than)), (Some(a), Some(b)) if a > b)
}

/// `~/.cache/thc/update.json` (or under $XDG_CACHE_HOME): `{checked, latest, notes}`.
pub fn check_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    let p = base.join("thc").join("update.json");
    crate::sandbox::check(&p);
    Some(p)
}

/// A version newer than `current` recorded by the last check, if any.
pub fn available(current: &str) -> Option<String> {
    let v: Value = serde_json::from_slice(&std::fs::read(check_file()?).ok()?).ok()?;
    let latest = v["latest"].as_str()?;
    newer(latest, current).then(|| latest.to_string())
}

/// `update = "auto" | "notify" | "off"` from ~/.config/thought/config.toml (default notify).
pub fn mode() -> String {
    crate::vault::global_config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| s.parse::<toml::Table>().ok())
        .and_then(|t| t.get("update").and_then(|v| v.as_str()).map(str::to_string))
        .filter(|m| ["auto", "notify", "off"].contains(&m.as_str()))
        .unwrap_or_else(|| "notify".into())
}

/// Automatic checks: never in CI, with THC_NO_UPDATE_CHECK, or with `update = "off"`.
pub fn checks_allowed() -> bool {
    std::env::var_os("THC_NO_UPDATE_CHECK").is_none() && std::env::var_os("CI").is_none() && !crate::sandbox::active() && mode() != "off"
}
