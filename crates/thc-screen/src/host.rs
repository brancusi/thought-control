//! ThcHost: the vault, as fictty sees it. `fetch` answers the screens' sources from the feeds,
//! `act` writes through thc-core's writer as the person, and a watcher (the daemon's `changed`
//! events, else the vault's log) tells the screen when the vault moved under it.

use crate::docs::{self, Doc};
use crate::feeds::{self, Cx};
use std::collections::HashMap;
use thc_core::edit::Rendered;
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
pub const FEEDS: [&str; 12] = ["frame", "today", "inbox", "tasks", "pages", "outline", "journal", "search", "log", "detail", "doc", "tx"];

/// The vault behind a screen. Cloning shares it.
#[derive(Clone)]
pub struct ThcHost {
    inner: Arc<Inner>,
}

struct Inner {
    vault: Mutex<Vault>,
    /// The documents the screen has loaded (`page/<id>`, `day/<date>`), each as last rendered:
    /// what its next save is lined up against, and what a vault change is compared with.
    docs: Mutex<HashMap<String, DocState>>,
    /// A pinned "now" (tests, renders); else thc's clock (THC_NOW, or the real one).
    at: Option<NaiveDateTime>,
    watch: bool,
    live: AtomicBool,
    notifier: Mutex<Option<Notifier>>,
    /// The route the screen starts on: its tab is shown when it starts (see `app`).
    first: Mutex<Option<Route>>,
}

