//! `thc update`: fetch the signed manifest, verify, swap the binary in
//! place, restart the daemon on it. `--rollback` puts `thc.prev` back. Nothing here needs a vault.
//!
//! Trust: every target in `latest.json` carries an ed25519 signature over
//! `thc <version> <target> <sha256>`, checked against the public key built into this binary, so a
//! manifest can't point an old (signed) build at a newer version. The tarball must match the
//! sha256 and the new binary must report that version.

use anyhow::{Context, Result, anyhow, bail};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::out::Out;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where the manifest lives: the releases repo's latest release (CLI releases own `latest`; a
/// resumed Mac app would move to `app-v*` tags with its own feed release).
pub const MANIFEST_URL: &str = "https://github.com/brancusi/thought-control-releases/releases/latest/download/latest.json";

/// The update signing key's public half (hex). All zeros until the release key is made: such a
/// build refuses to update rather than trust anything.
const PUBLIC_KEY: &str = include_str!("../update_key.pub");

/// This build's target triple, as release artifacts are named.
pub fn target() -> &'static str {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("aarch64", "macos") => "aarch64-apple-darwin",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu",
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu",
        _ => "unknown",
    }
}

fn manifest_url() -> String {
    std::env::var("THC_UPDATE_URL").unwrap_or_else(|_| MANIFEST_URL.into())
}

