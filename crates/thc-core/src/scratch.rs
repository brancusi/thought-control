//! Scratch paths a process makes in the temp dir (a vault's scratch copy, a test's sandbox,
//! a socket) go when the process does: on exit (a normal return, `std::process::exit`, a panic
//! that ends `main`) every registered path is removed. Without it, 25,000 of them piled up in
//! TMPDIR and made every mkdir there slow. A kill -9 still leaves its paths behind; whoever
//! makes them in a shared place prunes the dead ones (see `thc_tui::ui_server`).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once};

static PATHS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static HOOK: Once = Once::new();

extern "C" fn at_exit() {
    remove_all();
}

/// Remove `path` (a directory, a file or a socket) when this process exits.
pub fn remove_at_exit(path: &Path) {
    HOOK.call_once(|| unsafe {
        libc::atexit(at_exit);
    });
    if let Ok(mut v) = PATHS.lock() {
        if !v.iter().any(|p| p == path) {
            v.push(path.to_path_buf());
        }
    }
}

/// `temp_dir()/name`, removed when this process exits (not created: the caller does).
pub fn dir(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(name);
    remove_at_exit(&p);
    p
}

/// Remove every registered path now (what exit does).
pub fn remove_all() {
    let paths = match PATHS.lock() {
        Ok(mut v) => std::mem::take(&mut *v),
        Err(_) => return,
    };
    for p in paths {
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.is_dir() => {
                let _ = std::fs::remove_dir_all(&p);
            }
            Ok(_) => {
                let _ = std::fs::remove_file(&p);
            }
            Err(_) => {}
        }
    }
}

/// A scratch directory removed when dropped (and at exit, should it never be).
pub struct TempDir(PathBuf);

impl TempDir {
    /// `temp_dir()/<prefix>-<pid>-<unique>`, created.
    pub fn new(prefix: &str) -> std::io::Result<TempDir> {
        let p = std::env::temp_dir().join(format!("{prefix}-{}-{}", std::process::id(), crate::id::new_id()));
        std::fs::create_dir_all(&p)?;
        remove_at_exit(&p);
        Ok(TempDir(p))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_temp_dir_goes_when_dropped_and_a_scratch_dir_is_registered() {
        let t = TempDir::new("thc-scratch-test").unwrap();
        let p = t.path().to_path_buf();
        std::fs::write(p.join("f"), "x").unwrap();
        drop(t);
        assert!(!p.exists());
        // (Not remove_all here: other tests' scratch paths are registered too.)
        let d = dir(&format!("thc-scratch-test-reg-{}", crate::id::new_id()));
        assert!(PATHS.lock().unwrap().contains(&d));
    }
}