impl ThcHost {
    /// A host on an open vault. Its writes are the vault's actor's: open it as the person
    /// (`Actor { kind: "human" }`) for a screen a person drives.
    pub fn new(vault: Vault) -> ThcHost {
        ThcHost { inner: Arc::new(Inner { vault: Mutex::new(vault), docs: Mutex::new(HashMap::new()), at: None, watch: false, live: AtomicBool::new(false), notifier: Mutex::new(None), first: Mutex::new(None) }) }
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

    /// A fictty app on this vault, showing `route` in `theme`: run it (`run`) or draw it
    /// (`headless`). Its first message shows the route's tab.
    /// TODO(fictty): a tabs node's starting tab, so the UI says it and no message is needed.
    pub fn app(&self, route: &Route, theme: &str) -> fictty::App {
        if let Ok(mut f) = self.inner.first.lock() {
            *f = Some(route.clone());
        }
        fictty::App::new(self.screen(route)).themes(crate::theme::themes()).theme(theme).host(self.clone())
    }

    /// A feed's value: what `fetch` answers, callable directly (`thc screen --feed`).
    pub fn feed(&self, name: &str, args: &Value) -> anyhow::Result<Value> {
        let vault = self.inner.vault.lock().map_err(|_| anyhow::anyhow!("the vault lock was poisoned"))?;
        let cx = Cx { store: &vault.store, g: &UNICODE, now: self.now() };
        let s = |k: &str| args.get(k).and_then(Value::as_str).filter(|v| !v.is_empty());
        Ok(match name {
            "frame" => {
                let mut v = feeds::frame(&cx)?;
                v["live"] = self.live().into();
                v
            }
            "today" => feeds::today(&cx)?,
            "tx" => feeds::tx(&cx, s("id").unwrap_or(""))?,
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
            "doc" => feeds::doc(&cx, s("key").unwrap_or(""))?,
            "detail" => feeds::detail(&cx, s("id").unwrap_or(""))?,
            other => anyhow::bail!("no feed {other:?}"),
        })
    }

    /// A message into the screen, as from the socket.
    fn send(&self, msg: fictty::state::Msg) -> anyhow::Result<()> {
        let sent = self.inner.notifier.lock().ok().and_then(|n| n.as_ref().map(|n| n.msg(msg)));
        if sent != Some(true) {
            anyhow::bail!("the screen has gone");
        }
        Ok(())
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
                if matches!(view, View::Detail | View::Outline | View::Page) && route.id.is_none() {
                    anyhow::bail!("open {}: nothing selected", view.name());
                }
                let ui = self.screen(&route);
                self.send(fictty::state::Msg::Push { ui })?;
                for m in screens::show_tab(&route) {
                    self.send(m)?;
                }
                Ok(json!({"ok": true, "view": view.name()}))
            }
            // Show a view's tab (keys 1–7): the views are already on screen, so nothing is pushed.
            // TODO(fictty): an action that shows tab N would make this the screen's own.
            "tab" => {
                let n = args.get("n").and_then(Value::as_u64).ok_or_else(|| anyhow::anyhow!("tab needs n"))? as usize;
                let view = *View::TABS.get(n).ok_or_else(|| anyhow::anyhow!("no tab {n}"))?;
                for m in screens::show_tab(&Route::to(view)) {
                    self.send(m)?;
                }
                Ok(json!({"ok": true, "tab": n}))
            }
            // Focus a node (a click focuses): the prompt going back to the list that asked.
            "focus" => {
                let id = id()?;
                self.send(fictty::state::Msg::Click { id: id.clone(), row: None, tab: None })?;
                Ok(json!({"ok": true, "focus": id}))
            }
            // The bar prompt's answer (`d` a date, `m` where to): `{"ask": "due"|"move", "id", "text"}`.
            "answer" => {
                let id = id()?;
                let text = s("text").unwrap_or_default();
                match s("ask").as_deref() {
                    Some("due") => self.write(|b| b.set_props(&id, &[("due".into(), text.clone())])),
                    Some("move") => {
                        let to = {
                            let v = self.inner.vault.lock().map_err(|_| anyhow::anyhow!("the vault lock was poisoned"))?;
                            let t = text.trim().trim_start_matches('¶').trim();
                            v.store.find_root_by_title(t, false)?.ok_or_else(|| anyhow::anyhow!("no page {t:?}"))?
                        };
                        self.write(|b| b.move_to(&id, Some(to), None, None))
                    }
                    other => anyhow::bail!("nothing asked {other:?}"),
                }
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
        if let Some(r) = self.inner.first.lock().ok().and_then(|mut f| f.take()) {
            for m in screens::show_tab(&r) {
                notifier.msg(m);
            }
        }
        if self.inner.watch {
            let host = self.clone();
            std::thread::spawn(move || host.watch());
        }
    }

    fn fetch(&self, name: &str, args: &Value) -> Result<Value, String> {
        self.feed(name, args).map_err(|e| format!("{e:#}"))
    }

    fn act(&self, name: &str, args: &Value) -> Result<Value, String> {
        self.perform(name, args).map_err(|e| format!("{e:#}"))
    }

    fn keys(&self) -> Vec<String> {
        vec!["page/".into(), "day/".into()]
    }

    fn load(&self, key: &str) -> Result<Value, String> {
        self.load_doc(key).map(Value::String).map_err(|e| format!("{e:#}"))
    }

    fn save(&self, key: &str, value: &Value) -> Result<(), String> {
        let text = value.as_str().ok_or_else(|| format!("{key}: a document is text"))?;
        self.save_doc(key, text).map_err(|e| format!("{e:#}"))?;
        // What else shows the page (Today's rows, the pages list) reads it again.
        if let Some(n) = self.inner.notifier.lock().ok().and_then(|n| n.clone()) {
            for f in FEEDS {
                n.changed(f);
            }
        }
        Ok(())
    }
}

/// What the host knows of a document the screen has loaded.
#[derive(Default)]
struct DocState {
    /// Its last render: what the next save lines up against, and what a vault change is
    /// compared with. None for a day not written in yet.
    last: Option<Rendered>,
    /// Every line it has shown, with its node: a line that comes back is that node.
    seen: HashMap<String, String>,
}

impl ThcHost {
    /// A document's render (`edit::render`), or None for a day nobody has written in yet.
    fn render(&self, vault: &Vault, doc: &Doc) -> anyhow::Result<Option<Rendered>> {
        let root = match doc {
            Doc::Page(id) => Some(vault.store.resolve(id)?),
            Doc::Day(d) => vault.store.journal_node(&d.format("%Y-%m-%d").to_string())?,
        };
        root.map(|r| thc_core::edit::render(&vault.store, &r)).transpose()
    }

    /// The text a document's editor shows: its render, ids and help left out.
    pub fn load_doc(&self, key: &str) -> anyhow::Result<String> {
        let doc = Doc::parse(key).ok_or_else(|| anyhow::anyhow!("{key}: not a page or a day"))?;
        let vault = self.inner.vault.lock().map_err(|_| anyhow::anyhow!("the vault lock was poisoned"))?;
        let rendered = self.render(&vault, &doc)?;
        let text = match (&rendered, &doc) {
            (Some(r), _) => docs::shown(&r.text),
            // A day not written yet: its heading, as the render will write it.
            (None, Doc::Day(d)) => format!("# {} (journal)\n\n", d.format("%Y-%m-%d")),
            (None, Doc::Page(id)) => anyhow::bail!("no page {id}"),
        };
        self.remember(key, rendered)?;
        Ok(text)
    }

    /// A document's latest render, and its lines among those it has shown.
    fn remember(&self, key: &str, rendered: Option<Rendered>) -> anyhow::Result<()> {
        let mut all = self.inner.docs.lock().map_err(|_| anyhow::anyhow!("poisoned"))?;
        let st = all.entry(key.to_string()).or_default();
        if let Some(r) = &rendered {
            st.seen.extend(docs::seen(r));
        }
        st.last = rendered;
        Ok(())
    }

