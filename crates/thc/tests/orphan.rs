//! thc always leaves when it should (0.9.48): a TUI whose terminal is gone without a
//! SIGHUP reaching it (its shell died first, the window was force-closed) once sat at 100% CPU
//! for 8 hours on a revoked tty, and a plain `kill` didn't stop it. Here SIGHUP is blocked
//! across exec and the pty is revoked (macOS revoke(2)), as the orphan's fds were; and a loop
//! that never comes back is simulated. Whatever the loop is doing, SIGTERM / SIGINT end thc
//! within 2 s, with the line being typed saved or kept in recovery.json.
#![cfg(target_os = "macos")]

mod common;

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

unsafe extern "C" {
    fn revoke(path: *const libc::c_char) -> libc::c_int;
}

struct T {
    root: PathBuf,
    guard: common::Guard,
    master: std::fs::File,
    slave: std::ffi::CString,
}

impl T {
    fn start(name: &str, env: &[(&str, &str)]) -> T {
        let root = thc_core::scratch::dir(&format!("thc-orphan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut init = common::thc();
        Self::env(&root, &mut init);
        assert!(init.args(["init", "v"]).output().unwrap().status.success());
        let (mut m, mut s) = (0, 0);
        let mut name = [0 as libc::c_char; 128];
        let mut ws = libc::winsize { ws_row: 30, ws_col: 100, ws_xpixel: 0, ws_ypixel: 0 };
        assert_eq!(unsafe { libc::openpty(&mut m, &mut s, name.as_mut_ptr(), std::ptr::null_mut(), &mut ws) }, 0);
        let slave_path = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }.to_owned();
        let slave = unsafe { OwnedFd::from_raw_fd(s) };
        let sfd = slave.as_raw_fd();
        let mut c = common::thc();
        Self::env(&root, &mut c);
        for (k, v) in env {
            c.env(k, v);
        }
        c.args(["j", "--no-focus"]).stdin(slave.try_clone().unwrap()).stdout(slave.try_clone().unwrap()).stderr(slave);
        unsafe {
            c.pre_exec(move || {
                libc::setsid();
                libc::ioctl(sfd, libc::TIOCSCTTY as _, 0);
                // No SIGHUP will reach it (blocked signals stay blocked across exec).
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGHUP);
                libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
                Ok(())
            });
        }
        let guard = common::Guard::spawn(&mut c);
        let master = unsafe { std::fs::File::from_raw_fd(m) };
        let (mut r, mut w) = (master.try_clone().unwrap(), master.try_clone().unwrap());
        std::thread::spawn(move || {
            let mut b = [0u8; 65536];
            while let Ok(n) = r.read(&mut b) {
                if n == 0 {
                    break;
                }
                // Answer the keyboard query as a kitty terminal, or startup waits for it.
                if b[..n].windows(4).any(|x| x == b"\x1b[?u") {
                    let _ = w.write_all(b"\x1b[?0u\x1b[?62;22c");
                }
            }
        });
        std::thread::sleep(Duration::from_millis(1500));
        T { root, guard, master, slave: slave_path }
    }

    fn env(root: &std::path::Path, c: &mut std::process::Command) {
        c.current_dir(root).env("THC_VAULT", root.join("v")).env("THC_CACHE_DIR", root.join("c")).env_remove("THC_ACTOR").env("TERM", "xterm-256color");
    }

    fn type_(&mut self, s: &str) {
        self.master.write_all(s.as_bytes()).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        assert!(self.guard.0.try_wait().unwrap().is_none(), "running before the test acts");
    }

    fn revoke(&self) {
        assert_eq!(unsafe { revoke(self.slave.as_ptr()) }, 0, "revoke: {}", std::io::Error::last_os_error());
    }

    fn signal(&self, sig: libc::c_int) {
        unsafe { libc::kill(self.guard.0.id() as i32, sig) };
    }

    /// Waits for the exit; how long it took.
    fn exits_within(&mut self, limit: Duration, what: &str) -> Duration {
        let t = Instant::now();
        while self.guard.0.try_wait().unwrap().is_none() {
            assert!(t.elapsed() < limit, "{what}: thc still running after {limit:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
        t.elapsed()
    }

    fn saved(&self, text: &str) -> bool {
        let mut q = common::thc();
        Self::env(&self.root, &mut q);
        String::from_utf8_lossy(&q.args(["--json", "q", &format!("text:{text}")]).output().unwrap().stdout).contains(text)
    }

    fn recovered(&self, text: &str) -> bool {
        std::fs::read_to_string(self.root.join("c/recovery.json")).is_ok_and(|s| s.contains(text))
    }
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_revoked_terminal_without_a_hangup_makes_thc_leave() {
    let mut t = T::start("revoked", &[]);
    t.type_("half a thought");
    t.revoke();
    t.exits_within(Duration::from_secs(4), "revoked tty, no signal");
    assert!(t.saved("half a thought"), "saved on the way out");
}

#[test]
fn sigterm_on_a_revoked_terminal_ends_thc_within_2s() {
    let mut t = T::start("revoked-term", &[]);
    t.type_("half a thought");
    t.revoke();
    t.signal(libc::SIGTERM);
    t.exits_within(Duration::from_millis(2500), "revoked tty, then SIGTERM");
    assert!(t.saved("half a thought") || t.recovered("half a thought"), "the line is saved or in recovery.json");
}

#[test]
fn a_wedged_loop_still_ends_on_sigterm_and_sigint_keeping_the_line() {
    for (name, sig) in [("wedged-term", libc::SIGTERM), ("wedged-int", libc::SIGINT)] {
        let mut t = T::start(name, &[("THC_TEST_WEDGE", "1")]);
        // `!` wedges the loop for good (the test hook), after the line is typed.
        t.type_("half a thought!");
        t.signal(sig);
        t.exits_within(Duration::from_millis(2500), name);
        assert!(t.recovered("half a thought"), "{name}: the unsaved line is in recovery.json");
    }
}
