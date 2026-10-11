//! ThcHost: the vault, as fictty sees it. `fetch` answers the screens' sources from the feeds,
//! `act` writes through thc-core's writer as the person, and a watcher (the daemon's `changed`
//! events, else the vault's log) tells the screen when the vault moved under it.

use crate::feeds::{self, Cx};
use crate::screens::{self, Route, View};
use chrono::{NaiveDate, NaiveDateTime};
use fictty::{Host, Notifier};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thc_core::builder::TxBuilder;
use thc_core::glyphs::UNICODE;
use thc_core::vault::Vault;

/// Every feed a screen can read; a vault change refetches the ones on screen.
pub const FEEDS: [&str; 10] = ["frame", "today", "inbox", "tasks", "pages", "outline", "journal", "search", "log", "detail"];

/// The vault behind a screen. Cloning shares it.
#[derive(Clone)]
pub struct ThcHost {
    inner: Arc<Inner>,
}

struct Inner {
    vault: Mutex<Vault>,
    /// A pinned "now" (tests, renders); else thc's clock (THC_NOW, or the real one).
    at: Option<NaiveDateTime>,
    watch: bool,
    live: AtomicBool,
    notifier: Mutex<Option<Notifier>>,
}

impl ThcHost {
    /// A host on an open vault. Its writes are the vault's actor's: open it as the person
    /// (`Actor { kind: "human" }`) for a screen a person drives.
    pub fn new(vault: Vault) -> ThcHost {
        ThcHost { inner: Arc::new(Inner { vault: Mutex::new(vault), at: None, watch: false, live: AtomicBool::new(false), notifier: Mutex::new(None) }) }
    }

    /// Draw as of this time instead of the clock's.
    pub fn at(self, now: NaiveDateTime) -> ThcHost {
        self.rebuild(|i| i.at = Some(now))
    }

    /// Watch the vault (the daemon, else its log every second) and refetch what's on screen when
    /// it changes. Off for snapshots.
    pub fn watching(self) -> ThcHost {
        self.rebuild(|i| i.watch = true)
    }

    fn rebuild(self, f: impl FnOnce(&mut Inner)) -> ThcHost {
        let mut inner = Arc::try_unwrap(self.inner).unwrap_or_else(|_| panic!("ThcHost is configured before it's shared"));
        f(&mut inner);
        ThcHost { inner: Arc::new(inner) }
    }

    pub fn now(&self) -> NaiveDateTime {
        self.inner.at.unwrap_or_else(thc_core::dates::now_local)
    }

    /// Whether the daemon is connected (the bar's `● live` / `○ local`).
    pub fn live(&self) -> bool {
        self.inner.live.load(Ordering::Relaxed)
    }

    /// The screen for a route, as of now.
    pub fn screen(&self, route: &Route) -> fictty::model::Ui {
        screens::build(route, self.now().date())
    }

