//! Minute usage polling, off the daemon's socket and replay threads.
use crate::Shared;
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
use thc_core::token_usage;

pub fn start(shared: Arc<Shared>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let interval = if thc_core::sandbox::active() {
            std::env::var("THC_TEST_TOKEN_POLL_MS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .map(|n| Duration::from_millis(n.max(100)))
                .unwrap_or(Duration::from_secs(60))
        } else {
            Duration::from_secs(60)
        };
        let mut next = Instant::now();
        while !shared.shutdown.load(Ordering::SeqCst) {
            let periodic = Instant::now() >= next;
            if shared.tokens_dirty.swap(false, Ordering::SeqCst) || periodic {
                if periodic {
                    next = Instant::now() + interval;
                }
                if let Err(e) = poll(&shared, &home, periodic) {
                    shared.log("error", &format!("token collection: {e:#}"));
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    })
}

fn poll(shared: &Shared, home: &std::path::Path, periodic: bool) -> anyhow::Result<()> {
    let inputs = {
        let v = shared.vault.lock().unwrap();
        token_usage::pending(&v.store)?.into_iter().filter(|i| periodic || token_usage::cached(&v.store, &i.id).ok().flatten().is_none()).collect::<Vec<_>>()
    };
    let mut changed = Vec::new();
    for input in inputs {
        if shared.shutdown.load(Ordering::SeqCst) {
            break;
        }
        let now = chrono::Utc::now().timestamp_millis();
        // Parsing potentially large session logs must not hold up RPC or vault replay.
        let Some(tokens) = token_usage::collect(home, &input, now) else { continue };
        if token_usage::apply(&mut shared.vault.lock().unwrap(), &input, &tokens, now)? {
            changed.push(input.id);
        }
    }
    if !changed.is_empty() {
        shared.broadcast("tokens", "tokens", serde_json::json!({ "ids": changed }));
        shared.dirty.store(true, Ordering::SeqCst);
    }
    Ok(())
}