    /// Write an edited document back: its lines get their ids again (lined up with the last
    /// render) and `edit::apply` turns the difference into one transaction, as the person.
    pub fn save_doc(&self, key: &str, text: &str) -> anyhow::Result<()> {
        let doc = Doc::parse(key).ok_or_else(|| anyhow::anyhow!("{key}: not a page or a day"))?;
        let mut vault = self.inner.vault.lock().map_err(|_| anyhow::anyhow!("the vault lock was poisoned"))?;
        let today = self.now().date();
        let (last, seen) = {
            let all = self.inner.docs.lock().map_err(|_| anyhow::anyhow!("poisoned"))?;
            all.get(key).map(|d| (d.last.clone(), d.seen.clone())).unwrap_or_default()
        };
        let mut last = match (last, &doc) {
            (Some(r), _) => r,
            // The first line written on a day: the day comes first, then its lines.
            (None, Doc::Day(d)) => {
                let d = *d;
                vault.transact(|s| {
                    let mut b = TxBuilder::new(s, today);
                    b.journal(d)?;
                    Ok((b.finish(), ()))
                })?;
                self.render(&vault, &doc)?.ok_or_else(|| anyhow::anyhow!("{key}: the day wasn't made"))?
            }
            (None, Doc::Page(id)) => self.render(&vault, &Doc::Page(id.clone()))?.ok_or_else(|| anyhow::anyhow!("no page {id}"))?,
        };
        let mut edited = docs::with_ids(&last.text, text);
        // A line that comes back (an undo after the save that deleted it) is the node it was:
        // restored, so it keeps its id, then lined up again with the render that shows it.
        let back: Vec<String> = docs::new_lines(&edited)
            .iter()
            .filter_map(|l| seen.get(l))
            .filter(|id| vault.store.nodes_where("n.id = ?1 AND n.deleted = 1", &[id]).is_ok_and(|n| !n.is_empty()))
            .cloned()
            .collect();
        if !back.is_empty() {
            vault.transact(|s| {
                let mut b = TxBuilder::new(s, today);
                for id in &back {
                    b.restore(id)?;
                }
                Ok((b.finish(), ()))
            })?;
            last = self.render(&vault, &doc)?.ok_or_else(|| anyhow::anyhow!("{key}: gone"))?;
            edited = docs::with_ids(&last.text, text);
        }
        vault.transact(|s| {
            let mut b = TxBuilder::new(s, today);
            thc_core::edit::apply(&mut b, &last, &edited)?;
            Ok((b.finish(), ()))
        })?;
        let now = self.render(&vault, &doc)?;
        drop(vault);
        self.remember(key, now)
    }

    /// One watch step: take in what changed in the vault (another process, another device, an
    /// agent), refetch what's on screen, and reload every open document whose text moved, as an
    /// edit, so carets and undo carry on. The watcher calls this every second; tests call it.
    /// True when the screen is still there.
    pub fn poll(&self, news: bool) -> bool {
        let Some(notifier) = self.inner.notifier.lock().ok().and_then(|n| n.clone()) else { return false };
        let mut news = news;
        let mut moved = Vec::new();
        if let Ok(mut v) = self.inner.vault.lock() {
            // Another process here (the CLI, an agent) applies its own events to the shared store
            // and moves the log cursors itself, so catch-up alone finds nothing: the vault's news
            // counts every event applied since the last look, whoever applied it.
            let _ = v.catch_up();
            let n = v.take_news();
            news |= n.overflow || !n.events.is_empty();
            if news && let Ok(docs) = self.inner.docs.lock() {
                for (key, st) in docs.iter() {
                    let Some(doc) = Doc::parse(key) else { continue };
                    let now = self.render(&v, &doc).ok().flatten();
                    if now.as_ref().map(|r| &r.text) != st.last.as_ref().map(|r| &r.text) {
                        moved.push(key.clone());
                    }
                }
            }
        }
        for key in moved {
            notifier.reload(&key);
        }
        if news {
            for f in FEEDS {
                // False once the screen has gone.
                if !notifier.msg(fictty::state::Msg::Changed { host: f.into() }) {
                    return false;
                }
            }
        }
        true
    }
}

impl ThcHost {
    /// The daemon's `changed` events when it runs (live), else the vault's own log, every
    /// second: whatever moved the store refetches the feeds on screen.
    fn watch(&self) {
        let paths = match self.inner.vault.lock() {
            Ok(v) => v.paths.clone(),
            Err(_) => return,
        };
        let events = thc_core::live::spawn(paths);
        loop {
            let news = match events.recv_timeout(Duration::from_secs(1)) {
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
            if !self.poll(news) {
                return;
            }
        }
    }
}
