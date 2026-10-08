//! The test sandbox: every `thc` a test runs gets a scratch HOME, XDG dirs, no app, no login
//! item, no update checks, log-only notifications and a PATH whose launchctl / open / osascript /
//! systemctl are shims that only log. With THC_TEST=1 the binary itself also refuses any real path
//! or tool (thc_core::sandbox), so a test that forgets this still can't reach the real machine.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// This test process's scratch root (created once): home/, cache/, config/, shims/.
pub fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let r = thc_core::scratch::dir(&format!("thc-sandbox-{}", std::process::id()));
        for d in ["home", "cache", "config", "shims"] {
            std::fs::create_dir_all(r.join(d)).unwrap();
        }
        for tool in ["launchctl", "open", "osascript", "systemctl", "terminal-notifier"] {
            let p = r.join("shims").join(tool);
            std::fs::write(&p, format!("#!/bin/sh\necho \"{tool} $*\" >> \"{}\"\nexit 0\n", r.join("shims.log").display())).unwrap();
            Command::new("chmod").arg("+x").arg(&p).status().unwrap();
        }
        // The clipboard is a file here (clipboard()): no test ever reads or writes the real one.
        // A test that runs beside others gives its thc its own (THC_TEST_CLIPBOARD, see
        // clipboard_at): two pty tests sharing this one clobbered each other under load.
        let clip = r.join("clipboard.txt");
        for (tool, body) in [("pbcopy", "cat > \"$c\""), ("wl-copy", "cat > \"$c\""), ("pbpaste", "cat \"$c\" 2>/dev/null"), ("wl-paste", "cat \"$c\" 2>/dev/null"), ("xclip", "case \"$*\" in *-o*) cat \"$c\" 2>/dev/null;; *) cat > \"$c\";; esac")] {
            let p = r.join("shims").join(tool);
            std::fs::write(&p, format!("#!/bin/sh\nc=\"${{THC_TEST_CLIPBOARD:-{}}}\"\n{body}\n", clip.display())).unwrap();
            Command::new("chmod").arg("+x").arg(&p).status().unwrap();
        }
        r
    })
}

/// Put a command in the sandbox. Set a test's own HOME / THC_* after this: later wins.
pub fn sandbox(c: &mut Command) -> &mut Command {
    let r = root();
    let path = format!("{}:{}", r.join("shims").display(), std::env::var("PATH").unwrap_or_default());
    c.env("THC_TEST", "1")
        .env("THC_TEST_ROOT", r)
        .env("HOME", r.join("home"))
        .env("XDG_CACHE_HOME", r.join("cache"))
        .env("XDG_CONFIG_HOME", r.join("config"))
        .env("THC_SETUP_APP", "none")
        .env("THC_SETUP_LOGIN", "skip")
        .env("THC_NO_UPDATE_CHECK", "1")
        .env("THC_NOTIFY", "log")
        // The binary exits when this test process is gone (src/main.rs: test_watchdog), so no
        // child can outlive its run, even when the runner itself is killed.
        .env("THC_TEST_PARENT", std::process::id().to_string())
        .env("PATH", path)
        // Never the developer's editor: a test that opens one sets its own.
        .env_remove("VISUAL")
        .env_remove("EDITOR")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env_remove("THC_BOARD")
        .env_remove("THC_ROLE")
        .env_remove("THC_SESSION_ID")
        .env_remove("THC_CONFIG_DIR")
}

/// The thc under test, sandboxed.
pub fn thc() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_thc"));
    sandbox(&mut c);
    c
}

/// What a thc run with `THC_TEST_CLIPBOARD=<file>` last copied.
pub fn clipboard_at(file: &Path) -> String {
    std::fs::read_to_string(file).unwrap_or_default()
}

/// What a sandboxed thc last copied (the pbcopy shim's file).
pub fn clipboard() -> String {
    std::fs::read_to_string(root().join("clipboard.txt")).unwrap_or_default()
}

/// What the shims were asked to do (a test can assert a real tool would have been called).
pub fn shim_log() -> String {
    std::fs::read_to_string(root().join("shims.log")).unwrap_or_default()
}

/// A long-lived child (a daemon, a TUI on a pty) in its own process group, killed with its whole
/// group when this guard drops: at the end of the test, and on a failing assert (panics unwind).
pub struct Guard(pub std::process::Child);

impl Guard {
    pub fn spawn(c: &mut Command) -> Guard {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
        Guard(c.spawn().unwrap())
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            // SAFETY: kill(2) on our own child's group.
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            let _ = self.0.wait();
        }
    }
}
