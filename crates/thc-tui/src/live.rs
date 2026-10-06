//! Live connection to the daemon: subscribe to `changed` and `alerts`, reconnect every 10 s.
//! The TUI never claims alert delivery (daemon.md §2.1): it only shows a toast.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;
use thc_core::proto::{Client, Incoming};
use thc_core::vault::Paths;

pub enum LiveMsg {
    Connected { version_mismatch: bool },
    Disconnected,
    Event { event: String, data: serde_json::Value },
}

pub fn spawn(paths: Paths) -> Receiver<LiveMsg> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || run(paths, tx));
    rx
}

fn run(paths: Paths, tx: Sender<LiveMsg>) {
    let mut announced_offline = false;
    loop {
        if let Some(mut c) = Client::connect(&paths) {
            match c.hello("tui", &["changed", "alerts"], false) {
                Ok(hello) => {
                    announced_offline = false;
                    let mismatch = hello.get("version_mismatch").and_then(|v| v.as_bool()).unwrap_or(false);
                    if tx.send(LiveMsg::Connected { version_mismatch: mismatch }).is_err() {
                        return;
                    }
                    let _ = c.set_read_timeout(None);
                    loop {
                        match c.read() {
                            Ok(Some(Incoming::Event(e))) => {
                                if tx.send(LiveMsg::Event { event: e.event, data: e.data }).is_err() {
                                    return;
                                }
                            }
                            Ok(Some(Incoming::Response(_))) => {}
                            _ => break,
                        }
                    }
                }
                Err(_) => {}
            }
        }
        if !announced_offline {
            announced_offline = true;
            if tx.send(LiveMsg::Disconnected).is_err() {
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(10));
    }
}
