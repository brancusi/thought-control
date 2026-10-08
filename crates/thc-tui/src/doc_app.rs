//! The document editor inside the app (tui-editor.md): which document is open, saving it
//! through `outline::plan`, and patching it from changes made elsewhere.

use crate::app::{App, View};
use crate::editor::{Doc, Target};
use std::time::{Duration, Instant};
use thc_core::builder::TxBuilder;
use thc_core::outline::{self, Kind};

/// One save for the writer thread, and what came back.
pub(crate) struct Job {
    root: Option<String>,
    journal: Option<String>,
    ops: Vec<outline::BlockOp>,
    afters: std::collections::HashMap<String, Option<String>>,
    parsed: Vec<String>,
    sent: crate::editor::Sent,
}

pub(crate) struct Done {
    root: Option<String>,
    result: Result<Vec<outline::OpResult>, String>,
    afters: std::collections::HashMap<String, Option<String>>,
    parsed: Vec<String>,
    sent: crate::editor::Sent,
}

/// Saves in order on one thread, through the daemon's `blocks.apply`.
pub struct Saver {
    tx: std::sync::mpsc::Sender<Job>,
    rx: std::sync::mpsc::Receiver<Done>,
    pending: usize,
    /// A save asked for while one was out (`Some(all)`): planned when that one's result is
    /// folded in. A plan made before then would diff against what the vault had before the
    /// save in flight, and its result would land on top of newer edits (⌃T ⌃T came back open).
    waiting: Option<bool>,
    /// A test's writer (`Saver::manual`): used whether or not a daemon is live.
    manual: bool,
    /// A patch from the vault asked for while a save was out: done once it's in. The vault
    /// doesn't have that save yet, so patching then put its older state over newer edits.
    patch: bool,
}

impl Saver {
    /// A refresh from the vault waits for the save that's out (the fuzz: a change from
    /// elsewhere is still to land).
    #[cfg(test)]
    pub(crate) fn patch_waiting(&self) -> bool {
        self.patch
    }

    fn spawn(paths: thc_core::vault::Paths) -> Saver {
        let (tx, jobs) = std::sync::mpsc::channel::<Job>();
        let (done_tx, rx) = std::sync::mpsc::channel::<Done>();
        std::thread::spawn(move || {
            let mut client: Option<thc_core::proto::Client> = None;
            for job in jobs {
                let result = (|| -> Result<(Option<String>, Vec<outline::OpResult>), String> {
                    if client.is_none() {
                        client = thc_core::proto::Client::connect(&paths);
                    }
                    let c = client.as_mut().ok_or("the daemon went away")?;
                    let mut params = serde_json::json!({ "ops": job.ops, "via": "tui" });
                    match (&job.root, &job.journal) {
                        (Some(r), _) => params["root"] = serde_json::json!(r),
                        (None, Some(j)) => params["journal"] = serde_json::json!(j),
                        _ => return Err("nothing to save into".into()),
                    }
                    let v = c.call("blocks.apply", params).map_err(|e| {
                        client = None;
                        format!("{e:#}")
                    })?;
                    let results: Vec<outline::OpResultWire> = serde_json::from_value(v["results"].clone()).map_err(|e| e.to_string())?;
                    Ok((v["root"].as_str().map(str::to_string), results.into_iter().map(|r| r.into_result()).collect()))
                })();
                let (root, result) = match result {
                    Ok((root, r)) => (root, Ok(r)),
                    Err(e) => (None, Err(e)),
                };
                if done_tx.send(Done { root, result, afters: job.afters, parsed: job.parsed, sent: job.sent }).is_err() {
                    return;
                }
            }
        });
        Saver { tx, rx, pending: 0, waiting: None, manual: false, patch: false }
    }

    /// A save out, or one waiting to go: its result belongs to the document it came from.
    pub(crate) fn busy_now(&self) -> bool {
        self.pending > 0 || self.waiting.is_some()
    }

    /// A save out, or one waiting for it.
    #[cfg(test)]
    pub(crate) fn busy(&self) -> bool {
        self.pending > 0 || self.waiting.is_some() || self.patch
    }

    /// A writer the test drives: it gets the jobs and hands back results when (and in the
    /// order) it chooses, as a slow or busy daemon would.
    #[cfg(test)]
    pub(crate) fn manual() -> (Saver, ManualWriter) {
        let (tx, jobs) = std::sync::mpsc::channel::<Job>();
        let (done, rx) = std::sync::mpsc::channel::<Done>();
        (Saver { tx, rx, pending: 0, waiting: None, manual: true, patch: false }, ManualWriter { jobs, done })
    }
}

/// The other end of `Saver::manual`.
#[cfg(test)]
pub(crate) struct ManualWriter {
    jobs: std::sync::mpsc::Receiver<Job>,
    done: std::sync::mpsc::Sender<Done>,
}

#[cfg(test)]
impl ManualWriter {
    /// Run every job sent so far against the vault (as the daemon would), results held back.
    pub(crate) fn run(&self, vault: &mut thc_core::vault::Vault, today: chrono::NaiveDate) -> Vec<Done> {
        let mut out = Vec::new();
        while let Ok(job) = self.jobs.try_recv() {
            let result = save_inline(vault, job.root.clone(), job.journal.as_deref().and_then(|j| chrono::NaiveDate::parse_from_str(j, "%Y-%m-%d").ok()), job.ops, today);
            let (root, result) = match result {
                Ok((root, r)) => (Some(root), Ok(r)),
                Err(e) => (None, Err(e)),
            };
            out.push(Done { root, result, afters: job.afters, parsed: job.parsed, sent: job.sent });
        }
        out
    }

    /// Hand one result back to the app (it folds it in at its next `drain_saves`).
    pub(crate) fn deliver(&self, d: Done) {
        let _ = self.done.send(d);
    }
}

/// A save written here, in this process: the journal day made first when it has no note yet,
/// then the plan, with the committed revisions read back.
fn save_inline(vault: &mut thc_core::vault::Vault, root: Option<String>, journal: Option<chrono::NaiveDate>, ops: Vec<outline::BlockOp>, today: chrono::NaiveDate) -> Result<(String, Vec<outline::OpResult>), String> {
    let root = match (root, journal) {
        (Some(r), _) => r,
        (None, Some(date)) => vault
            .transact(|st| {
                let mut b = TxBuilder::new(st, today);
                let id = b.journal(date)?;
                Ok((b.finish(), id))
            })
            .map(|(_, id)| id)
            .map_err(|e| format!("{e:#}"))?,
        (None, None) => return Err("nothing to save into".into()),
    };
    let r = root.clone();
    let (_, mut res) = vault.transact(move |st| outline::plan(st, &r, &ops, today)).map_err(|e| format!("{e:#}"))?;
    // The results' revisions are the plan's provisional ones: read the committed ones.
    outline::refresh_revs(&vault.store, &mut res);
    Ok((root, res))
}

/// A document's key in the caret memory: `page:<id>`, `day:<date>`.
pub(crate) fn caret_key(t: &Target) -> String {
    match t {
        Target::Page { id, .. } => format!("page:{id}"),
        Target::Journal { date } => format!("day:{}", date.format("%Y-%m-%d")),
    }
}

/// The caret memory (`carets.json` in the vault's cache): document → (line id, byte, scroll).
pub(crate) type Carets = std::collections::HashMap<String, (String, usize, usize)>;