    /// A feed's value: what `fetch` answers, callable directly (`thc screen --feed`).
    pub fn feed(&self, name: &str, args: &Value) -> anyhow::Result<Value> {
        let vault = self.inner.vault.lock().map_err(|_| anyhow::anyhow!("the vault lock was poisoned"))?;
        let cx = Cx { store: &vault.store, g: &UNICODE, now: self.now() };
        let s = |k: &str| args.get(k).and_then(Value::as_str).filter(|v| !v.is_empty());
        Ok(match name {
            "frame" => {
                let mut v = feeds::frame(&cx, args.get("tab").and_then(Value::as_u64).unwrap_or(0) as usize)?;
                v["live"] = self.live().into();
                v
            }
            "today" => feeds::today(&cx)?,
            "inbox" => feeds::inbox(&cx)?,
            "tasks" => feeds::tasks(&cx, s("query").unwrap_or("status:open"))?,
            "pages" => feeds::pages(&cx)?,
            "outline" => feeds::outline(&cx, s("id").ok_or_else(|| anyhow::anyhow!("outline needs an id"))?)?,
            "journal" => {
                let date = s("date").and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).unwrap_or(cx.today());
                feeds::journal(&cx, date)?
            }
            "search" => feeds::search(&cx, s("query").unwrap_or(""))?,
            "log" => feeds::log(&cx)?,
            "detail" => feeds::detail(&cx, s("id").ok_or_else(|| anyhow::anyhow!("detail needs an id"))?)?,
            other => anyhow::bail!("no feed {other:?}"),
        })
    }

    /// A write as the vault's actor, in one transaction.
    fn write(&self, f: impl FnOnce(&mut TxBuilder) -> anyhow::Result<()>) -> anyhow::Result<Value> {
        let mut vault = self.inner.vault.lock().map_err(|_| anyhow::anyhow!("the vault lock was poisoned"))?;
        let today = self.now().date();
        let (events, ()) = vault.transact(|s| {
            let mut b = TxBuilder::new(s, today);
            f(&mut b)?;
            Ok((b.finish(), ()))
        })?;
        Ok(json!({"ok": true, "events": events.len(), "tx": events.first().map(|e| e.tx.clone())}))
    }

    /// Do one of the screens' actions (see `act`), with `anyhow` errors.
    pub fn perform(&self, name: &str, args: &Value) -> anyhow::Result<Value> {
        let s = |k: &str| args.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_string);
        let id = || s("id").ok_or_else(|| anyhow::anyhow!("{name} needs an id"));
        match name {
            "done" => {
                let id = id()?;
                self.write(|b| b.complete(&id).map(|_| ()))
            }
            "reopen" => {
                let id = id()?;
                self.write(|b| b.set_props(&id, &[("status".into(), "todo".into()), ("done_at".into(), String::new())]))
            }
            // `{"id": …, "props": {"due": "fri", "priority": "high"}}`: values as `thc set` reads them.
            "set" => {
                let id = id()?;
                let props: Vec<(String, String)> = args
                    .get("props")
                    .and_then(Value::as_object)
                    .ok_or_else(|| anyhow::anyhow!("set needs props"))?
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())))
                    .collect();
                self.write(|b| b.set_props(&id, &props))
            }
            // `{"text": "Call dentist due:fri", "under": id}`: capture syntax; today's journal
            // unless `under` or `inbox` says otherwise.
            "add" => {
                let text = s("text").ok_or_else(|| anyhow::anyhow!("add needs text"))?;
                let (under, inbox) = (s("under"), args.get("inbox").and_then(Value::as_bool).unwrap_or(false));
                let today = self.now().date();
                self.write(|b| {
                    let cap = thc_core::capture::parse(&text, today)?;
                    let parent = match (under, inbox) {
                        (Some(u), _) => Some(u),
                        (None, true) => None,
                        (None, false) => Some(b.journal(today)?),
                    };
                    b.create_from_capture(parent, &cap, None).map(|_| ())
                })
            }
            // `{"id": …, "under": id}`, or no `under` for the inbox.
            "mv" => {
                let id = id()?;
                let under = s("under");
                self.write(|b| b.move_to(&id, under, None, None))
            }
            // Show another screen: the route's fields, as the keys wrote them.
            "open" => {
                let view = s("view").and_then(|v| View::parse(&v)).ok_or_else(|| anyhow::anyhow!("open: which view?"))?;
                let route = Route { view, id: s("id"), query: s("query"), date: s("date"), back: s("back").and_then(|b| View::parse(&b)) };
                if matches!(view, View::Detail | View::Outline) && route.id.is_none() {
                    anyhow::bail!("open {}: nothing selected", view.name());
                }
                let ui = self.screen(&route);
                let sent = self.inner.notifier.lock().ok().and_then(|n| n.as_ref().map(|n| n.msg(fictty::state::Msg::Push { ui })));
                if sent != Some(true) {
                    anyhow::bail!("open: the screen has gone");
                }
                Ok(json!({"ok": true, "view": view.name()}))
            }
            other => anyhow::bail!("no action {other:?}"),
        }
    }
}

impl Host for ThcHost {
    fn start(&self, notifier: Notifier) {
        if let Ok(mut n) = self.inner.notifier.lock() {
            *n = Some(notifier.clone());
        }
        if self.inner.watch {
            let host = self.clone();
            std::thread::spawn(move || host.watch(notifier));
        }
    }

    fn fetch(&self, name: &str, args: &Value) -> Result<Value, String> {
        self.feed(name, args).map_err(|e| format!("{e:#}"))
    }

    fn act(&self, name: &str, args: &Value) -> Result<Value, String> {
        self.perform(name, args).map_err(|e| format!("{e:#}"))
    }
}

impl ThcHost {
    /// The daemon's `changed` events when it runs (live), else the vault's own log, every
    /// second: whatever moved the store refetches the feeds on screen.
    fn watch(&self, notifier: Notifier) {
        let paths = match self.inner.vault.lock() {
            Ok(v) => v.paths.clone(),
            Err(_) => return,
        };
        let events = thc_core::live::spawn(paths);
        loop {
            let mut news = match events.recv_timeout(Duration::from_secs(1)) {
                Ok(thc_core::live::LiveMsg::Connected { .. }) => {
                    self.inner.live.store(true, Ordering::Relaxed);
                    true
                }
                Ok(thc_core::live::LiveMsg::Disconnected) => {
                    self.inner.live.store(false, Ordering::Relaxed);
                    true
                }
                Ok(thc_core::live::LiveMsg::Event { .. }) => true,
                Err(_) => false,
            };
            if let Ok(mut v) = self.inner.vault.lock() {
                let before = v.store.max_okey().ok().flatten();
                let caught = v.catch_up().unwrap_or(0);
                news |= caught > 0 || v.store.max_okey().ok().flatten() != before;
            }
            if news {
                for f in FEEDS {
                    // False once the screen has gone: stop watching.
                    if !notifier.msg(fictty::state::Msg::Changed { host: f.into() }) {
                        return;
                    }
                }
            }
        }
    }
}