fn public_key() -> Result<VerifyingKey> {
    // Tests sign with their own key; only debug builds read it from the environment.
    #[cfg(debug_assertions)]
    let hex = std::env::var("THC_UPDATE_PUBKEY").unwrap_or_else(|_| PUBLIC_KEY.trim().to_string());
    #[cfg(not(debug_assertions))]
    let hex = PUBLIC_KEY.trim().to_string();
    let bytes = unhex(&hex).ok_or_else(|| anyhow!("the update key in this build is malformed"))?;
    if bytes.iter().all(|b| *b == 0) {
        bail!("this build has no update key, so it can't verify updates · install from the releases page");
    }
    let arr: [u8; 32] = bytes.try_into().map_err(|_| anyhow!("the update key must be 32 bytes"))?;
    Ok(VerifyingKey::from_bytes(&arr)?)
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub use thc_core::release::newer;

/// The message a target's signature covers.
pub fn signed_message(version: &str, target: &str, sha256: &str) -> String {
    format!("thc {version} {target} {sha256}")
}

/// Fetch a URL (https or file) with the system curl: no HTTP stack in thc.
fn fetch(url: &str, to: Option<&Path>) -> Result<Vec<u8>> {
    let mut c = Command::new("curl");
    c.args(["-fsSL", "--retry", "2", "--max-time", "120"]);
    if let Some(p) = to {
        c.arg("-o").arg(p);
    }
    let out = c.arg(url).output().context("running curl")?;
    if !out.status.success() {
        bail!("couldn't fetch {url}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

/// The latest release as the manifest describes it, for this target.
#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub url: String,
    pub sha256: String,
    pub sig: String,
}

pub fn latest() -> Result<Release> {
    let raw = fetch(&manifest_url(), None)?;
    let m: Value = serde_json::from_slice(&raw).context("the update manifest isn't JSON")?;
    let version = m["version"].as_str().ok_or_else(|| anyhow!("the manifest has no version"))?.to_string();
    let t = &m["targets"][target()];
    if t.is_null() {
        bail!("thc {version} has no build for {}", target());
    }
    let field = |k: &str| t[k].as_str().map(str::to_string).ok_or_else(|| anyhow!("the manifest's {} entry has no {k}", target()));
    Ok(Release { version, notes: m["notes"].as_str().unwrap_or("").to_string(), url: field("url")?, sha256: field("sha256")?, sig: field("sig")? })
}

/// The installed binary to replace, refusing a link into Thought Central.app (that copy belongs
/// to the app: `thc setup --migrate-from-app` makes a standalone one first).
pub fn installed_path() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    thc_core::sandbox::check(&exe);
    if exe.to_string_lossy().contains(".app/Contents/") {
        bail!("this thc lives inside Thought Central.app · run `thc setup --migrate-from-app` to install a standalone thc first");
    }
    // Run as thc.prev (a rollback): the binary is its sibling `thc`.
    if exe.file_name().is_some_and(|n| n == "thc.prev") {
        return Ok(exe.with_file_name("thc"));
    }
    Ok(exe)
}

/// Download, verify and stage `r` next to `dest`; returns the staged binary (same filesystem, so
/// the swap is a rename).
pub fn stage(r: &Release, dest: &Path) -> Result<(PathBuf, PathBuf)> {
    let key = public_key()?;
    let sig = Signature::from_slice(&unhex(&r.sig).ok_or_else(|| anyhow!("the signature isn't hex"))?)?;
    key.verify(signed_message(&r.version, target(), &r.sha256).as_bytes(), &sig)
        .map_err(|_| anyhow!("the update's signature doesn't verify · not installed"))?;
    let dir = dest.parent().ok_or_else(|| anyhow!("no directory for {}", dest.display()))?;
    let tmp = dir.join(format!(".thc-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let tarball = tmp.join("thc.tar.gz");
    fetch(&r.url, Some(&tarball))?;
    let got = hex(&Sha256::digest(std::fs::read(&tarball)?));
    if got != r.sha256 {
        let _ = std::fs::remove_dir_all(&tmp);
        bail!("the download's sha256 is {got}, the manifest says {} · not installed", r.sha256);
    }
    let x = Command::new("tar").arg("-xzf").arg(&tarball).arg("-C").arg(&tmp).output()?;
    if !x.status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        bail!("couldn't unpack the update: {}", String::from_utf8_lossy(&x.stderr).trim());
    }
    let bin = tmp.join("thc");
    if !bin.is_file() {
        let _ = std::fs::remove_dir_all(&tmp);
        bail!("the update has no thc binary");
    }
    let v = Command::new(&bin).arg("--version").output().context("running the new thc")?;
    let says = String::from_utf8_lossy(&v.stdout).trim().to_string();
    if says != format!("thc {}", r.version) {
        let _ = std::fs::remove_dir_all(&tmp);
        bail!("the new binary says {says:?}, expected thc {} · not installed", r.version);
    }
    std::fs::File::open(&bin)?.sync_all()?;
    Ok((bin, tmp))
}

/// CI: every target in a manifest verifies with the built-in key and matches its tarball.
pub fn verify_manifest(manifest: &Path, dir: &Path) -> Result<()> {
    let key = public_key()?;
    let m: Value = serde_json::from_slice(&std::fs::read(manifest)?)?;
    let version = m["version"].as_str().ok_or_else(|| anyhow!("no version"))?;
    let targets = m["targets"].as_object().ok_or_else(|| anyhow!("no targets"))?;
    for (t, e) in targets {
        let sha = e["sha256"].as_str().unwrap_or("");
        let sig = Signature::from_slice(&unhex(e["sig"].as_str().unwrap_or("")).ok_or_else(|| anyhow!("{t}: signature isn't hex"))?)?;
        key.verify(signed_message(version, t, sha).as_bytes(), &sig).map_err(|_| anyhow!("{t}: signature doesn't verify with the built-in key"))?;
        let file = e["url"].as_str().and_then(|u| u.rsplit('/').next()).ok_or_else(|| anyhow!("{t}: no url"))?;
        // install.sh has only its own target's tarball: the others are checked by signature.
        let Ok(bytes) = std::fs::read(dir.join(file)) else {
            println!("ok {t} (signature)");
            continue;
        };
        let got = hex(&Sha256::digest(bytes));
        if got != sha {
            bail!("{t}: {file} is {got}, the manifest says {sha}");
        }
        println!("ok {t} {file}");
    }
    Ok(())
}

/// Swap: keep the current binary as `thc.prev`, rename the staged one into place.
pub fn swap(staged: &Path, dest: &Path) -> Result<()> {
    let prev = dest.with_file_name("thc.prev");
    if dest.exists() {
        std::fs::copy(dest, &prev).with_context(|| format!("keeping {}", prev.display()))?;
    }
    std::fs::rename(staged, dest).with_context(|| format!("replacing {}", dest.display()))?;
    if let Some(dir) = dest.parent() {
        let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
    }
    Ok(())
}

/// After a swap, the binary now at `bin` finishes the update with its own logic: it restarts
/// the daemon and refreshes the agent files (`thc update --finish <version>`), so a fix to how
/// that's done ships with the version that has it. A binary without `--finish` (a rollback to
/// an older one) gets this binary's own steps instead.
fn finish_with(bin: &Path, to: &str) -> (Option<String>, Option<String>) {
    let o = Command::new(bin).args(["update", "--finish", to]).current_dir("/").stdin(std::process::Stdio::null()).output();
    if let Some(v) = o.ok().filter(|o| o.status.success()).and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok()) {
        return (v["daemon"].as_str().map(str::to_string), v["agents"].as_str().map(str::to_string));
    }
    (restart_daemon(bin, to), refresh_agents(bin))
}

/// Restart the running daemon on the binary at `bin` and check it came back on `expect`.
/// The daemon is the login one (its vault from the LaunchAgent), never whatever vault the
/// caller's directory resolves to (a .thc.toml there once made update skip the restart).
fn restart_daemon(bin: &Path, expect: &str) -> Option<String> {
    let cmd = |args: &[&str]| {
        let mut c = Command::new(bin);
        c.args(args).current_dir("/").stdin(std::process::Stdio::null());
        for k in ["THC_VAULT", "THC_CACHE_DIR"] {
            if let Some(v) = crate::daemon_cmd::login_env(k) {
                c.env(k, v);
            }
        }
        c
    };
    let status = || cmd(&["--json", "daemon", "status"]).output().ok().and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok());
    if status()?["state"] != "live" {
        return None;
    }
    let r = cmd(&["daemon", "restart"]).output().ok()?;
    if !r.status.success() {
        return Some(format!("! daemon restart failed: {} · thc daemon restart", String::from_utf8_lossy(&r.stderr).trim()));
    }
    let expect = expect.trim_start_matches("thc ").trim();
    Some(match status() {
        Some(v) if v["version"].as_str() == Some(expect) => "daemon restarted on the new thc".into(),
        Some(v) if v["state"] == "live" => format!("! the daemon still runs thc {} · thc daemon restart", v["version"].as_str().unwrap_or("?")),
        _ => "! the daemon didn't come back · thc daemon start".into(),
    })
}

/// The new binary's restart (`update --finish`): the login daemon (its vault from the login
/// item, never whatever the caller's directory resolves to) restarts through whatever runs it,
/// launchd, systemd or a plain process, and must come back on `expect`. None when no daemon runs.
fn finish_daemon(me: &Path, expect: &str) -> Option<String> {
    // A .thc.toml in the caller's directory once made update skip the restart.
    let _ = std::env::set_current_dir("/");
    let paths = crate::daemon_cmd::login_paths().or_else(|| thc_core::vault::Paths::resolve(std::env::var_os("THC_VAULT").map(PathBuf::from).as_deref()).ok())?;
    let expect = expect.trim_start_matches("thc ").trim();
    let done = crate::daemon_cmd::ensure(&paths, me, true);
    // Other vaults' own daemons on the replaced binary restart too (their lines aren't part of
    // the summary the old binary prints; doctor reports any left).
    let _ = crate::daemon_cmd::ensure_registered(me, Some(&paths));
    Some(match done {
        Ok(e) if e.action == "offline" => return None,
        Ok(e) => match &e.after {
            Some(v) if v["version"].as_str() == Some(expect) => "daemon restarted on the new thc".into(),
            Some(v) => format!("! the daemon still runs thc {} · thc daemon restart", v["version"].as_str().unwrap_or("?")),
            None => "! the daemon didn't come back · thc daemon start".into(),
        },
        Err(e) => format!("! daemon restart failed: {e:#} · thc daemon restart"),
    })
}

pub use thc_core::release::check_file;

/// The agent files setup installed carry the version: refresh them with the new binary, for
/// the agents setup recorded only (never adds one).
fn refresh_agents(bin: &Path) -> Option<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let rec: Value = serde_json::from_slice(&std::fs::read(home.join(".config/thought/setup.json")).ok()?).ok()?;
    let mut agents: Vec<String> = rec["items"].as_array()?.iter().filter(|i| i["piece"] == "agent").filter_map(|i| i["agent"].as_str().map(str::to_string)).collect();
    agents.sort();
    agents.dedup();
    if agents.is_empty() {
        return None;
    }
    // The vault step also adds any settings block a new version brings ([tui]); it never
    // changes existing settings.
    let _ = Command::new(bin).args(["setup", "--step", "vault"]).stdin(std::process::Stdio::null()).output();
    let o = Command::new(bin).args(["setup", "--step", "agents", "--agents", &agents.join(",")]).stdin(std::process::Stdio::null()).output().ok()?;
    Some(if o.status.success() { format!("agent files refreshed ({})", agents.join(", ")) } else { "agent files not refreshed · thc setup --step agents".into() })
}