pub(crate) fn load_carets(cache: &std::path::Path) -> Carets {
    if crate::SNAPSHOT.with(|s| s.get()) && std::env::var_os("THC_TUI_SNAPSHOT_CARETS").is_none() {
        return Carets::default();
    }
    std::fs::read(cache.join("carets.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_carets(cache: &std::path::Path, c: &Carets) {
    // Snapshots keep none unless a test asks (THC_TUI_SNAPSHOT_CARETS=1): replayed keys
    // count on a page opening at its top.
    if crate::SNAPSHOT.with(|s| s.get()) && std::env::var_os("THC_TUI_SNAPSHOT_CARETS").is_none() {
        return;
    }
    // Bounded: the most recent 300 documents are plenty.
    let mut c = c.clone();
    if c.len() > 300 {
        let keys: Vec<String> = c.keys().take(c.len() - 300).cloned().collect();
        for k in keys {
            c.remove(&k);
        }
    }
    if let Ok(j) = serde_json::to_vec(&c) {
        let tmp = cache.join("carets.json.tmp");
        if std::fs::write(&tmp, j).is_ok() {
            let _ = std::fs::rename(&tmp, cache.join("carets.json"));
        }
    }
}

/// A borrowed row under the document: read-only, completed or opened where it lives.
#[derive(Clone, Debug)]
pub struct FooterRow {
    pub id: String,
    pub status: Option<String>,
    pub text: String,
    /// Origin, then why it's here: `¶ Admin · due oct 01`.
    pub meta: String,
}

impl App {
    /// `also today` / `linked from` for the open document (tui-editor.md §3.3, §3.4).
    pub fn load_footer(&mut self) {
        let Some(d) = self.doc.as_ref() else { self.doc_footer = None; return };
        let s = &self.vault.store;
        let here: std::collections::HashSet<&str> = d.blocks().iter().map(|l| l.id.as_str()).collect();
        let origin = |id: &str| -> String {
            let mut cur = s.node(id).ok().flatten();
            while let Some(n) = cur.clone() {
                match n.parent.clone() {
                    Some(p) => cur = s.node(&p).ok().flatten(),
                    None => break,
                }
            }
            match cur {
                Some(top) if top.journal.is_some() => {
                    let j = top.journal.unwrap_or_default();
                    match chrono::NaiveDate::parse_from_str(&j, "%Y-%m-%d") {
                        Ok(d) => format!("§ {}", d.format("%a %b %d").to_string().to_lowercase()),
                        Err(_) => format!("§ {j}"),
                    }
                }
                Some(top) => top.title.map(|t| format!("¶ {t}")).unwrap_or_else(|| "inbox".into()),
                None => String::new(),
            }
        };
        let row = |n: &thc_core::model::Node| -> FooterRow {
            let block = thc_core::outline::Block {
                id: n.id.clone(), parent: n.parent.clone(), depth: 0, kind: if n.status.is_some() { Kind::Task } else { Kind::Bullet }, status: n.status.clone(),
                text: n.text.clone(), scheduled: n.scheduled.clone(), due: n.due.clone(), priority: n.priority.clone(),
                repeat: n.repeat.as_ref().and_then(|r| r.get("text")).and_then(|t| t.as_str()).map(str::to_string),
                tags: vec![], rev: None, text_rev: None, conflict: false, done_at: n.done_at.clone(), gap: None,
            };
            let why = crate::editor::meta_text(&block, self.today);
            let o = origin(&n.id);
            FooterRow { id: n.id.clone(), status: n.status.clone(), text: s.render_text(&n.text), meta: if why.is_empty() { o } else { format!("{o} · {why}") } }
        };
        self.doc_footer = match &d.target {
            Target::Journal { date } if *date == self.today => {
                let mut rows: Vec<FooterRow> = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for q in ["status:open (due<=today or sched<=today) sort:due", "done>=today sort:updated-"] {
                    for n in s.query(q, self.today, 50).unwrap_or_default() {
                        if !here.contains(n.id.as_str()) && seen.insert(n.id.clone()) {
                            rows.push(row(&n));
                        }
                    }
                }
                (!rows.is_empty()).then(|| ("also today".to_string(), rows))
            }
            Target::Page { id, .. } => {
                let nodes = s.nodes_where("n.deleted=0 AND n.id IN (SELECT src FROM edges WHERE rel='mention' AND dst=?1) ORDER BY n.updated_ms DESC LIMIT 50", &[id]).unwrap_or_default();
                let rows: Vec<FooterRow> = nodes.iter().filter(|n| !here.contains(n.id.as_str())).map(row).collect();
                (!rows.is_empty()).then(|| (format!("linked from {:02}", rows.len()), rows))
            }
            _ => None,
        };
    }

    /// The document this view shows, if it's a document (a journal day, an open page).
    pub fn doc_target(&self) -> Option<Target> {
        match self.view {
            View::Journal => Some(Target::Journal { date: self.journal_date }),
            View::Pages => self.page_open.as_ref().map(|id| Target::Page { id: id.clone(), title: self.node_label(id) }),
            _ => None,
        }
    }

    /// After every reload: open the view's document, switch documents (saving the old one), or
    /// patch the open one with changes made elsewhere.
    pub fn sync_doc(&mut self) {
        let want = self.doc_target();
        let same = match (&self.doc, &want) {
            (Some(d), Some(t)) => match (&d.target, t) {
                (Target::Page { id: a, .. }, Target::Page { id: b, .. }) => a == b,
                (a, b) => a == b,
            },
            (None, None) => return,
            _ => false,
        };
        if same {
            self.patch_doc();
            self.load_footer();
            self.build_rail();
            return;
        }
        // Arriving from a view (no document open) or leaving to one: the rail's order is
        // recomputed then, and only then (navigation.md §8).
        if self.doc.is_none() || want.is_none() {
            self.rail_frozen = None;
        }
        if self.doc.is_some() {
            self.save_doc(true);
            // A save that waited for the one out is planned from this document: wait for it
            // here (one round trip), or it would go with the document.
            if self.doc_saver.as_ref().is_some_and(|s| s.waiting.is_some()) {
                self.drain_saves(true);
            }
            self.remember_caret();
            if let Some(d) = self.doc.take() {
                // A panel showing the same page keeps the document (sidebar.md §7.1).
                self.park_main_doc(d);
            }
        }
        if let Some(t) = want {
            self.open_doc(t);
            // Every arrival parks the document (navigation.md §6.1): Tab and ⇧Tab still change
            // views until you write.
            self.doc_parked = true;
        }
        self.build_rail();
    }

    /// Whether the rail shows beside the open document (navigation.md §3): 120 columns or more,
    /// outside Focus unless `nav` is on there, and `[tui] page_rail` outside it.
    pub fn rail_shows(&self) -> bool {
        self.doc.is_some()
            && self.screen_width >= 120
            && if self.focus_mode { self.focus_cfg.has(thc_core::tui_config::El::Nav) } else { self.tui_prefs.page_rail }
    }

    /// The detail pane takes room beside the document: it's on, and the sidebar (which it
    /// gives way to, sidebar.md §6.2) isn't a column. A document's text column is measured
    /// without the room a yielded pane would have had (ui::split_width).
    pub fn detail_shows(&self) -> bool {
        self.show_detail && !(self.sidebar_col.is_some() && crate::sidebar::policy::DETAIL_YIELDS)
    }

    /// The crumb above the title instead, when the rail doesn't fit (or is off).
    pub fn crumb_shows(&self) -> bool {
        self.doc.is_some()
            && !self.rail_shows()
            && if self.focus_mode { self.focus_cfg.has(thc_core::tui_config::El::Nav) } else { true }
    }

    /// The rail's rows: pages (recent first, then A–Z, with open counts) beside a page; days
    /// with entries (today first, then newest) beside a day.
    pub fn build_rail(&mut self) {
        self.rail.clear();
        if self.doc.is_none() || self.screen_width < 120 {
            return;
        }
        let s = &self.vault.store;
        match self.doc.as_ref().map(|d| d.target.clone()) {
            Some(Target::Page { .. }) => {
                let pages = s.nodes_where(&format!("n.parent IS NULL AND n.title IS NOT NULL AND n.is_tag=0 AND n.deleted=0 AND {} ORDER BY n.title COLLATE NOCASE", thc_core::views::HIDDEN_SQL), &[]).unwrap_or_default();
                let mut open: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
                // Each open task climbs to its page (not every page walked down to its tasks: the
                // whole vault on every open, a third of opening a 5,000-line page, vw384). A
                // deleted node on the way stops the climb, as it stopped the walk down (UNION, not
                // UNION ALL: a parent loop, which the store shouldn't have, still ends).
                if let Ok(mut st) = s.conn.prepare_cached(
                    "WITH RECURSIVE up(task, cur) AS (SELECT id, id FROM nodes WHERE status IN ('todo','doing','waiting') AND deleted = 0 \
                     UNION SELECT up.task, n.parent FROM up JOIN nodes n ON n.id = up.cur WHERE n.parent IS NOT NULL AND n.deleted = 0) \
                     SELECT r.id, count(*) FROM up JOIN nodes r ON r.id = up.cur \
                     WHERE r.parent IS NULL AND r.title IS NOT NULL AND r.is_tag = 0 AND r.deleted = 0 GROUP BY r.id",
                ) {
                    if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
                        for (id, n) in rows.flatten() {
                            open.insert(id, n as usize);
                        }
                    }
                }
                let recent: Vec<String> = self.recent_docs.iter().filter_map(|t| match t {
                    Target::Page { id, .. } => Some(id.clone()),
                    _ => None,
                }).collect();
                let item = |p: &thc_core::model::Node| crate::app::RailItem::Page { id: p.id.clone(), title: p.label(), open: open.get(&p.id).copied().unwrap_or(0) };
                let mut items: Vec<crate::app::RailItem> = Vec::new();
                match &self.rail_frozen {
                    // Frozen (navigation.md §8): the order as it was, a page gone drops out, a new
                    // page goes in by name among the A–Z ones after it.
                    Some(order) => {
                        for id in order {
                            if let Some(p) = pages.iter().find(|p| &p.id == id) {
                                items.push(item(p));
                            }
                        }
                        for p in pages.iter().filter(|p| !order.contains(&p.id)) {
                            let t = p.label().to_lowercase();
                            let at = items.iter().rposition(|it| matches!(it, crate::app::RailItem::Page { title, .. } if title.to_lowercase() <= t)).map_or(items.len(), |i| i + 1);
                            items.insert(at, item(p));
                        }
                    }
                    None => {
                        for id in &recent {
                            if let Some(p) = pages.iter().find(|p| &p.id == id) {
                                items.push(item(p));
                            }
                        }
                        for p in &pages {
                            if !recent.contains(&p.id) {
                                items.push(item(p));
                            }
                        }
                        self.rail_frozen = Some(items.iter().filter_map(|it| match it {
                            crate::app::RailItem::Page { id, .. } => Some(id.clone()),
                            _ => None,
                        }).collect());
                    }
                }
                self.rail = items;
            }
            Some(Target::Journal { .. }) => {
                let today = self.today;
                let mut days: Vec<(chrono::NaiveDate, usize)> = Vec::new();
                if let Ok(mut st) = s.conn.prepare(
                    "SELECT j.journal, (SELECT count(*) FROM nodes c WHERE c.parent = j.id AND c.deleted = 0) AS n FROM nodes j \
                     WHERE j.journal IS NOT NULL AND j.deleted = 0 AND j.parent IS NULL ORDER BY j.journal DESC LIMIT 60",
                ) {
                    if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
                        for (j, n) in rows.flatten() {
                            if let Ok(d) = chrono::NaiveDate::parse_from_str(&j, "%Y-%m-%d") {
                                if n > 0 && d != today {
                                    days.push((d, n as usize));
                                }
                            }
                        }
                    }
                }
                let today_n = days.iter().position(|(d, _)| *d == today).map(|i| days.remove(i).1);
                let today_n = today_n.or_else(|| s.journal_node(&today.format("%Y-%m-%d").to_string()).ok().flatten().and_then(|id| s.children(&id).ok()).map(|c| c.len())).unwrap_or(0);
                self.rail.push(crate::app::RailItem::Day { date: today, count: today_n });
                self.rail.extend(days.into_iter().map(|(date, count)| crate::app::RailItem::Day { date, count }));
            }
            None => {}
        }
    }

    /// Reopen the document from the vault and put this device's unsaved lines back in, the
    /// caret on the line it was on: a day made elsewhere, or a structure changed elsewhere.
    /// Note where the caret is in the open document (per page or day, per device: kept in the
    /// vault's cache, never synced) so reopening it lands there.
    pub(crate) fn remember_caret(&mut self) {
        let Some(d) = self.doc.as_ref() else { return };
        // A new, empty line isn't a place to come back to: the line above it is. The fresh line
        // it arrived with: coming back arrives again, so nothing is kept.
        let Some(a) = d.place_anchor() else {
            if self.carets.remove(&caret_key(&d.target)).is_some() {
                save_carets(&self.vault.paths.cache, &self.carets);
            }
            return;
        };
        self.carets.insert(caret_key(&d.target), (a.id, a.byte, d.scroll()));
        save_carets(&self.vault.paths.cache, &self.carets);
    }

    fn reopen_keeping_unsaved(&mut self) {
        let Some(mut d) = self.doc.take() else { return };
        let mine = crate::recover::unsaved(&d);
        let caret = d.caret_anchor();
        // Every view of it (the main view, panels) comes back at its place.
        let (current, places) = d.view_places();
        self.open_doc(d.target.clone());
        if let Some(nd) = self.doc.as_mut() {
            nd.adopt_views(current, &places);
        }
        if let (Some(rec), Some(nd)) = (mine, self.doc.as_mut()) {
            crate::recover::apply(nd, &rec);
        }
        if let Some(nd) = self.doc.as_mut() {
            nd.set_caret_anchor(&caret);
            self.doc_line_id = Some(nd.caret_block().id.clone());
        }
    }

    fn open_doc(&mut self, target: Target) {
        self.doc_pending_scroll = None;
        let same = |a: &Target, b: &Target| match (a, b) {
            (Target::Page { id: x, .. }, Target::Page { id: y, .. }) => x == y,
            (a, b) => a == b,
        };
        self.recent_docs.retain(|t| !same(t, &target));
        self.recent_docs.insert(0, target.clone());
        self.recent_docs.truncate(8);
        // A panel shows this page already: one document, a main view added (sidebar.md §7.1).
        if self.in_panel.is_none() {
            if let Some(mut d) = self.adopt_panel_doc(&target) {
                if let Some((line, byte, scroll)) = self.carets.get(&caret_key(&d.target)).cloned() {
                    if let Some(i) = d.restore_caret(&crate::editor::Anchor { id: line, byte }, false) {
                        self.doc_pending_scroll = Some((scroll.min(i), false));
                    }
                }
                self.doc_line_id = Some(d.caret_block().id.clone());
                self.doc = Some(d);
                self.doc_write = true;
                self.doc_first_ever = false;
                self.load_footer();
                return;
            }
        }
        let s = &self.vault.store;
        let root = match &target {
            Target::Journal { date } => s.journal_node(&date.format("%Y-%m-%d").to_string()).ok().flatten(),
            Target::Page { id, .. } => Some(id.clone()),
        };
        let t0 = Instant::now();
        let blocks = root.as_ref().and_then(|r| outline::render_for_editor(s, r).ok()).unwrap_or_default();
        let t1 = Instant::now();
        let journal = matches!(target, Target::Journal { .. });
        let recovered = crate::recover::take(&self.vault.paths.cache, &target);
        let mut d = Doc::new(target, root, &blocks, self.today);
        d.tick(self.ui.now_ms);
        if std::env::var("THC_TUI_TRACE").is_ok_and(|v| v == "2") {
            eprintln!("open: render {:.1} ms · buffer {:.1} ms · {} lines", (t1 - t0).as_secs_f64() * 1e3, t1.elapsed().as_secs_f64() * 1e3, blocks.len());
        }
        let remembered = self.carets.get(&caret_key(&d.target)).cloned();
        // The remembered scroll goes on at the first layout (`doc_pending_scroll`).
        self.doc_pending_scroll = arrive(&mut d, remembered);
        // Lines a crash left unsaved come back, one ⌃Z away (recover.rs), and save at once.
        let mut recovered_n = 0;
        if let Some(rec) = recovered {
            recovered_n = crate::recover::apply(&mut d, &rec);
        }
        self.doc_line_id = Some(d.caret_block().id.clone());
        self.doc = Some(d);
        if recovered_n > 0 {
            self.save_doc(true);
            self.notice(format!("recovered {recovered_n} unsaved line{} from a crash · ⌃Z undoes", if recovered_n == 1 { "" } else { "s" }));
        }
        self.doc_write = true;
        self.doc_first_ever = journal
            && self
                .vault
                .store
                .conn
                .query_row("SELECT count(*) FROM nodes p WHERE p.journal IS NOT NULL AND p.deleted=0 AND EXISTS (SELECT 1 FROM nodes c WHERE c.parent=p.id AND c.deleted=0)", [], |r| r.get::<_, i64>(0))
                .is_ok_and(|n| n == 0);
        if self.doc.as_ref().is_some_and(|d| d.blocks().iter().any(|l| l.conflict)) {
            self.fill_conflict_names();
            // Opened with the caret on a line moved here: the bar says why, as on arriving.
            if let Some(id) = self.doc.as_ref().map(|d| d.caret_block()).filter(|l| l.conflict_with.as_deref() == Some(crate::doc_ui::MOVED_HERE)).map(|l| l.id.clone()) {
                self.say_why_moved(&id);
            }
        }
        let t2 = Instant::now();
        self.load_footer();
        if std::env::var("THC_TUI_TRACE").is_ok_and(|v| v == "2") {
            eprintln!("open: footer {:.1} ms", t2.elapsed().as_secs_f64() * 1e3);
        }
    }

    /// Save every changed line except the caret's (unless `all`) as one transaction.
    pub fn save_doc(&mut self, all: bool) {
        let today = self.today;
        // One save out at a time: the next is planned once this one's result is in.
        if let Some(saver) = self.doc_saver.as_mut().filter(|s| s.pending > 0) {
            saver.waiting = Some(all || saver.waiting.unwrap_or(false));
            return;
        }
        self.check_near_miss(all);
        let Some(d) = self.doc.as_mut() else { return };
        let plan = d.plan_save(all);
        if plan.ops.is_empty() {
            return;
        }
        let started = self.ui.now_ms;
        d.mark_saving(&plan.parsed, Some(started));
        // Review fixture: a save that never lands, as if more than 3 s late (◌ in the marks).
        if std::env::var_os("THC_TUI_FAKE_SAVE_LATE").is_some() {
            d.mark_saving(&plan.parsed, Some(started.saturating_sub(4000)));
            return;
        }
        // Review fixture: a save that fails (the footer's `not saved · :retry`).
        if std::env::var_os("THC_TUI_FAKE_SAVE_FAIL").is_some() {
            d.mark_saving(&plan.parsed, None);
            d.mark_save_failed(&plan.parsed, "the disk is full (fixture)");
            return;
        }
        // The daemon is live: hand the save to the writer thread; typing never waits.
        let snapshot = crate::SNAPSHOT.with(|x| x.get()) && std::env::var_os("THC_TUI_SNAPSHOT_DAEMON").is_none();
        if (self.daemon_live && !snapshot) || self.doc_saver.as_ref().is_some_and(|s| s.manual) {
            let journal = match &d.target {
                Target::Journal { date } if d.root.is_none() => Some(date.format("%Y-%m-%d").to_string()),
                _ => None,
            };
            let job = Job { root: d.root.clone(), journal, ops: plan.ops, afters: plan.afters, parsed: plan.parsed, sent: plan.sent };
            let paths = self.vault.daemon_paths().clone();
            let saver = self.doc_saver.get_or_insert_with(|| Saver::spawn(paths));
            if saver.tx.send(job).is_ok() {
                saver.pending += 1;
                return;
            }
            self.doc_saver = None;
            return self.error("not saved: the writer stopped · :retry");
        }
        let (root, journal) = match &d.target {
            Target::Journal { date } if d.root.is_none() => (None, Some(*date)),
            Target::Page { id, .. } if d.root.is_none() => (Some(id.clone()), None),
            _ => (d.root.clone(), None),
        };
        let result = save_inline(&mut self.vault, root, journal, plan.ops.clone(), today);
        let Some(d) = self.doc.as_mut() else { return };
        match result {
            Ok((root, results)) => {
                d.root = Some(root);
                let msgs = d.apply_results(&results, &plan.afters, &plan.parsed, &plan.sent, today);
                let msgs = self.name_conflicts(msgs);
                self.log_sizes = self.vault.log.files().unwrap_or_default();
                if let Some(m) = msgs.into_iter().last() {
                    self.info(m);
                }
                // The rail's counts and the conflicts follow the save (see drain_saves).
                self.after_save_results();
            }
            Err(e) => {
                // The text stays in the buffer; ◌ shows after 3 s and the bar says why.
                d.mark_save_failed(&plan.parsed, &e);
                self.error(format!("not saved: {e} · :retry"));
            }
        }
    }

    /// What a save's results change outside the document: the conflicts the banner shows and
    /// the rail's counts.
    fn after_save_results(&mut self) {
        let conflicts = self.vault.store.open_conflicts().unwrap_or_default();
        if conflicts != self.conflicts {
            self.conflicts = conflicts;
        }
        if self.in_panel.is_none() {
            self.build_rail();
        }
    }

    /// Fold the writer thread's results back into the document.
    pub fn drain_saves(&mut self, wait: bool) {
        // A test's writer answers when the test says; waiting for it is the test settling it.
        #[cfg(test)]
        if wait && self.doc_saver.as_ref().is_some_and(|s| s.manual) {
            return crate::fuzz::settle(self);
        }
        let today = self.today;
        loop {
            let Some(saver) = self.doc_saver.as_mut() else { return };
            if saver.pending == 0 {
                if std::mem::take(&mut saver.patch) {
                    self.patch_doc();
                }
                return;
            }
            let got = if wait { saver.rx.recv_timeout(Duration::from_secs(5)).ok() } else { saver.rx.try_recv().ok() };
            let Some(done) = got else {
                if wait {
                    self.doc_saver = None; // gave up waiting: a fresh writer next time
                }
                return;
            };
            saver.pending -= 1;
            let msgs = match (self.doc.as_mut(), done.result) {
                (Some(d), Ok(results)) => {
                    if d.root.is_none() {
                        d.root = done.root.clone();
                    }
                    d.apply_results(&results, &done.afters, &done.parsed, &done.sent, today)
                }
                (Some(d), Err(e)) => {
                    d.mark_save_failed(&done.parsed, &e);
                    vec![format!("not saved: {e} · :retry")]
                }
                (None, Err(e)) => vec![format!("not saved: {e}")],
                (None, Ok(_)) => vec![],
            };
            let msgs = self.name_conflicts(msgs);
            if let Some(m) = msgs.into_iter().last() {
                self.info(m);
            }
            // What the save changed shows at once, as a fresh session would read it: the rail's
            // counts (a page's open tasks, a day's entries; 64j4y) and the conflicts banner (a
            // save that kept both sides; t741c). The rail is the main view's: a panel's save
            // leaves it for the main view's next.
            self.after_save_results();
            // The save that waited for this one: planned now, against what the vault has.
            if let Some(all) = self.doc_saver.as_mut().filter(|s| s.pending == 0).and_then(|s| s.waiting.take()) {
                self.save_doc(all);
            }
            // A patch that waited: the vault now has every save made here.
            if self.doc_saver.as_mut().filter(|s| s.pending == 0 && s.waiting.is_none()).is_some_and(|s| std::mem::take(&mut s.patch)) {
                self.patch_doc();
            }
        }
    }

    /// Conflicts a save just made name who else edited: the chip `≠ claude · c compare` and
    /// the bar `≠ claude changed this line while you typed · both kept · c compare`.
    fn name_conflicts(&mut self, msgs: Vec<String>) -> Vec<String> {
        match self.fill_conflict_names() {
            Some(who) => msgs.into_iter().map(|m| if m.starts_with('≠') { format!("≠ {who} changed this line while you typed · both kept · ⌃O compare") } else { m }).collect(),
            None => msgs,
        }
    }

    /// Name the other side of every `≠` line that has no name yet (a synced conflict says who
    /// too: `≠ claude`, `≠ mbp`). Returns the last name given.
    pub(crate) fn fill_conflict_names(&mut self) -> Option<String> {
        let details = self.vault.store.conflict_details(None).unwrap_or_default();
        let mut named = None;
        let names: Vec<(String, String)> = self
            .doc
            .as_ref()
            .map(|d| d.blocks().iter().filter(|l| l.conflict && l.conflict_with.is_none()).map(|l| l.id.clone()).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| {
                details.iter().find(|x| x.node == id && (x.kind == "text" || x.kind == "rehomed")).map(|x| (id, if x.kind == "rehomed" { crate::doc_ui::MOVED_HERE.to_string() } else { self.theirs_name(x) }))
            })
            .collect();
        if let Some(d) = self.doc.as_mut() {
            for (id, who) in names {
                if d.name_conflict(&id, &who) && who != crate::doc_ui::MOVED_HERE {
                    named = Some(who);
                }
            }
        }
        named
    }

    /// The 1.5 s idle save (text only) and leaving a line (a full save of the others).
    /// The editor's clock, given before input and on every tick (the model reads none).
    /// The editor's clock is the UI's logical clock (UiState::now_ms, moved by ticks), so a
    /// trace replays the same typing runs, undo steps and idle saves.
    pub fn clock_tick(&mut self) {
        // The document's clock; node ids for what the next edits make come back as an effect
        // (the model mints none).
        crate::runtime_effects::dispatch(self, crate::update::Msg::DocClock { now_ms: self.ui.now_ms });
    }

    /// The runtime's idle step for the open document: the clock, saves that came back, and the
    /// idle save. True when the idle point passed (the typing so far became one undo step, and
    /// it saved what there was): the session records that as an `idle` message, so a replay
    /// does the same at the same point.
    pub fn doc_tick(&mut self) -> bool {
        self.clock_tick();
        self.drain_saves(false);
        let main = self.idle_step();
        let panels = self.panels_idle();
        main || panels
    }

    /// The open document's idle point: true when it passed (see `doc_tick`).
    pub(crate) fn idle_step(&mut self) -> bool {
        let Some(d) = self.doc.as_mut() else { return false };
        if !d.idle_elapsed(Duration::from_millis(1500)) {
            return false;
        }
        let Some(op) = d.plan_idle() else { return true };
        let Some(root) = d.root.clone() else {
            self.save_doc(false);
            return true;
        };
        let today = self.ui.today;
        let id = d.caret_block().id.clone();
        let text = d.caret_block().text.clone();
        match self.vault.transact(move |st| outline::plan(st, &root, &[op], today)) {
            Ok((_, mut results)) => {
                outline::refresh_revs(&self.vault.store, &mut results);
                if let (Some(d), Some(b)) = (self.doc.as_mut(), results.first().and_then(|r| r.block.as_ref())) {
                    // Yours landed with its base: a remote text held for this line is moot.
                    d.idle_saved(&id, b.text_rev.clone(), text);
                }
                self.log_sizes = self.vault.log.files().unwrap_or_default();
            }
            Err(e) => self.error(format!("not saved: {e:#} · :retry")),
        }
        true
    }

    /// After a key in the document: leaving a line saves the lines left behind.
    pub fn doc_after_key(&mut self) {
        self.doc_after(true);
    }

    /// After a click in the document: as after a key, but the view stays where it is (the
    /// caret went where the pointer is, on screen; mouse.md "the mouse never scrolls").
    pub fn doc_after_click(&mut self) {
        self.doc_after(false);
        if let Some(d) = self.doc.as_mut() {
            d.hold_view();
        }
    }

    fn doc_after(&mut self, follow: bool) {
        self.near_miss_typed();
        if let Some(d) = self.doc.as_mut().filter(|_| follow) {
            d.follow_caret();
        }
        let Some(d) = self.doc.as_mut() else { return };
        let now = d.caret_block().id.clone();
        if self.ui.doc_line_id.as_deref() != Some(now.as_str()) {
            // The line left: a remote change waiting on it lands now (unless you typed on it:
            // then the save writes yours with its base and the core keeps both).
            if let Some(prev) = self.ui.doc_line_id.clone() {
                d.apply_held_text(&prev);
            }
            // Onto a line moved here (its parent was deleted elsewhere): the bar says why
            // (daemon.md §4.0a).
            let moved = d.caret_block().conflict_with.as_deref() == Some(crate::doc_ui::MOVED_HERE);
            self.doc_line_id = Some(now.clone());
            if moved {
                self.say_why_moved(&now);
            }
            // The keystroke reaches the screen first; the save runs right after the frame.
            self.doc_save_after_frame = true;
        }
    }

    /// `≠ moved here · its parent "Plan the offsite" was deleted on studio-mini · c review`.
    pub(crate) fn say_why_moved(&mut self, id: &str) {
        let Some(d) = self.vault.store.conflict_details(Some(id)).unwrap_or_default().into_iter().find(|d| d.kind == "rehomed") else { return };
        let s = &self.vault.store;
        let parent = d.other.as_ref().map(|o| s.node(&o.text).ok().flatten().map(|n| s.render_text(&n.label())).unwrap_or_else(|| o.text.clone())).unwrap_or_default();
        let dev = d.other.as_ref().map(|o| if o.dev == self.vault.device { "this device".to_string() } else { o.dev.clone() }).unwrap_or_default();
        let key = if self.doc_write { "⌃O" } else { "c" };
        self.info(format!("≠ moved here · its parent \"{parent}\" was deleted on {dev} · {key} review"));
    }

    /// After a frame: the save a line-leave deferred.
    /// What waits for the frame after a key: the save of a line just left. True when it ran
    /// (the session records that as a `frame` message, so a replay saves there too).
    pub fn after_frame(&mut self) -> bool {
        let panels = if self.in_panel.is_none() { self.panels_after_frame() } else { false };
        if !std::mem::take(&mut self.doc_save_after_frame) {
            return panels;
        }
        self.save_doc(false);
        // A line left with a shape change held for it: take it up now.
        if self.doc.as_ref().is_some_and(|d| {
            let caret = &d.caret_block().id;
            d.blocks().iter().any(|l| l.remote_shape && &l.id != caret && !l.edited())
        }) {
            self.patch_doc();
        }
        true
    }

    /// Changes made elsewhere: lines you aren't on and haven't edited update in place; lines
    /// gone from this document leave it. The caret never moves (tui-editor.md §9).
    pub(crate) fn patch_doc(&mut self) {
        if let Some(saver) = self.doc_saver.as_mut().filter(|s| s.pending > 0 || s.waiting.is_some()) {
            saver.patch = true;
            return;
        }
        let today = self.today;
        // A day opened before it existed, made since by another device (or an agent): take it
        // up, keeping what's typed here (two-device soak: the other device's notes never showed).
        if let Some(Target::Journal { date }) = self.doc.as_ref().filter(|d| d.root.is_none()).map(|d| d.target.clone()) {
            if self.vault.store.journal_node(&date.format("%Y-%m-%d").to_string()).ok().flatten().is_some() {
                self.reopen_keeping_unsaved();
            }
            return;
        }
        let Some(d) = self.doc.as_mut() else { return };
        let Some(root) = d.root.clone() else { return };
        let ids: Vec<String> = d.blocks().iter().filter(|l| !l.is_new).map(|l| l.id.clone()).collect();
        let Ok((blocks, gone)) = outline::render_ids(&self.vault.store, &root, &ids) else { return };
        let all = outline::render_for_editor(&self.vault.store, &root).ok();
        let patched = d.patch(blocks, gone, all.as_deref(), &root, today);
        if patched.reopen {
            self.reopen_keeping_unsaved();
            return;
        }
        if patched.announce.is_some() {
            self.doc_announce = patched.announce;
        }
        // A conflict that arrived with this refresh says who, too.
        if patched.unnamed_conflicts {
            self.fill_conflict_names();
        }
    }

    /// Navigate on a document line: the existing single-key commands act on its node.
    pub fn doc_select_line(&mut self) {
        let Some(d) = self.doc.as_ref() else { return };
        let l = d.caret_block();
        if l.is_new {
            // A line not saved yet has no node: commands have nothing to act on.
            self.selected = None;
            self.rows.clear();
            self.cursor = 0;
            return;
        }
        // The commands read rows[cursor]: point it at this line's node (it held the line
        // selected before, so x kept toggling that one).
        let id = l.id.clone();
        self.selected = Some(id.clone());
        self.row_vault.clear();
        self.rows = self.vault.store.node(&id).ok().flatten().filter(|n| !n.deleted).map(|n| vec![crate::app::App::node_row_public(n)]).unwrap_or_default();
        self.cursor = 0;
    }

    pub fn open_palette(&mut self) {
        self.overlay = Some(crate::app::Overlay::Palette { input: crate::input::LineInput::default(), sel: 0 });
    }

    /// ⌥[ / ⌥]: a day back or forward (from Write; the line is saved first).
    /// Before a save: a `[[Title]]` with no page that's within 2 edits of one (`Lisbn` /
    /// `Lisbon`) raises the near-miss chip for 3 s (writing.md §5). The save still makes the
    /// page; ⌃O while the chip shows rewrites the link and removes that page.
    fn check_near_miss(&mut self, all: bool) {
        let Some(d) = self.doc.as_ref() else { return };
        let s = &self.vault.store;
        let caret = d.caret().line;
        for (i, l) in d.blocks().iter().enumerate() {
            if (!all && i == caret) || !(l.is_new || l.edited()) || !l.text.contains("[[") {
                continue;
            }
            if self.near_miss.as_ref().is_some_and(|n| n.0 == l.id) {
                continue;
            }
            for title in links_in(&l.text) {
                if s.find_root_by_title(&title, false).ok().flatten().is_some() {
                    continue;
                }
                let titles = s.nodes_where("n.parent IS NULL AND n.title IS NOT NULL AND n.is_tag=0 AND n.deleted=0", &[]).unwrap_or_default();
                let near = titles.into_iter().filter_map(|p| p.title).filter(|t| t.to_lowercase() != title.to_lowercase()).map(|t| (edit_distance(&t.to_lowercase(), &title.to_lowercase()), t)).filter(|(e, _)| *e <= 2).min();
                if let Some((_, existing)) = near {
                    self.near_miss = Some((l.id.clone(), title, existing, None, self.ui.now_ms));
                    return;
                }
            }
        }
    }

    /// As soon as a link is closed (`]]`) on the caret's line, the same check (writing.md §5):
    /// the chip shows while you're still on the line, and ⌃O takes the page before any stub.
    pub fn near_miss_typed(&mut self) {
        let Some(d) = self.doc.as_ref() else { return };
        let l = d.caret_block();
        let caret = d.caret().byte;
        if !l.text[..caret].ends_with("]]") {
            return;
        }
        let s = &self.vault.store;
        let Some(title) = links_in(&l.text[..caret]).pop() else { return };
        if s.find_root_by_title(&title, false).ok().flatten().is_some() {
            return;
        }
        let titles = s.nodes_where("n.parent IS NULL AND n.title IS NOT NULL AND n.is_tag=0 AND n.deleted=0", &[]).unwrap_or_default();
        let near = titles.into_iter().filter_map(|p| p.title).filter(|t| t.to_lowercase() != title.to_lowercase()).map(|t| (edit_distance(&t.to_lowercase(), &title.to_lowercase()), t)).filter(|(e, _)| *e <= 2).min();
        if let Some((_, existing)) = near {
            self.near_miss = Some((l.id.clone(), title, existing, None, self.ui.now_ms));
        }
    }

    /// ⌃O on the near-miss chip: the link becomes the existing page, and the page the save just
    /// made goes (when nothing else links to it and it has nothing in it).
    fn take_near_miss(&mut self) -> bool {
        let Some((line_id, typed, existing, _, since)) = self.near_miss.clone() else { return false };
        if self.ui.age(since).as_secs() >= 3 {
            self.near_miss = None;
            return false;
        }
        self.near_miss = None;
        let Some(d) = self.doc.as_mut() else { return false };
        let Some(text) = d.blocks().iter().find(|l| l.id == line_id).map(|l| l.text.replace(&format!("[[{typed}]]"), &format!("[[{existing}]]"))) else { return false };
        d.replace_content(&line_id, &text);
        self.save_doc(true);
        let s = &self.vault.store;
        if let Some(stub) = s.find_root_by_title(&typed, false).ok().flatten() {
            let refs: i64 = s.conn.query_row("SELECT count(*) FROM edges e JOIN nodes n ON n.id=e.src WHERE e.dst=?1 AND e.rel='mention' AND n.deleted=0", [&stub], |r| r.get(0)).unwrap_or(1);
            let kids = s.children(&stub).map(|c| c.len()).unwrap_or(1);
            if refs == 0 && kids == 0 {
                let today = self.today;
                let _ = self.vault.transact(|st| {
                    let mut b = TxBuilder::new(st, today);
                    b.delete(&stub)?;
                    Ok((b.finish(), ()))
                });
            }
        }
        self.info(format!("linked ¶ {existing}"));
        true
    }

    /// ⌃O (writing.md §3): on the near-miss chip, take the existing page; on a `[[link]]`, open
    /// it (saving first); on a `≠` line, the compare; anywhere else, go to a page by name.
    /// Attach the clipboard's image, if it holds one, as a new line at the caret. False: no image.
    pub fn attach_clipboard_image(&mut self) -> bool {
        let Some(data) = clipboard_image() else { return false };
        let now = thc_core::dates::now_local();
        let name = format!("screenshot {}.png", now.format("%H%M"));
        self.attach_bytes(&data, &name, &format!("screenshot {}", now.format("%H:%M")), "⌃Z undo").is_some()
    }

    /// ⌘V: the clipboard read here, a local session's. An image is attached; text is
    /// pasted exactly as a bracketed paste would be.
    pub fn paste_system(&mut self) {
        if self.attach_clipboard_image() {
            return;
        }
        match clipboard_text() {
            Some(t) if !t.is_empty() => crate::doc_keys::paste(self, &t),
            _ => self.info("the clipboard is empty"),
        }
    }

    /// A dropped file (its path pasted): attached at once. The first ⌃Z turns it back into the
    /// pasted text.
    pub fn attach_dropped(&mut self, path: &std::path::Path, raw: &str) {
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) => return self.error(format!("can't read {}: {e}", path.display())),
        };
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        let caption = std::path::Path::new(&name).file_stem().and_then(|s| s.to_str()).unwrap_or("file").replace(['-', '_'], " ");
        if let Some(id) = self.attach_bytes(&data, &name, &caption, "⌃Z keep the path") {
            let depth = self.doc.as_ref().map_or(0, |d| d.undo_depth());
            self.last_drop = Some((id, raw.to_string(), depth));
        }
    }

    /// Store bytes as an attachment and add its line at the caret (on the caret's line when
    /// that's empty, else on a new line after it), one undo step, saved at once. The bar says
    /// `attached <caption> · W×H · <undo>`. The new line's id, or None when refused.
    pub fn attach_bytes(&mut self, data: &[u8], name: &str, caption: &str, undo: &str) -> Option<String> {
        let max = thc_core::attach::max_mb(&thc_core::settings::current());
        let st = match thc_core::attach::store_bytes(&self.vault.paths.vault, data, name, self.today, max) {
            Ok(s) => s,
            Err(e) => {
                self.error(format!("{e:#}").trim_start_matches("invalid: ").to_string());
                return None;
            }
        };
        let line = thc_core::attach::line(caption, &st.path);
        let d = self.doc.as_mut()?;
        // Its own note (a paragraph whose whole text is the image): on the caret's line when
        // that's empty, else a new line after it; then a fresh line below to type on. One undo
        // step.
        let id = d.insert_blocks(&[&line, ""]).swap_remove(0);
        self.save_doc(true);
        let dims = match (st.w, st.h) {
            (Some(w), Some(h)) => format!(" · {w}×{h}"),
            _ => format!(" · {}", crate::doc_ui::human_bytes(st.bytes)),
        };
        self.info(format!("attached {caption}{dims} · {undo}"));
        Some(id)
    }

    /// Open an attachment with the system's viewer (macOS `open`, Linux `xdg-open`).
    pub fn open_attachment(&mut self, path: &str) {
        let abs = self.vault.paths.vault.join(path);
        if !abs.exists() {
            return self.error(format!("▣ missing: {path} · thc doctor"));
        }
        // Snapshots and tests never launch anything.
        if crate::SNAPSHOT.with(|s| s.get()) {
            return self.info(format!("open {path} (skipped in snapshot)"));
        }
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        match std::process::Command::new(opener).arg(&abs).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn() {
            Ok(_) => self.info(format!("opened {}", path.rsplit('/').next().unwrap_or(path))),
            Err(e) => self.error(format!("can't open {path}: {e}")),
        }
    }

    pub fn doc_open(&mut self) {
        if self.take_near_miss() {
            return;
        }
        let Some(d) = self.doc.as_ref() else { return };
        let l = d.caret_block();
        // In a panel, a link is followed in the main view (sidebar.md §12).
        if self.in_panel.is_some() && crate::sidebar::policy::PANEL_CLICK_FOLLOWS_IN_MAIN && crate::doc_ui::image_line(&l.text).is_none() && !l.conflict {
            match link_at(&l.text, d.caret().byte) {
                Some(title) => self.panel_defer.push(crate::sidebar_app::Deferred::Follow(title)),
                None => self.panel_defer.push(crate::sidebar_app::Deferred::Action("finder.open".into())),
            }
            return;
        }
        // An attachment's line opens its file (attachments.md §3).
        if let Some((_, path)) = crate::doc_ui::image_line(&l.text) {
            return self.open_attachment(&path);
        }
        if l.conflict {
            self.selected = Some(l.id.clone());
            return self.open_compare();
        }
        let link = link_at(&l.text, d.caret().byte);
        // ⌃O on an issue's line (not on a link in it): the issue opens as its own document, and
        // Esc comes back here (issues.md §1).
        let issue = (link.is_none() && !l.is_new).then(|| l.id.clone()).and_then(|id| self.vault.store.node(&id).ok().flatten()).filter(|n| self.is_issue(n));
        let here = (d.target.clone(), l.id.clone());
        self.save_doc(true);
        // The saved line's id (a new line has its id only once it's saved).
        let here = (here.0, self.doc.as_ref().map_or(here.1, |d| d.caret_block().id.clone()));
        if let Some(n) = issue {
            self.doc_back = matches!(here.0, Target::Page { .. }).then(|| (here.0, here.1, n.id.clone()));
            self.page_open = Some(n.id.clone());
            self.set_view(crate::app::View::Pages);
            return;
        }
        match link {
            Some(title) => match self.vault.store.find_root_by_title(&title, false).ok().flatten() {
                Some(id) => {
                    // Esc is "up one level" (navigation.md §2, §7.4): back to where the chain of
                    // links began. From a list, that's the list (doc_origin); from a document with
                    // no list behind it (a day), that document, however many links on. ⌘[ is the
                    // page before (history).
                    if !matches!(&here.0, Target::Page { id: h, .. } if *h == id) {
                        let in_chain = matches!(&self.doc_back, Some((_, _, open)) if matches!(&here.0, Target::Page { id: h, .. } if h == open));
                        if in_chain {
                            if let Some(b) = self.doc_back.as_mut() {
                                b.2 = id.clone();
                            }
                        } else if self.doc_origin.is_none() {
                            self.doc_back = Some((here.0, here.1, id.clone()));
                        }
                    }
                    self.page_open = Some(id);
                    self.set_view(crate::app::View::Pages);
                }
                None => self.info(format!("no page ¶ {title} yet · it's made when the line saves")),
            },
            None => self.overlay = Some(crate::app::Overlay::Finder { input: crate::input::LineInput::default(), sel: 0 }),
        }
    }

    pub fn shift_journal(&mut self, delta: i64) {
        if self.view == View::Journal {
            self.journal_date += chrono::Duration::days(delta);
            self.selected = None;
            self.set_view(View::Journal);
        }
    }

    /// ⌥T: today's journal.
    pub fn journal_today(&mut self) {
        self.journal_date = self.today;
        self.selected = None;
        self.set_view(View::Journal);
    }

    // ---- the `[[` popup (tui-editor.md §5.1) ------------------------------------------------

    /// The open `[[` before the caret on its line: (byte where `[[` starts, the query typed).
    pub fn link_query(&self) -> Option<(usize, String)> {
        let d = self.doc.as_ref()?;
        let t = &d.caret_block().text[..d.caret().byte];
        let start = t.rfind("[[")?;
        let q = &t[start + 2..];
        (!q.contains("]]") && !q.contains('\n')).then(|| (start, q.to_string()))
    }

    /// Matches for the popup: pages (fuzzy, most recent first), then days; `(label, insert)`.
    /// The last element is the creation row when it's offered.
    pub fn link_matches(&self, q: &str) -> (Vec<(String, String)>, Option<String>) {
        let s = &self.vault.store;
        let ql = q.to_lowercase();
        let pages = s
            .nodes_where(&format!("n.parent IS NULL AND n.title IS NOT NULL AND n.is_tag=0 AND n.deleted=0 AND {} ORDER BY n.updated_ms DESC", thc_core::views::HIDDEN_SQL), &[])
            .unwrap_or_default();
        let mut out: Vec<(String, String)> = pages
            .into_iter()
            .filter_map(|p| p.title.clone())
            .filter(|t| ql.is_empty() || t.to_lowercase().contains(&ql))
            .take(6)
            .map(|t| (format!("¶ {t}"), t))
            .collect();
        // Days: today, yesterday, tomorrow, a weekday name (the coming one), a typed date.
        let today = self.today;
        let mut days: Vec<(&str, chrono::NaiveDate)> = vec![("today", today), ("yesterday", today - chrono::Duration::days(1)), ("tomorrow", today + chrono::Duration::days(1))];
        let names = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
        use chrono::Datelike;
        let mut owned: Vec<(String, chrono::NaiveDate)> = Vec::new();
        if ql.len() >= 3 {
            if let Some(i) = names.iter().position(|n| ql.starts_with(n)) {
                let ahead = (i as i64 - today.weekday().num_days_from_monday() as i64 + 7) % 7;
                owned.push((names[i].to_string(), today + chrono::Duration::days(ahead)));
            }
        }
        if let Ok(d) = chrono::NaiveDate::parse_from_str(q, "%Y-%m-%d") {
            owned.push((q.to_string(), d));
        }
        days.retain(|(w, _)| !ql.is_empty() && w.starts_with(&ql));
        for (w, d) in days.iter().map(|(w, d)| (w.to_string(), *d)).chain(owned) {
            if out.len() < 8 {
                let iso = d.format("%Y-%m-%d").to_string();
                out.push((format!("§ {w} · {}", d.format("%a %b %d").to_string().to_lowercase()), iso));
            }
        }
        let exact = out.iter().any(|(_, ins)| ins.eq_ignore_ascii_case(q.trim()));
        let create = (q.trim().chars().count() >= 2 && !exact).then(|| q.trim().to_string());
        (out, create)
    }

    /// The ⌃O finder's rows (writing.md §3): pages, fuzzy and most recent first; days by any date
    /// thc reads (`fri`, `oct 2`, `yesterday`, `+3d`); and `+ new page` when nothing is exactly it.
    pub fn finder_matches(&self, q: &str) -> Vec<(String, crate::app::Go)> {
        use crate::app::Go;
        use chrono::Datelike;
        let s = &self.vault.store;
        let q = q.trim();
        let ql = q.to_lowercase();
        let pages = s
            .nodes_where(&format!("n.parent IS NULL AND n.title IS NOT NULL AND n.is_tag=0 AND n.deleted=0 AND {} ORDER BY n.updated_ms DESC", thc_core::views::HIDDEN_SQL), &[])
            .unwrap_or_default();
        let mut scored: Vec<(i64, usize, String, String)> = pages
            .into_iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let t = p.title.clone()?;
                let score = if ql.is_empty() { Some(0) } else { crate::app::fuzzy(&ql, &t.to_lowercase()) };
                score.map(|sc| (sc, i, t, p.id))
            })
            .collect();
        if !ql.is_empty() {
            scored.sort_by_key(|(sc, i, _, _)| (-sc, *i));
        }
        let mut out: Vec<(String, Go)> = Vec::new();
        // Nothing typed: where you've been, most recent first, the previous document preselected
        // (⌃O Enter goes back); then today's journal; then pages.
        if ql.is_empty() {
            let current = self.doc.as_ref().map(|d| d.target.clone());
            let is_current = |t: &Target| match (t, &current) {
                (Target::Page { id: a, .. }, Some(Target::Page { id: b, .. })) => a == b,
                (a, Some(b)) => a == b,
                _ => false,
            };
            let mut listed_pages: Vec<String> = match &current {
                Some(Target::Page { id, .. }) => vec![id.clone()],
                _ => Vec::new(),
            };
            let mut today_listed = matches!(current, Some(Target::Journal { date }) if date == self.today);
            for t in self.recent_docs.iter().filter(|t| !is_current(t)) {
                match t {
                    Target::Journal { date } => {
                        today_listed |= *date == self.today;
                        out.push((format!("← § {}", date.format("%a %d %b").to_string().to_lowercase()), Go::Day(*date)));
                    }
                    Target::Page { id, .. } => {
                        if let Some(n) = s.node(id).ok().flatten().filter(|n| !n.deleted) {
                            listed_pages.push(id.clone());
                            out.push((format!("← ¶ {}", n.label()), Go::Page(id.clone())));
                        }
                    }
                }
            }
            if !today_listed {
                out.push(("§ today".to_string(), Go::Day(self.today)));
            }
            out.extend(scored.into_iter().filter(|(_, _, _, id)| !listed_pages.contains(id)).take(12).map(|(_, _, t, id)| (format!("¶ {t}"), Go::Page(id))));
            return out;
        }
        // A date first when the query reads as one.
        let mut days: Vec<chrono::NaiveDate> = Vec::new();
        if !ql.is_empty() {
            // Dates read forward (a deadline), but going to a day means the nearest one: `oct 2`
            // in October is this year's, and a bare weekday offers the past one as well.
            if let Ok(d) = thc_core::dates::parse(q, self.today) {
                let mut d = d.date();
                let has_year = q.split(|c: char| !c.is_ascii_digit()).any(|n| n.len() == 4);
                if !has_year && (d - self.today).num_days() > 182 {
                    d = d.with_year(d.year() - 1).unwrap_or(d);
                }
                days.push(d);
                let weekday = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"].iter().any(|w| ql.starts_with(w) && ql.chars().all(|c| c.is_ascii_alphabetic()));
                if weekday && d > self.today {
                    days.push(d - chrono::Duration::days(7));
                }
            }
            for (w, d) in [("today", self.today), ("yesterday", self.today - chrono::Duration::days(1)), ("tomorrow", self.today + chrono::Duration::days(1))] {
                if w.starts_with(&ql) && !days.contains(&d) {
                    days.push(d);
                }
            }
        }
        for d in days {
            let n = (d - self.today).num_days();
            let when = match n {
                0 => "today".to_string(),
                -1 => "yesterday".to_string(),
                1 => "tomorrow".to_string(),
                2..=13 => format!("in {n} days"),
                -13..=-2 => format!("{} days ago", -n),
                _ => String::new(),
            };
            let fmt = if d.year() == self.today.year() { "%a %b %-d" } else { "%a %b %-d %Y" };
            let label = if when.is_empty() { format!("§ {}", d.format(fmt)) } else { format!("§ {} · {when}", d.format(fmt)) };
            out.push((label, Go::Day(d)));
        }
        out.extend(scored.into_iter().take(12).map(|(_, _, t, id)| (format!("¶ {t}"), Go::Page(id))));
        let exact = out.iter().any(|(l, g)| matches!(g, Go::Page(_)) && l.trim_start_matches("¶ ").eq_ignore_ascii_case(q));
        // The guard: a new page needs a real name (two characters), never a stray key.
        if q.chars().count() >= 2 && !exact && out.iter().all(|(_, g)| !matches!(g, Go::Day(_))) {
            out.push((format!("+ new page \"{q}\""), Go::New(q.to_string())));
        }
        out
    }

    /// Go where a finder row points (saving first).
    pub fn finder_go(&mut self, g: crate::app::Go) {
        use crate::app::Go;
        self.save_doc(true);
        // Opened from a list (⌃O over Pages, Today…): Esc comes back to it, as Enter's would.
        self.remember_origin();
        match g {
            Go::Day(d) => {
                self.journal_date = d;
                self.selected = None;
                self.set_view(crate::app::View::Journal);
            }
            Go::Page(id) => {
                self.page_open = Some(id);
                self.set_view(crate::app::View::Pages);
            }
            Go::New(title) => {
                let mut pid = String::new();
                let out = &mut pid;
                match self.write_here(|b| {
                    *out = b.create_page(&title, &[])?;
                    Ok(())
                }) {
                    Ok(_) => {
                        self.page_open = Some(pid);
                        self.set_view(crate::app::View::Pages);
                    }
                    Err(e) => self.error(crate::app::friendly_error(&e)),
                }
            }
        }
    }

    /// Enter / Tab in the popup: replace `[[query` with `[[Title]]`.
    pub fn link_insert(&mut self, title: &str) {
        let Some((start, _)) = self.link_query() else { return };
        let Some(d) = self.doc.as_mut() else { return };
        d.replace_before_caret(start, &format!("[[{title}]]"));
        self.link_open = false;
        self.link_sel = None;
    }
}

