//! ThcHost: the vault, as fictty sees it. `fetch` answers the screens' sources from the feeds,
//! `act` writes through thc-core's writer as the person, and a watcher (the daemon's `changed`
//! events, else the vault's log) tells the screen when the vault moved under it.

use crate::docs::{self, Doc};
use crate::feeds::{self, Cx};
use std::collections::HashMap;
use thc_core::edit::Rendered;
use crate::screens::{self, Route};
use crate::sidebar::{OVER, PLACES};
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
pub const FEEDS: [&str; 15] = ["frame", "today", "inbox", "tasks", "pages", "outline", "journal", "search", "log", "detail", "doc", "tx", "context", "dest", "sidebar"];

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
    /// Keep the sidebar's stack in the vault's cache (live screens; never snapshots).
    persist: bool,
    /// Panels dropped on load because their page is gone: said once, at the start.
    dropped: Mutex<Vec<String>>,
    live: AtomicBool,
    notifier: Mutex<Option<Notifier>>,
    /// The size the screen last read the sidebar's rows at (`width`, `height`): only to lay out
    /// the rows an act sends with its change (`activate`), which the screen's own read replaces.
    sidebar_size: Mutex<Value>,
}

impl ThcHost {
    /// A host on an open vault. Its writes are the vault's actor's: open it as the person
    /// (`Actor { kind: "human" }`) for a screen a person drives.
    pub fn new(mut vault: Vault) -> ThcHost {
        // What happened before the screen opened isn't news to it (no toast, no live marks).
        let _ = vault.take_news();
        ThcHost { inner: Arc::new(Inner { vault: Mutex::new(vault), docs: Mutex::new(HashMap::new()), at: None, watch: false, persist: false, dropped: Mutex::new(Vec::new()), live: AtomicBool::new(false), notifier: Mutex::new(None), sidebar_size: Mutex::new(json!({})) }) }
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
        let mut ui = screens::build(route, self.now().date());
        // The stack this device kept for this vault (sidebar.md §11), starting where it was.
        if let Some(stack) = self.load_stack() {
            let beside = !stack.open.is_empty();
            ui.store.insert("sidebar".into(), stack.to_value());
            ui.store.insert("sidebar_shown".into(), beside.into());
            ui.store.insert("sidebar_beside".into(), beside.into());
        }
        ui
    }

    /// Keep the stack across runs: `.persisting()` on a live screen.
    pub fn persisting(self) -> ThcHost {
        self.rebuild(|i| i.persist = true)
    }

    /// Where the stack is kept: the vault's cache, per vault and per device (§11).
    fn stack_file(&self) -> Option<std::path::PathBuf> {
        let v = self.inner.vault.lock().ok()?;
        Some(crate::sidebar::file(&v.paths.cache))
    }

    /// The kept stack, without panels whose page was deleted (named in `dropped`, for the bar).
    fn load_stack(&self) -> Option<crate::sidebar::Stack> {
        if !self.inner.persist {
            return None;
        }
        let raw = std::fs::read(self.stack_file()?).ok()?;
        let mut stack = crate::sidebar::Stack::from_value(&serde_json::from_slice(&raw).ok()?);
        let v = self.inner.vault.lock().ok()?;
        let mut gone = Vec::new();
        stack.open.retain(|p| {
            let here = p.kind != crate::sidebar::Kind::Page || v.store.node(&p.target).ok().flatten().is_some_and(|n| !n.deleted);
            if !here {
                gone.push(p.target.clone());
            }
            here
        });
        if let Ok(mut d) = self.inner.dropped.lock() {
            *d = gone;
        }
        Some(stack)
    }

    /// Write the stack to the cache (atomically: a temp file, then a rename).
    fn save_stack(&self, stack: &crate::sidebar::Stack) {
        if !self.inner.persist {
            return;
        }
        if let Ok(v) = self.inner.vault.lock() {
            crate::sidebar::save(&v.paths.cache, stack);
        }
    }

    /// A fictty app on this vault, showing `route` in `theme`: run it (`run`) or draw it
    /// (`headless`).
    pub fn app(&self, route: &Route, theme: &str) -> fictty::App {
        fictty::App::new(self.screen(route)).themes(crate::theme::themes()).theme(theme).host(self.clone())
    }

