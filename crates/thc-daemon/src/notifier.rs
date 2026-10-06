//! OS notification channels for alerts no client claimed (daemon.md §2.6).
//!
//! | Platform | Channel | Click / actions | Withdraw |
//! |---|---|---|---|
//! | macOS + `terminal-notifier` | banner, grouped by alert id | opens `thc tui --focus <id>` in the terminal | `-remove <id>` |
//! | macOS otherwise | `osascript display notification` | none | not possible |
//! | Linux + `notify-send` | desktop notification | none | not possible |

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use thc_core::alerts::Delivery;

/// Deliveries made through the log channel (observable by tests).
pub static LOGGED: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Channel {
    /// Log the delivery only (tests, and when no OS channel exists).
    Log,
    TerminalNotifier(PathBuf),
    Osascript,
    NotifySend(PathBuf),
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let extra = ["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from);
    std::env::split_paths(&path).chain(extra).map(|d| d.join(name)).find(|p| p.is_file())
}

/// Best channel on this machine. `THC_NOTIFY=log|osascript|terminal-notifier|notify-send` overrides.
pub fn detect() -> Channel {
    match std::env::var("THC_NOTIFY").as_deref() {
        Ok("log") => return Channel::Log,
        Ok("osascript") => return Channel::Osascript,
        // Tests never put real notifications on the screen.
        _ if thc_core::sandbox::active() => return Channel::Log,
        _ => {}
    }
    if cfg!(target_os = "macos") {
        match which("terminal-notifier") {
            Some(p) => Channel::TerminalNotifier(p),
            None => Channel::Osascript,
        }
    } else {
        match which("notify-send") {
            Some(p) => Channel::NotifySend(p),
            None => Channel::Log,
        }
    }
}

/// (title, subtitle, message) for a delivery, in plain text.
pub fn texts(d: &Delivery) -> (String, String, String) {
    match d {
        Delivery::Single { notification: n } => (n.title.clone(), n.line1.clone(), n.line2_plain.clone()),
        Delivery::Summary { title, body, .. } => (title.clone(), String::new(), body.clone()),
        Delivery::Silent { .. } => (String::new(), String::new(), String::new()),
    }
}

/// Shell command that opens the TUI focused on a node in the user's terminal.
/// `THC_TERMINAL` (a command prefix, e.g. `wezterm start --`) overrides detection.
pub fn open_command(node: Option<&str>) -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "thc".into());
    let focus = node.map(|n| format!(" --focus {n}")).unwrap_or_default();
    let tui = format!("{exe} tui{focus}");
    if let Ok(prefix) = std::env::var("THC_TERMINAL") {
        return format!("{prefix} {tui}");
    }
    if let Some(w) = which("wezterm") {
        return format!("{} start -- {tui}", w.display());
    }
    if std::path::Path::new("/Applications/Ghostty.app").exists() {
        return format!("open -na Ghostty --args -e {tui}");
    }
    format!("osascript -e 'tell application \"Terminal\" to do script \"{tui}\"' -e 'tell application \"Terminal\" to activate'")
}

/// The argument list for terminal-notifier (also used by tests).
pub fn terminal_notifier_args(d: &Delivery) -> Vec<String> {
    let (title, subtitle, message) = texts(d);
    let (group, node) = match d {
        Delivery::Single { notification } => (notification.alert.clone(), Some(notification.node.clone())),
        Delivery::Summary { alerts, .. } => (alerts.first().cloned().unwrap_or_default(), None),
        Delivery::Silent { .. } => (String::new(), None),
    };
    let mut args = vec!["-title".into(), title, "-message".into(), if message.is_empty() { " ".into() } else { message }, "-group".into(), group];
    if !subtitle.is_empty() {
        args.push("-subtitle".into());
        args.push(subtitle);
    }
    args.push("-execute".into());
    args.push(open_command(node.as_deref()));
    args
}

fn applescript_quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

impl Channel {
    pub fn name(&self) -> &'static str {
        match self {
            Channel::Log => "log",
            Channel::TerminalNotifier(_) => "terminal-notifier",
            Channel::Osascript => "osascript",
            Channel::NotifySend(_) => "notify-send",
        }
    }

    /// Show a delivery; returns the channel name used.
    pub fn deliver(&self, d: &Delivery) -> String {
        if matches!(d, Delivery::Silent { .. }) {
            return "silent".into();
        }
        let ok = match self {
            Channel::Log => {
                LOGGED.lock().unwrap().extend(thc_core::alerts::delivery_alerts(d));
                true
            }
            Channel::TerminalNotifier(p) => run(Command::new(p).args(terminal_notifier_args(d))),
            Channel::Osascript => {
                let (title, subtitle, message) = texts(d);
                let mut script = format!("display notification \"{}\" with title \"{}\"", applescript_quote(&message), applescript_quote(&title));
                if !subtitle.is_empty() {
                    script.push_str(&format!(" subtitle \"{}\"", applescript_quote(&subtitle)));
                }
                run(thc_core::sandbox::tool("osascript").args(["-e", &script]))
            }
            Channel::NotifySend(p) => {
                let (title, subtitle, message) = texts(d);
                let body = [subtitle, message].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
                run(Command::new(p).args(["-a", "thc", &title, &body]))
            }
        };
        if ok { self.name().to_string() } else { format!("{} (failed)", self.name()) }
    }

    /// Withdraw a delivered notification (best effort; only terminal-notifier can).
    pub fn withdraw(&self, alert: &str) {
        if let Channel::TerminalNotifier(p) = self {
            let _ = run(Command::new(p).args(["-remove", alert]));
        }
    }
}

fn run(cmd: &mut Command) -> bool {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use thc_core::alerts::Notification;

    fn single() -> Delivery {
        Delivery::Single {
            notification: Notification {
                alert: "xr7pdaaaaaaa".into(),
                node: "jkrc1aaaaaaa".into(),
                title: "Pay rent".into(),
                line1: "Tue 09:00 · ↻ every month on the 1st".into(),
                line2: "in ¶ Home · set by ◆ claude".into(),
                line2_plain: "in Home · set by claude".into(),
                thread: "thc.alerts.2026-10-06".into(),
                time_sensitive: true,
                is_task: true,
                fire_at: "2026-10-06T09:00".into(),
            },
        }
    }

    #[test]
    fn terminal_notifier_args_group_by_alert_and_focus_the_node() {
        let a = terminal_notifier_args(&single());
        let get = |k: &str| a.iter().position(|x| x == k).map(|i| a[i + 1].clone()).unwrap();
        assert_eq!(get("-title"), "Pay rent");
        assert_eq!(get("-subtitle"), "Tue 09:00 · ↻ every month on the 1st");
        assert_eq!(get("-message"), "in Home · set by claude");
        assert_eq!(get("-group"), "xr7pdaaaaaaa");
        assert!(get("-execute").contains("tui --focus jkrc1aaaaaaa"));
    }

    #[test]
    fn summary_texts() {
        let d = Delivery::Summary { title: "3 reminders".into(), body: "Pay rent · Call the dentist · Water plants".into(), alerts: vec!["a".into()], missed: false, thread: "thc.alerts.2026-10-06".into() };
        assert_eq!(texts(&d), ("3 reminders".into(), String::new(), "Pay rent · Call the dentist · Water plants".into()));
    }
}