/// The `[[Title]]` the caret is in (or touching), by title.
pub fn link_at(text: &str, caret: usize) -> Option<String> {
    // Quoted text is never parsed (writing.md §1): a quoted `[[…]]` isn't a link.
    let quoted = thc_core::capture::quoted_ranges(text);
    let in_quotes = |at: usize| quoted.iter().any(|(q0, q1)| at > *q0 && at < *q1);
    let mut from = 0;
    while let Some(a) = text[from..].find("[[").map(|k| from + k) {
        let b = text[a + 2..].find("]]").map(|k| a + 2 + k)?;
        if in_quotes(a) {
            from = b + 2;
            continue;
        }
        if caret >= a && caret <= b + 2 {
            let inner = &text[a + 2..b];
            return Some(inner.split('|').next_back().unwrap_or(inner).trim().to_string());
        }
        from = b + 2;
    }
    None
}

/// Whether `byte` is on a link's title, inside its `[[` and `]]` (a click there follows it).
pub fn on_link_title(text: &str, byte: usize) -> bool {
    link_title_range(text, byte).is_some()
}

/// The byte range of the link title `byte` is on, between its `[[` and `]]`.
pub fn link_title_range(text: &str, byte: usize) -> Option<std::ops::Range<usize>> {
    let quoted = thc_core::capture::quoted_ranges(text);
    let mut from = 0;
    while let Some(a) = text[from..].find("[[").map(|k| from + k) {
        let b = text[a + 2..].find("]]").map(|k| a + 2 + k)?;
        if !quoted.iter().any(|(q0, q1)| a > *q0 && a < *q1) && byte >= a + 2 && byte < b {
            return Some(a + 2..b);
        }
        from = b + 2;
    }
    None
}