    /// A feed's value: what `fetch` answers, callable directly (`thc screen --feed`).
    pub fn feed(&self, name: &str, args: &Value) -> anyhow::Result<Value> {
        let vault = self.inner.vault.lock().map_err(|_| anyhow::anyhow!("the vault lock was poisoned"))?;
        let cx = Cx { store: &vault.store, g: &UNICODE, now: self.now() };
        let mut out = self.feed_rows(&cx, name, args)?;
        if !matches!(name, "frame" | "doc" | "detail" | "tx" | "context" | "dest" | "sidebar") {
            // The screen's `live` entry (the ids an agent just changed) and the open conflicts mark
            // the rows' gutters.
            let live: std::collections::HashSet<String> = args.get("live").and_then(Value::as_object).map(|m| m.keys().cloned().collect()).unwrap_or_default();
            let conflicted = vault.store.open_conflicts()?.into_iter().map(|c| c.1).collect();
            feeds::decorate(&mut out, &live, &conflicted);
        }
        Ok(out)
    }

    /// A feed's own value, before the screen's marks.
    fn feed_rows(&self, cx: &Cx, name: &str, args: &Value) -> anyhow::Result<Value> {
        let s = |k: &str| args.get(k).and_then(Value::as_str).filter(|v| !v.is_empty());
        Ok(match name {
            "frame" => {
                let mut v = feeds::frame(cx)?;
                v["live"] = self.live().into();
                v
            }
            // Today's sections, or with `mode: agenda` (`w`) the agenda.
            "today" if s("mode") == Some("agenda") => feeds::agenda(cx)?,
            "today" => feeds::today(cx)?,
            "context" => feeds::context(cx, s("id").unwrap_or(""), s("query").unwrap_or(""))?,
            "sidebar" => {
                let stack = crate::sidebar::Stack::from_value(args.get("stack").unwrap_or(&Value::Null));
                let (width, height) = (args.get("width").and_then(Value::as_u64), args.get("height").and_then(Value::as_u64));
                if let (Some(w), Some(h), Ok(mut size)) = (width, height, self.inner.sidebar_size.lock()) {
                    *size = json!({"width": w, "height": h});
                }
                feeds::sidebar(cx, &stack, s("active").unwrap_or(""), height.unwrap_or(40) as usize, width.unwrap_or(120) as usize)?
            }
            "dest" => feeds::dest(cx, s("query").unwrap_or(""))?,
            "tx" => feeds::tx(cx, s("id").unwrap_or(""))?,
            "inbox" => feeds::inbox(cx)?,
            "tasks" => {
                let keep: Vec<String> = args.get("keep").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
                feeds::tasks(cx, s("query").unwrap_or("status:open"), &keep)?
            }
            "pages" => feeds::pages(cx)?,
            // Nothing open: an empty outline.
            "outline" => match s("id") {
                Some(id) => feeds::outline(cx, id)?,
                None => json!({"rows": [], "title_m": "", "meta_m": "", "bar_m": ""}),
            },
            "journal" => {
                let date = s("date").and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).unwrap_or(cx.today());
                feeds::journal(cx, date)?
            }
            "search" => feeds::search(cx, s("query").unwrap_or(""))?,
            "log" => feeds::log(cx, s("by") == Some("agents"))?,
            "doc" => feeds::doc(cx, s("key").unwrap_or(""))?,
            "detail" => feeds::detail(cx, s("id").unwrap_or(""))?,
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
            Ok((b.finish()?, ()))
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
            // `t`: a task, or a task no more (§6.2).
            "task" => {
                let id = id()?;
                let on = {
                    let v = self.inner.vault.lock().map_err(|_| anyhow::anyhow!("the vault lock was poisoned"))?;
                    v.store.must_node(&id)?.status.is_none()
                };
                self.write(|b| b.set_props(&id, &[("status".into(), if on { "todo".into() } else { String::new() })]))
            }
            // `D`, confirmed: delete it (and what's under it); the bar says so, `u` in the Log
            // brings it back.
            "delete" => {
                let id = id()?;
                let (short, label) = {
                    let v = self.inner.vault.lock().map_err(|_| anyhow::anyhow!("the vault lock was poisoned"))?;
                    (v.store.short(&id), v.store.must_node(&id)?.label())
                };
                let r = self.write(|b| b.delete(&id).map(|_| ()))?;
                self.toast(&format!("deleted {short}  {}", thc_core::present::truncate_str(&label, 40, UNICODE.ellipsis)), "text")?;
                Ok(r)
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
            // The Journal's day moves (`[` `]` `T`): `{"from": "2026-10-11", "by": -1 | 1 | "today"}`.
            // The day is the screen's store entry `journal_date`; dates are the host's to work out.
            "day" => {
                let today = self.now().date();
                let from = s("from").and_then(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok()).unwrap_or(today);
                let to = match args.get("by").and_then(Value::as_i64) {
                    Some(by) => from + chrono::Duration::days(by),
                    None => today,
                };
                let date = to.format("%Y-%m-%d").to_string();
                self.send(fictty::state::Msg::Write { key: "journal_date".into(), write: fictty::store::Write::Put { value: date.clone().into() }, if_rev: None, by: "person".into(), for_ms: None })?;
                Ok(json!({"ok": true, "date": date}))
            }
            // The bar prompt's answer: `{"ask", "id", "text"}`. `capture` and `inbox` (a, A) add the
            // text in capture syntax to today's journal or the inbox; `due` (d) and `move` (m) act
            // on the row they were asked about.
            "answer" => {
                let text = s("text").unwrap_or_default();
                match s("ask").as_deref() {
                    Some("capture") => return self.perform("add", &json!({"text": text})),
                    Some("inbox") => return self.perform("add", &json!({"text": text, "inbox": true})),
                    _ => {}
                }
                let id = id()?;
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
            // `{"tx": …}`: undo one transaction as a new one (thc undo --tx), the person's. The
            // bar confirms it (§8.3), and `U` undoes that undo: `{"tx": redo_tx, "of": the
            // transaction first undone, "word": "redo" | "undo"}`, so U alternates redo and undo
            // and the bar names the original change each time.
            // No tx (the agent's toast has gone, so `agent_tx` has too): nothing to do.
            "undo" => {
                let Some(tx) = s("tx") else { return Ok(json!({"ok": true, "undone": null})) };
                let of = s("of").unwrap_or_else(|| tx.clone());
                let redo = s("word").as_deref() == Some("redo");
                let (who, nodes) = {
                    let v = self.inner.vault.lock().map_err(|_| anyhow::anyhow!("the vault lock was poisoned"))?;
                    let entries = v.store.history_where("tx = ?1 ORDER BY okey", &[&of])?;
                    let who = entries.first().map(|e| who_said(&e.actor)).ok_or_else(|| anyhow::anyhow!("no transaction {tx}"))?;
                    let nodes: std::collections::HashSet<&str> = entries.iter().filter(|e| e.entity.len() == 12).map(|e| e.entity.as_str()).collect();
                    (who, nodes.len())
                };
                let r = self.write(|b| {
                    let inverse = thc_core::review::undo_ops(b.store, &tx)?;
                    if inverse.is_empty() {
                        anyhow::bail!("that transaction has nothing to undo");
                    }
                    b.ops.extend(inverse);
                    Ok(())
                })?;
                let new = r["tx"].as_str().unwrap_or_default().to_string();
                self.put("redo_tx", new.into())?;
                self.put("redo_of", of.clone().into())?;
                // What U does next: redo what was just undone, or undo what was just redone.
                self.put("redo_word", if redo { "undo" } else { "redo" }.into())?;
                self.put("agent_tx", Value::Null)?;
                let n = if nodes == 1 { "1 node".to_string() } else { format!("{nodes} nodes") };
                let (verb, next) = if redo { ("redone", "U undo") } else { ("undone", "U redo") };
                self.toast(&format!("{verb}  tx {} · {who} · {n}   {next}", suffix(&of)), "text")?;
                Ok(r)
            }
            // `{"key", "now", "a", "b"}`: the screen's entry `key` to whichever of a and b it isn't
            // (`w` sections / agenda, `\` the detail pane, the Log's `@`).
            "flip" => {
                let key = s("key").ok_or_else(|| anyhow::anyhow!("flip needs a key"))?;
                let raw = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
                let to = if s("now").unwrap_or_default() == raw("b") { raw("a") } else { raw("b") };
                self.put(&key, to.clone().into())?;
                Ok(json!({"ok": true, key: to}))
            }
            // The sidebar (sidebar.md). Every act reads the stack it was pressed against
            // (`{store.sidebar}`) and writes the next one; the rules are `sidebar::Stack`'s.
            // `aside`: `{"id"}` a row's node (its page or day), or `{"kind", "ref"}`.
            "aside" => {
                let mut stack = crate::sidebar::Stack::from_value(args.get("stack").unwrap_or(&Value::Null));
                let panel = match (s("kind"), s("ref"), s("title")) {
                    (Some(k), Some(r), _) => crate::sidebar::Panel::new(serde_json::from_value(json!(k))?, &r),
                    // A link's page or tag (`as`: page, tag, or link: whichever it names).
                    (_, _, Some(t)) => self.panel_for_link(&t, s("as").as_deref().unwrap_or("link"))?,
                    (_, _, None) if s("as").is_some() => return Ok(json!({"ok": true, "panel": null})),
                    _ => self.panel_for(&id()?)?,
                };
                let title = panel.key();
                let opened = stack.open(panel);
                // Opening shows it (the column, or the drawer below 120), and it's the active
                // panel (§2 rule 1). Narrow, the drawer (or replace) takes the focus too; beside,
                // the focus stays where it was. Which of them shows is the UI's `when` on the
                // screen's width: the focus goes to both, and only the one showing takes it.
                self.put_stack(&stack, true)?;
                let view = s("view");
                self.activate(&stack, Some(&title), view.as_deref().map(|v| (v, OVER)))?;
                match opened {
                    crate::sidebar::Opened::Evicted(p) => self.toast(&format!("{} closed to make room · ⌥⇧T reopen", p.key()), "text")?,
                    crate::sidebar::Opened::Refused => self.toast("8 panels pinned · unpin one to open another", "neg")?,
                    _ => {}
                }
                Ok(json!({"ok": true, "panel": title}))
            }
            // `{"op": close | pin | fold | up | down, "key"}` on a panel; `reopen` and `show` on
            // the stack.
            "panel" => {
                let mut stack = crate::sidebar::Stack::from_value(args.get("stack").unwrap_or(&Value::Null));
                // The panel: its key, or its id (the active one's, `sidebar_active`).
                let key = s("key").filter(|k| !k.is_empty()).or_else(|| s("id").and_then(|id| stack.open.iter().find(|p| p.id() == id).map(|p| p.key()))).unwrap_or_default();
                let mut shown = match args.get("shown") {
                    Some(Value::Bool(b)) => *b,
                    Some(Value::String(v)) => !v.is_empty() && v != "false",
                    _ => false,
                };
                match s("op").as_deref() {
                    Some("close") => {
                        stack.close(&key);
                    }
                    Some("pin") => {
                        stack.toggle_pin(&key);
                    }
                    Some("fold") => {
                        stack.toggle_fold(&key);
                    }
                    Some("up") => {
                        stack.move_by(&key, true);
                    }
                    Some("down") => {
                        stack.move_by(&key, false);
                    }
                    Some("reopen") => {
                        if stack.reopen().is_none() {
                            return Ok(json!({"ok": true, "reopened": null}));
                        }
                    }
                    Some("close_all") => stack.close_unpinned(),
                    // A day panel's `[` `]` `T` (§8.1).
                    Some("day") => {
                        let by = args.get("by").and_then(Value::as_i64);
                        stack.retarget_day(&key, by, self.now().date());
                    }
                    // ⌥S, ⌥J, ⌥K: the UI moved the focus to the panel already (so keys pressed
                    // right after act there, S02c); the active panel follows it. Without one it
                    // knows (none yet, or it's gone), the top panel, and the focus goes there.
                    Some("focus") => {
                        if stack.position(&key).is_some() {
                            self.activate(&stack, Some(&key), None)?;
                        } else {
                            self.activate(&stack, None, s("view").as_deref().map(|v| (v, PLACES)))?;
                        }
                        return Ok(json!({"ok": true}));
                    }
                    // ⌥S showed it; ⌥\ hides or shows it.
                    Some("show") => shown = true,
                    Some("toggle") => shown = !shown,
                    other => anyhow::bail!("no panel op {other:?}"),
                }
                self.put_stack(&stack, shown)?;
                // The active panel after the op: the one it was on (where it went), else the one
                // that took its place; the focus with it, where the keys were pressed.
                let view = s("view");
                let at = s("at").and_then(|a| a.parse::<usize>().ok());
                let keep = if stack.position(&key).is_some() { Some(key.clone()) } else { at.and_then(|i| stack.open.get(i.min(stack.open.len().saturating_sub(1)))).map(|p| p.key()) };
                let refocus = s("op").is_some_and(|o| o != "toggle" && o != "reopen");
                self.activate(&stack, keep.as_deref(), if refocus && !stack.open.is_empty() { view.as_deref().map(|v| (v, PLACES)) } else { None })?;
                Ok(json!({"ok": true}))
            }
            // `⌥M` (§3.4): the main view goes to this panel's page or day, and the panel closes.
            "to_main" => {
                let mut stack = crate::sidebar::Stack::from_value(args.get("stack").unwrap_or(&Value::Null));
                let key = s("key").unwrap_or_default();
                let Some(p) = stack.open.iter().find(|p| p.key() == key).cloned() else { return Ok(json!({"ok": true})) };
                stack.close(&key);
                self.put_stack(&stack, true)?;
                self.activate(&stack, None, None)?;
                self.open_in_main(&p, args.get("from").and_then(Value::as_u64), &Value::Null)?;
                Ok(json!({"ok": true}))
            }
            // Enter on a list panel's row (§8.2): its page or day in the main view.
            "to_main_node" => {
                let p = self.panel_for(&id()?)?;
                self.open_in_main(&p, args.get("from").and_then(Value::as_u64), &Value::Null)?;
                Ok(json!({"ok": true}))
            }
            // A click on `[[Page]]` (or ⌃O on it): the page in the main view, and Esc comes back
            // to what was open where the link was (`back`).
            "follow" => {
                let Some(title) = s("title") else { return Ok(json!({"ok": true, "followed": null})) };
                let id = {
                    let v = self.inner.vault.lock().map_err(|_| anyhow::anyhow!("the vault lock was poisoned"))?;
                    v.store.find_root_by_title(&title, false)?.ok_or_else(|| anyhow::anyhow!("no page {title:?}"))?
                };
                let p = crate::sidebar::Panel::new(crate::sidebar::Kind::Page, &id);
                self.open_in_main(&p, args.get("from").and_then(Value::as_u64), args.get("back").unwrap_or(&Value::Null))?;
                Ok(json!({"ok": true, "followed": id}))
            }
            // Esc from a page or day opened from elsewhere (⌥M, a link): back to the view it came
            // from, and to what was open there, if anything.
            "return" => {
                let Some(tab) = args.get("tab").and_then(Value::as_u64).map(|t| t as usize).filter(|t| *t < screens::View::TABS.len()) else {
                    return Ok(json!({"ok": true, "returned": null}));
                };
                let view = screens::View::TABS[tab];
                self.put("return_tab", Value::Null)?;
                self.put("return_open", Value::Null)?;
                self.send(fictty::state::Msg::Set { id: "views".into(), selected: None, key: None, tab: Some(tab) })?;
                let open = args.get("open").filter(|o| o.is_object()).cloned();
                match open {
                    Some(o) => {
                        self.put(&format!("open_{}", view.name()), o)?;
                        self.send(fictty::state::Msg::Focus { id: format!("{}_doc", view.name()) })?;
                    }
                    None => {
                        if let Some(list) = screens::first_focus(&screens::Route::to(view)) {
                            self.send(fictty::state::Msg::Focus { id: list })?;
                        }
                    }
                }
                Ok(json!({"ok": true, "returned": tab}))
            }
            // `{"keys": "p h"}`: press a command's keys (the palette runs a command this way, so it
            // does just what its key does).
            "press" => {
                let keys = s("keys").ok_or_else(|| anyhow::anyhow!("press needs keys"))?;
                for key in keys.split(' ').filter(|k| !k.is_empty()) {
                    self.send(fictty::state::Msg::Key { key: key.into() })?;
                }
                Ok(json!({"ok": true}))
            }
            other => anyhow::bail!("no action {other:?}"),
        }
    }

    /// A link's panel (§2): `page` a page by its title, `tag` the tag's list, `link` whichever the
    /// name is (a page first).
    fn panel_for_link(&self, name: &str, kind: &str) -> anyhow::Result<crate::sidebar::Panel> {
        use crate::sidebar::{Kind, Panel};
        let tag = || Panel::new(Kind::Query, &format!("#{}", name.trim_start_matches('#').to_lowercase()));
        if kind == "tag" {
            return Ok(tag());
        }
        let v = self.inner.vault.lock().map_err(|_| anyhow::anyhow!("the vault lock was poisoned"))?;
        match v.store.find_root_by_title(name, false)? {
            Some(id) => Ok(Panel::new(Kind::Page, &id)),
            None if kind == "link" && v.store.find_root_by_title(name, true)?.is_some() => Ok(tag()),
            None => anyhow::bail!("no page {name:?}"),
        }
    }

    /// The panel a row opens aside (sidebar.md §2): a page is itself, a day is its day, a tag is a
    /// `#tag` list, anything else is the page or day it lives in.
    pub fn panel_for(&self, id: &str) -> anyhow::Result<crate::sidebar::Panel> {
        use crate::sidebar::{Kind, Panel};
        let v = self.inner.vault.lock().map_err(|_| anyhow::anyhow!("the vault lock was poisoned"))?;
        let mut n = v.store.must_node(id)?;
        if n.is_tag {
            return Ok(Panel::new(Kind::Query, &format!("#{}", n.label().to_lowercase())));
        }
        loop {
            if let Some(d) = n.journal.as_deref().and_then(thc_core::dates::DateVal::from_stored) {
                return Ok(Panel::new(Kind::Day, &d.date().format("%Y-%m-%d").to_string()));
            }
            if n.title.is_some() && n.parent.is_none() {
                return Ok(Panel::new(Kind::Page, &n.id));
            }
            match &n.parent {
                Some(p) => n = v.store.must_node(p)?,
                None => anyhow::bail!("{} is in the inbox: no page or day to open beside", v.store.short(id)),
            }
        }
    }

    /// Make a panel the active one (`sidebar_active`, its id; `sidebar_target`, what in it takes
    /// the focus): `key`, else the top panel. With `focus` (the tab whose instance has the keys,
    /// and the places that may take it), the focus goes to it in whichever of those places shows:
    /// fictty focuses only a node that shows, and what shows at the screen's width is the UI's
    /// `when`, so the host never needs to know the width.
    fn activate(&self, stack: &crate::sidebar::Stack, key: Option<&str>, focus: Option<(&str, &[&str])>) -> anyhow::Result<()> {
        let p = key.and_then(|k| stack.open.iter().find(|p| p.key() == k)).or(stack.open.first());
        let (id, target) = p.map(|p| (p.id(), p.target())).unwrap_or_default();
        self.put("sidebar_active", id.clone().into())?;
        self.put("sidebar_target", target.clone().into())?;
        if let (Some((view, places)), false) = (focus, target.is_empty()) {
            for m in self.panel_focus(stack, &id, view, places) {
                self.send(m)?;
            }
        }
        Ok(())
    }

    /// The messages that put the focus on panel `id` (the stack's, active) in tab `view`, in
    /// whichever of `places` shows. The sidebar's rows with this stack go first: a real screen
    /// reads them again on its own thread after the stack changes, and a focus that came before
    /// them would find no panel there to take it (a new panel's nodes come from its row).
    pub fn panel_focus(&self, stack: &crate::sidebar::Stack, id: &str, view: &str, places: &[&str]) -> Vec<fictty::state::Msg> {
        let Some(p) = stack.open.iter().find(|p| p.id() == id) else { return vec![] };
        let mut args = self.inner.sidebar_size.lock().map(|s| s.clone()).unwrap_or_default();
        args["stack"] = stack.to_value();
        args["active"] = id.into();
        let rows = self.feed("sidebar", &args).map_err(|e| format!("{e:#}"));
        let mut out = vec![fictty::state::Msg::Data { name: "sidebar_rows".into(), result: rows }];
        out.extend(places.iter().map(|place| fictty::state::Msg::Focus { id: format!("{place}_{view}_{}", p.target()) }));
        out
    }

    /// A page or day in the main view (§3.4, §12): its tab, its layer open to write in, the focus
    /// in it. A list panel's view or query goes to its tab. With `from` (another tab) or `back`
    /// (what was open), Esc comes back there (`return`).
    fn open_in_main(&self, p: &crate::sidebar::Panel, from: Option<u64>, back: &Value) -> anyhow::Result<()> {
        use crate::sidebar::Kind;
        let (tab, view, open) = match p.kind {
            Kind::Page => (3, "pages", json!({"kind": "page", "id": p.target, "key": format!("page/{}", p.target)})),
            Kind::Day => {
                let d = if p.target == "today" { self.now().date().format("%Y-%m-%d").to_string() } else { p.target.clone() };
                self.put("journal_date", d.clone().into())?;
                (4, "journal", json!({"kind": "day", "key": format!("day/{d}")}))
            }
            Kind::View | Kind::Query => {
                let tab = screens::View::TABS.iter().position(|v| v.name() == p.target).unwrap_or(2);
                if p.kind == Kind::Query {
                    self.put("tasks_q", p.target.clone().into())?;
                }
                self.send(fictty::state::Msg::Set { id: "views".into(), selected: None, key: None, tab: Some(tab) })?;
                return Ok(());
            }
        };
        if let Some(from) = from.filter(|f| *f != tab as u64 || back.is_object()) {
            self.put("return_tab", from.into())?;
            self.put("return_open", back.clone())?;
        }
        self.put(&format!("open_{view}"), open)?;
        self.send(fictty::state::Msg::Set { id: "views".into(), selected: None, key: None, tab: Some(tab) })?;
        self.send(fictty::state::Msg::Focus { id: format!("{view}_doc") })?;
        Ok(())
    }

    /// Write the stack, whether it shows, and whether it's beside the view (both: the detail pane
    /// and the Journal's calendar yield to it, §6.2).
    fn put_stack(&self, stack: &crate::sidebar::Stack, shown: bool) -> anyhow::Result<()> {
        self.save_stack(stack);
        self.put("sidebar", stack.to_value())?;
        self.put("sidebar_shown", shown.into())?;
        self.put("sidebar_beside", (shown && !stack.open.is_empty()).into())
    }

    /// Write one of the screen's store entries, as the host.
    fn put(&self, key: &str, value: Value) -> anyhow::Result<()> {
        self.write_store(key, fictty::store::Write::Put { value }, None)
    }

    /// A store write, lasting `for_ms` on the screen's clock when given (fictty takes it away).
    fn write_store(&self, key: &str, write: fictty::store::Write, for_ms: Option<u64>) -> anyhow::Result<()> {
        self.send(fictty::state::Msg::Write { key: key.into(), write, if_rev: None, by: "host".into(), for_ms })
    }

    /// A toast (§7.3), for `TOAST_MS` on the screen's clock, in a theme token's tone.
    fn toast(&self, text: &str, tone: &str) -> anyhow::Result<()> {
        self.send(fictty::state::Msg::Toast { text: text.into(), tone: Some(tone.into()), ms: Some(TOAST_MS) })
    }

    /// The daemon connected or went away: the bar's `● live` / `○ local` (§3.3), from
    /// thc_core::live's messages.
    pub fn connected(&self, up: bool) {
        self.inner.live.store(up, Ordering::Relaxed);
        if let Some(n) = self.inner.notifier.lock().ok().and_then(|n| n.clone()) {
            n.changed("frame");
        }
    }
}

/// How long a toast shows, and so how long its `u` is on offer (§7.3).
const TOAST_MS: u64 = 4000;
/// How long a changed row's `•` shows (§4).
const LIVE_MS: u64 = 3000;

/// A transaction's short suffix, as the Log shows it.
fn suffix(tx: &str) -> &str {
    &tx[tx.len().saturating_sub(6)..]
}

/// Who wrote, as the bar says it: `◆ claude`, or `you`.
fn who_said(actor: &str) -> String {
    match actor.strip_prefix("agent:") {
        Some(a) => format!("{}{}{a}", UNICODE.agent, UNICODE.agent_sep),
        None if actor.starts_with("agent") => format!("{}{}agent", UNICODE.agent, UNICODE.agent_sep),
        None => "you".into(),
    }
}

impl Host for ThcHost {
    fn start(&self, notifier: Notifier) {
        if let Ok(mut n) = self.inner.notifier.lock() {
            *n = Some(notifier.clone());
        }
        if self.inner.watch {
            let host = self.clone();
            std::thread::spawn(move || host.watch());
        }
        // A kept panel whose page was deleted is said once (sidebar.md §11).
        let gone = self.inner.dropped.lock().map(|mut d| std::mem::take(&mut *d)).unwrap_or_default();
        if let Some(id) = gone.first() {
            let title = self.inner.vault.lock().ok().and_then(|v| v.store.node(id).ok().flatten().map(|n| n.label())).unwrap_or_else(|| id.clone());
            let more = if gone.len() > 1 { format!(" (and {} more)", gone.len() - 1) } else { String::new() };
            let _ = self.toast(&format!("¶ {title} was deleted · removed from the sidebar{more}"), "text");
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
            (None, Doc::Day(_)) => String::new(),
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
                    Ok((b.finish()?, ()))
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
                Ok((b.finish()?, ()))
            })?;
            last = self.render(&vault, &doc)?.ok_or_else(|| anyhow::anyhow!("{key}: gone"))?;
            edited = docs::with_ids(&last.text, text);
        }
        vault.transact(|s| {
            let mut b = TxBuilder::new(s, today);
            thc_core::edit::apply(&mut b, &last, &edited)?;
            Ok((b.finish()?, ()))
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
        let mut agent = None;
        if let Ok(mut v) = self.inner.vault.lock() {
            // Another process here (the CLI, an agent) applies its own events to the shared store
            // and moves the log cursors itself, so catch-up alone finds nothing: the vault's news
            // counts every event applied since the last look, whoever applied it.
            let _ = v.catch_up();
            let n = v.take_news();
            news |= n.overflow || !n.events.is_empty();
            agent = agent_change(&v.store, &n);
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
        // An agent's change (§7.3): the toast on the agent's tint, the rows it touched marked
        // live for 3 s, its transaction the one `u` undoes while the toast shows. The screen's
        // clock times all three (fictty's expiring writes), so they replay.
        if let Some(c) = agent {
            let live: serde_json::Map<String, Value> = c.nodes.iter().map(|id| (id.clone(), Value::Bool(true))).collect();
            let _ = self.write_store("live", fictty::store::Write::Merge { value: Value::Object(live) }, Some(LIVE_MS));
            let _ = self.write_store("agent_tx", fictty::store::Write::Put { value: c.tx.clone().into() }, Some(TOAST_MS));
            let _ = self.toast(&format!("{}   L review  u undo", c.words), "agent");
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

/// What an agent just did, as the toast says it.
struct AgentChange {
    /// `◆ claude added h2kbq "Repot the basil"`, or `◆ claude changed 3 nodes`.
    words: String,
    /// The last of its transactions (what `u` undoes).
    tx: String,
    /// The nodes it touched (full ids).
    nodes: Vec<String>,
}

/// The agents' part of what arrived (not this screen's own writes), if any: the TUI's toast
/// (thc-tui app.rs, `poll`) in words.
fn agent_change(store: &thc_core::store::Store, news: &thc_core::vault::News) -> Option<AgentChange> {
    let eids: Vec<&str> = news.events.iter().filter(|(_, own)| !own).map(|(e, _)| e.as_str()).collect();
    if eids.is_empty() {
        return None;
    }
    let json = serde_json::to_string(&eids).ok()?;
    let fresh = store.history_where("eid IN (SELECT value FROM json_each(?1)) ORDER BY okey", &[&json]).ok()?;
    let agents: Vec<&thc_core::model::HistoryEntry> = fresh.iter().filter(|e| e.actor.starts_with("agent")).collect();
    let first = agents.first()?;
    let mut nodes: Vec<String> = Vec::new();
    for e in &agents {
        if e.entity.len() == 12 && !nodes.contains(&e.entity) {
            nodes.push(e.entity.clone());
        }
    }
    let created: Vec<&&thc_core::model::HistoryEntry> = agents.iter().filter(|e| e.op == "node.create").collect();
    let g = &UNICODE;
    let who = who_said(&first.actor);
    let words = if created.len() == 1 {
        let id = &created[0].entity;
        let label = store.node(id).ok().flatten().map(|n| thc_core::present::truncate_str(&n.label(), 40, g.ellipsis)).unwrap_or_default();
        format!("{who} added {} \"{label}\"", store.short(id))
    } else {
        let n = nodes.len().max(1);
        format!("{who} changed {n} node{}", if n == 1 { "" } else { "s" })
    };
    Some(AgentChange { words, tx: agents.last()?.tx.clone(), nodes })
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
                    self.connected(true);
                    true
                }
                Ok(thc_core::live::LiveMsg::Disconnected) => {
                    self.connected(false);
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
