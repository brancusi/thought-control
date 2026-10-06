//! Test runs never reach the real machine. With `THC_TEST=1` (set for every `cargo test` by
//! .cargo/config.toml, and by the app's checks), any path thc resolves under the real home
//! directory or /Applications, and any real launchctl / open / osascript, stops the process at
//! once with exit 70. An abort, not an error: nothing can swallow it with `let _ =`.
//!
//! Twice a test escaped into the developer's environment (an app helper reading the real home;
//! a migration test unregistering the real app's login item). This is the systemic guard.

use std::path::{Path, PathBuf};

pub fn active() -> bool {
    std::env::var("THC_TEST").is_ok_and(|v| v == "1")
}

/// The real home directory from the password database: $HOME can be pointed anywhere.
pub fn real_home() -> Option<PathBuf> {
    // SAFETY: getpwuid returns a pointer into static storage; we copy the string out at once.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        let dir = std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned();
        Some(PathBuf::from(dir))
    }
}

fn abort(what: &str) -> ! {
    eprintln!("\nTHC_TEST: refusing to touch the real machine: {what}\n(tests run under a scratch HOME; see crates/thc/tests/common)\n");
    std::process::exit(70);
}

/// A path thc is about to read or write. Under THC_TEST it must be outside the real home and
/// /Applications. Returns the path for chaining.
pub fn check(p: &Path) -> &Path {
    if active() {
        if p.starts_with("/Applications") || p.starts_with("/Library/LaunchAgents") || p.starts_with("/Library/LaunchDaemons") {
            abort(&format!("{} is a real system location", p.display()));
        }
        if let Some(home) = real_home() {
            if p.starts_with(&home) && !allowed_in_home(p, &home) {
                abort(&format!("{} is inside the real home {}", p.display(), home.display()));
            }
        }
    }
    p
}

/// The repo and build outputs can live under the real home (a checkout in ~/code): paths inside
/// THC_TEST_ROOT (the scratch root) or the cargo target directory are fine.
fn allowed_in_home(p: &Path, _home: &Path) -> bool {
    let roots = ["THC_TEST_ROOT", "CARGO_TARGET_TMPDIR", "THC_TEST_ALLOW"];
    roots.iter().filter_map(|k| std::env::var_os(k)).any(|r| p.starts_with(Path::new(&r)))
}

/// A system tool thc runs (launchctl, open, osascript). Under THC_TEST it must resolve to a shim
/// (PATH), never the real one in /bin or /usr/bin.
pub fn tool(name: &str) -> std::process::Command {
    if active() {
        let found = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|c| c.is_file()))
            .flatten();
        match found {
            Some(f) if !(f.starts_with("/bin") || f.starts_with("/usr/bin") || f.starts_with("/sbin") || f.starts_with("/usr/sbin")) => {
                return std::process::Command::new(f);
            }
            _ => abort(&format!("the real {name}")),
        }
    }
    std::process::Command::new(name)
}