/// Every `[[Title]]` in a line's text, by title.
pub fn links_in(text: &str) -> Vec<String> {
    let quoted = thc_core::capture::quoted_ranges(text);
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(a) = text[from..].find("[[").map(|k| from + k) {
        let Some(b) = text[a + 2..].find("]]").map(|k| a + 2 + k) else { break };
        if quoted.iter().any(|(q0, q1)| a > *q0 && a < *q1) {
            from = b + 2;
            continue;
        }
        let inner = &text[a + 2..b];
        out.push(inner.split('|').next_back().unwrap_or(inner).trim().to_string());
        from = b + 2;
    }
    out
}

/// Levenshtein distance, for "did you mean" (short titles).
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + (ca != *cb) as usize).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The clipboard's text (⌘V read by thc itself): macOS `pbpaste`, Linux `wl-paste` / `xclip`.
/// Tests and snapshots use `THC_CLIPBOARD_TEXT`, never the real clipboard.
fn clipboard_text() -> Option<String> {
    if let Some(t) = std::env::var_os("THC_CLIPBOARD_TEXT") {
        return Some(t.to_string_lossy().to_string());
    }
    if crate::SNAPSHOT.with(|s| s.get()) || std::env::var_os("THC_CLIPBOARD_IMAGE").is_some() {
        return None;
    }
    let tries: &[(&str, &[&str])] = if cfg!(target_os = "macos") { &[("pbpaste", &[])] } else { &[("wl-paste", &["--no-newline"]), ("xclip", &["-selection", "clipboard", "-o"])] };
    for (cmd, args) in tries {
        if let Ok(o) = std::process::Command::new(cmd).args(*args).output() {
            if o.status.success() {
                return Some(String::from_utf8_lossy(&o.stdout).to_string());
            }
        }
    }
    None
}

