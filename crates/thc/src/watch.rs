//! Read-only team subscription, with reconciliation across daemon outages.
use crate::{Ctx, out::node_json};
use anyhow::Result;
use clap::Args;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    io::{Read, Write},
    os::unix::net::UnixStream,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use thc_core::{
    dates,
    messages::{self, Recipient},
    model::Node,
    proto::{self, Incoming, Request},
};

pub const SPEC: crate::registry::Spec = crate::spec!("watch", "Stream addressed messages and newly ready routed tasks (read-only)", WatchArgs, run, board: true);

#[derive(Args, Debug)]
pub struct WatchArgs {
    /// Watch messages and unowned ready tasks for this role or actor.
    #[arg(long = "for", value_name = "ROLE|ACTOR")]
    pub recipient: String,
    /// Exit after printing the first matching item, including existing unread/ready work.
    #[arg(long)]
    pub once: bool,
}

static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// Restore the process's handlers if this command returns (notably with --once).
struct Signals(libc::sighandler_t, libc::sighandler_t);
impl Signals {
    fn install() -> Self {
        STOP.store(false, Ordering::SeqCst);
        // SAFETY: the handler only stores to a lock-free atomic; no allocation or I/O.
        unsafe {
            Self(libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t), libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t))
        }
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        // SAFETY: restore the handlers returned by signal(2).
        unsafe {
            libc::signal(libc::SIGINT, self.0);
            libc::signal(libc::SIGTERM, self.1);
        }
    }
}

/// Keep partial JSON lines across read timeouts. EOF must differ from a quiet connection,
/// which the general-purpose RPC client's read() intentionally treats alike.
struct Subscription {
    stream: UnixStream,
    bytes: Vec<u8>,
}
enum Readiness {
    Message(Incoming),
    Quiet,
    Closed,
}
impl Subscription {
    fn connect(paths: &thc_core::vault::Paths) -> Option<Self> {
        let mut stream = UnixStream::connect(proto::socket_path(paths)).ok()?;
        stream.set_read_timeout(Some(Duration::from_millis(200))).ok()?;
        stream.set_write_timeout(Some(Duration::from_millis(200))).ok()?;
        let request = Request {
            id: 1,
            method: "hello".into(),
            params: json!({
                "client": "thc-watch", "version": proto::VERSION, "proto": proto::PROTO_VERSION,
                "topics": ["changed"], "deliver": false
            }),
        };
        writeln!(stream, "{}", serde_json::to_string(&request).ok()?).ok()?;
        Some(Self { stream, bytes: Vec::new() })
    }
    fn read(&mut self) -> Result<Readiness> {
        loop {
            if let Some(end) = self.bytes.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = self.bytes.drain(..=end).collect();
                return Ok(Readiness::Message(serde_json::from_slice(&line)?));
            }
            let mut chunk = [0; 8192];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Ok(Readiness::Closed),
                Ok(n) => self.bytes.extend_from_slice(&chunk[..n]),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {
                    return Ok(Readiness::Quiet);
                }
                Err(e) => return Err(e.into()),
            }
            if STOP.load(Ordering::SeqCst) {
                return Ok(Readiness::Quiet);
            }
        }
    }
}

#[derive(Default)]
struct Seen {
    messages: HashSet<String>,
    ready: HashSet<String>,
}

fn emit(ctx: &mut Ctx, event: &str, node: &Node, source: &str, change: Option<&Value>) {
    if ctx.out.json {
        let mut item = json!({ "event": event, "node": node_json(ctx.store(), node), "board": ctx.board, "source": source });
        if let Some(change) = change {
            item["change"] = change.clone();
        }
        ctx.out.json(&item);
    } else {
        // Each item occupies exactly one line, including multiline messages.
        ctx.out.line(format!("{event} {} {}", ctx.store().short(&node.id), node.text.split_whitespace().collect::<Vec<_>>().join(" ")));
    }
    ctx.out.flush();
}