/// Record a check: `{checked, latest, notes}`.
pub fn record(r: &Release) {
    if let Some(p) = check_file() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, json!({ "checked": chrono::Local::now().to_rfc3339(), "latest": r.version, "notes": r.notes }).to_string());
    }
}

pub fn run(out: &mut Out, check: bool, rollback: bool, finish: Option<&str>) -> Result<()> {
    // The new binary's half of an update (see finish_with): always JSON, for the old binary.
    if let Some(to) = finish {
        let me = std::env::current_exe()?;
        let daemon = finish_daemon(&me, to);
        let agents = refresh_agents(&me);
        println!("{}", json!({ "daemon": daemon, "agents": agents }));
        return Ok(());
    }
    if rollback {
        let dest = installed_path()?;
        let prev = dest.with_file_name("thc.prev");
        if !prev.is_file() {
            bail!("nothing to roll back to ({} doesn't exist)", prev.display());
        }
        let tmp = dest.with_file_name(".thc-rollback");
        std::fs::rename(&dest, &tmp)?;
        std::fs::rename(&prev, &dest)?;
        std::fs::rename(&tmp, &prev)?;
        let v = Command::new(&dest).arg("--version").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let (daemon, _) = finish_with(&dest, &v);
        if out.json {
            out.json(&json!({ "ok": true, "rolled_back_to": v, "daemon": daemon }));
        } else {
            out.line(format!("rolled back · now {v}{}", daemon.map(|d| format!(" · {d}")).unwrap_or_default()));
        }
        return Ok(());
    }
    let r = latest()?;
    record(&r);
    let is_newer = newer(&r.version, VERSION);
    if check || !is_newer {
        if out.json {
            out.json(&json!({ "current": VERSION, "latest": r.version, "available": is_newer, "notes": r.notes }));
        } else if is_newer {
            out.line(format!("update {} available (this is {VERSION}) · thc update", r.version));
        } else {
            out.line(format!("thc is up to date ({VERSION})"));
        }
        return Ok(());
    }
    let dest = installed_path()?;
    let (bin, tmp) = stage(&r, &dest)?;
    let swapped = swap(&bin, &dest);
    let _ = std::fs::remove_dir_all(&tmp);
    swapped?;
    let (daemon, agents) = finish_with(&dest, &r.version);
    if out.json {
        out.json(&json!({ "ok": true, "from": VERSION, "to": r.version, "path": dest, "notes": r.notes, "daemon": daemon, "agents": agents }));
    } else {
        out.line(format!("updated thc {VERSION} → {} · {}", r.version, dest.display()));
        if let Some(d) = daemon {
            out.line(d);
        }
        if let Some(a) = &agents {
            out.line(a.clone());
        }
        if !r.notes.trim().is_empty() {
            out.line(String::new());
            out.line(r.notes.trim().to_string());
        }
        out.line(out.dim("thc update --rollback puts the previous version back"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use thc_core::release::parse_version;

    #[test]
    fn versions_compare() {
        assert!(newer("0.7.1", "0.7.0"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(!newer("0.7.0", "0.7.0"));
        assert!(!newer("0.6.9", "0.7.0"));
        assert!(newer("v1.0.0", "0.99.0-dev"));
        assert_eq!(parse_version("0.7"), Some((0, 7, 0)));
    }
}