/// The clipboard's image as PNG bytes, if it holds one: macOS through `osascript` (the clipboard
/// as «class PNGf»), Linux `wl-paste` or `xclip`. Tests and snapshots never read the real
/// clipboard: `THC_CLIPBOARD_IMAGE=<file>` stands in for it.
fn clipboard_image() -> Option<Vec<u8>> {
    if let Some(f) = std::env::var_os("THC_CLIPBOARD_IMAGE") {
        return std::fs::read(f).ok().filter(|b| !b.is_empty());
    }
    if crate::SNAPSHOT.with(|s| s.get()) {
        return None;
    }
    let png = |b: Vec<u8>| (b.starts_with(b"\x89PNG")).then_some(b);
    if cfg!(target_os = "macos") {
        let tmp = std::env::temp_dir().join(format!("thc-clip-{}.png", std::process::id()));
        let script = format!(
            "try\nset d to the clipboard as «class PNGf»\nset f to open for access POSIX file \"{}\" with write permission\nset eof f to 0\nwrite d to f\nclose access f\non error\nreturn \"none\"\nend try",
            tmp.display()
        );
        let ok = std::process::Command::new("osascript").arg("-e").arg(&script).output().ok()?;
        if !ok.status.success() {
            return None;
        }
        let b = std::fs::read(&tmp).ok();
        let _ = std::fs::remove_file(&tmp);
        return b.and_then(png);
    }
    for (cmd, args) in [("wl-paste", &["--type", "image/png"][..]), ("xclip", &["-selection", "clipboard", "-t", "image/png", "-o"][..])] {
        if let Ok(o) = std::process::Command::new(cmd).args(args).output() {
            if o.status.success() {
                if let Some(b) = png(o.stdout) {
                    return Some(b);
                }
            }
        }
    }
    None
}

/// Where the caret is when a document opens (the arrival rule, in one place; rdfar, decided
/// 2026-10-08, option A of three):
/// - **Where you left it** (writing.md §1, "the caret remembers"): this document's caret on this
///   device, if its line is still here, with its scroll.
/// - **Otherwise ready to type:** a day or a page opens on a fresh line after its own notes, so
///   what you type right after jumping there is a note of its own. On a long page the view ends
///   with that line, the page above it filling the screen. `⌃End` goes there too.
///
/// The alternatives weighed: (B) arrive at the top and let the first key start a note above the
/// first one (the page shifts two rows at that key), (C) arrive at the top and type into the
/// first note (it glued: `helloGoals for the quarter`).
/// The scroll it returns is the remembered one, for the view's first layout: rows count as the
/// view's width wraps them, and a document just read has no width yet (vw384).
pub(crate) fn arrive(d: &mut Doc, remembered: Option<(String, usize, usize)>) -> Option<(usize, bool)> {
    d.caret_to_end(true);
    let (line, byte, scroll) = remembered?;
    // (The fresh line isn't needed when the caret goes back.)
    let i = d.restore_caret(&crate::editor::Anchor { id: line, byte }, true)?;
    Some((scroll.min(i), false))
}