fn reconcile(ctx: &mut Ctx, recipient: &Recipient, seen: &mut Seen, once: bool, source: &str, change: Option<&Value>) -> Result<bool> {
    ctx.vault.catch_up()?;
    ctx.out.today = dates::today();
    let root = ctx.board.as_ref().and_then(|b| b.page.as_ref()).map(|p| p.id.clone());
    // Like msgs/prime, messages span the board vault, including its loose ¶ Messages page.
    // Only the task queue is scoped to the configured capture page.
    for node in messages::query(ctx.store(), "is:unread sort:order", ctx.out.today, i64::MAX as usize, recipient)? {
        if STOP.load(Ordering::SeqCst) {
            return Ok(true);
        }
        if seen.messages.insert(node.id.clone()) {
            emit(ctx, "message", &node, source, change);
            if once {
                return Ok(true);
            }
        }
    }
    // A blocker, ancestor move or owner change can affect ids other than those in the push.
    // Re-run the queue, rather than assuming the pushed ids are the entire affected set.
    let mut ready = HashSet::new();
    if let Some(role) = recipient.role.as_deref() {
        let route = if matches!(role, "pm" | "lead") { "(role=pm or role=lead)".into() } else { format!("role={role}") };
        let query = match &root {
            Some(id) => format!("is:task is:ready {route} under:{id} sort:order"),
            None => format!("is:task is:ready {route} sort:order"),
        };
        for node in ctx.store().query(&query, ctx.out.today, i64::MAX as usize)? {
            if STOP.load(Ordering::SeqCst) {
                return Ok(true);
            }
            let props = ctx.store().props_of(&node.id)?;
            let owned = props.get("owner").and_then(Value::as_str).is_some_and(|o| !o.is_empty());
            if owned {
                continue;
            }
            ready.insert(node.id.clone());
            if !seen.ready.contains(&node.id) {
                emit(ctx, "task", &node, source, change);
                if once {
                    return Ok(true);
                }
            }
        }
    }
    seen.ready = ready;
    Ok(false)
}

pub fn run(ctx: &mut Ctx, args: WatchArgs) -> Result<()> {
    let mut recipient = Recipient::target(&args.recipient)?;
    if recipient.actor.is_none() {
        recipient.actor = crate::msg::identity(ctx).actor;
    }
    let _signals = Signals::install();
    let mut seen = Seen::default();
    let mut subscription = None;
    let mut pending = None;
    let mut handshake = Instant::now();
    let mut retry = Instant::now();
    let mut tick = Instant::now();
    let mut offline_notice = false;
    let mut minute = dates::now_local().format("%Y-%m-%dT%H:%M").to_string();
    while !STOP.load(Ordering::SeqCst) {
        if subscription.is_none() && pending.is_none() && Instant::now() >= retry {
            pending = Subscription::connect(ctx.vault.daemon_paths());
            handshake = Instant::now();
            retry = Instant::now() + Duration::from_secs(1);
            if pending.is_none() && !offline_notice {
                eprintln!("watch: daemon offline; polling the board log every second (retrying connection)");
                offline_notice = true;
            }
        }
        if let Some(client) = pending.as_mut() {
            match client.read() {
                Ok(Readiness::Message(Incoming::Response(r))) if r.id == 1 && r.error.is_none() && r.result.is_some() => {
                    subscription = pending.take();
                    offline_notice = false;
                    // Subscribe before the snapshot so changes during reconciliation are queued.
                    let finished = reconcile(ctx, &recipient, &mut seen, args.once, "reconcile", None)?;
                    eprintln!("watch: daemon connected; watching {}", args.recipient);
                    if finished {
                        break;
                    }
                    continue;
                }
                Ok(Readiness::Quiet) if handshake.elapsed() < Duration::from_secs(2) => {}
                Ok(Readiness::Message(Incoming::Event(_))) => {}
                _ => {
                    pending = None;
                    if !offline_notice {
                        eprintln!("watch: daemon unavailable; polling the board log every second (retrying connection)");
                        offline_notice = true;
                    }
                }
            }
        }
        if let Some(client) = subscription.as_mut() {
            match client.read() {
                Ok(Readiness::Message(Incoming::Event(ev))) if ev.event == "changed" => {
                    if reconcile(ctx, &recipient, &mut seen, args.once, "daemon", Some(&ev.data))? {
                        break;
                    }
                }
                Ok(Readiness::Message(Incoming::Event(ev))) if ev.event == "shutdown" => {
                    subscription = None;
                }
                Ok(Readiness::Closed) | Err(_) => {
                    subscription = None;
                }
                _ => {}
            }
        } else {
            if Instant::now() >= tick {
                if reconcile(ctx, &recipient, &mut seen, args.once, "poll", None)? {
                    break;
                }
                tick = Instant::now() + Duration::from_secs(1);
            }
            if pending.is_none() {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let now = dates::now_local().format("%Y-%m-%dT%H:%M").to_string();
        if now != minute {
            minute = now;
            // Scheduled readiness can change without any log event.
            if reconcile(ctx, &recipient, &mut seen, args.once, "reconcile", None)? {
                break;
            }
        }
    }
    Ok(())
}
