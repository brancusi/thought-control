#![recursion_limit = "256"]
mod cli;
mod next;
mod status_cmd;
mod lifecycle;
mod edits;
mod reads;
mod inspect;
mod capture_cmd;
mod alert_cmd;
mod early;
mod registry;
mod team_roles;
mod attachment_cmd;
mod ocr_cmd;
mod shot;
mod msg;
mod watch;
mod team_cmd;
mod team_host;
mod team_launch;
mod team_board;
mod wezterm;
use thc_tui::keys_edit;
mod daemon_cmd;
mod apply;
mod view_cmd;
mod vault_cmd;
mod out;
mod pre;
mod review_cmd;
mod schema;
mod instructions;
mod setup;
mod bootstrap;
mod update;

use anyhow::{Context, Result, anyhow};

use cli::{Cli, Cmd, ConflictCmd, PageCmd};
use out::{Out, node_json};
use serde_json::{Value, json};
use std::io::IsTerminal;
use thc_core::builder::TxBuilder;
use thc_core::error::{ThcError, invalid, usage};
use thc_core::event::{Actor, Event, Op};
use thc_core::model::Node;
use thc_core::vault::{self, Paths, Vault};
use thc_core::{capture, dates, edit, export, ingest};

/// Under tests (`THC_TEST` with `THC_TEST_PARENT`): exit when the test process that started
/// this one is gone, so no daemon or TUI outlives its test run (three leaked before).
fn test_watchdog() {
    let (Some(_), Ok(parent)) = (std::env::var_os("THC_TEST"), std::env::var("THC_TEST_PARENT")) else { return };
    let Ok(pid) = parent.parse::<i32>() else { return };
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        // SAFETY: kill(pid, 0) only checks that the process exists.
        if unsafe { libc::kill(pid, 0) } != 0 {
            std::process::exit(0);
        }
    });
}

fn main() {
    test_watchdog();
    // One clap command: the Cmd enum's and the registered ones (registry.rs).
    let matches = registry::command().get_matches();
    let cli = <Cli as clap::FromArgMatches>::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let reg = matches.subcommand().and_then(|(name, m)| registry::find(name).map(|s| (s, m.clone())));
    let json = cli.json;
    let code = match run(cli, reg) {
        Ok(()) => 0,
        Err(e) => report(&e, json),
    };
    std::process::exit(code);
}

fn report(e: &anyhow::Error, json: bool) -> i32 {
    // Already printed (e.g. `daemon status` offline): only the exit code matters.
    if e.downcast_ref::<daemon_cmd::Quiet>().is_some() {
        return e.downcast_ref::<ThcError>().map(|t| t.exit_code()).unwrap_or(1);
    }
    if let Some(ThcError::Stale { message, hint, node, rev, changed }) = e.downcast_ref::<ThcError>() {
        if json {
            eprintln!("{}", json!({ "error": { "kind": "stale", "message": message, "node": node, "rev": rev, "changed": changed } }));
        } else {
            eprintln!("thc: {message}\n{hint}");
        }
        return 4;
    }
    if let Some(ThcError::Refused { kind, message, hint, actor, tier, verb, config }) = e.downcast_ref::<ThcError>() {
        if json {
            let mut err = json!({ "kind": kind, "message": message, "actor": actor, "tier": tier, "verb": verb });
            if let Some(c) = config {
                err["config"] = json!(c);
            }
            eprintln!("{}", json!({ "error": err }));
        } else if hint.is_empty() {
            eprintln!("thc: {message}");
        } else {
            eprintln!("thc: {message}\n{hint}");
        }
        return 6;
    }
    if let Some(ThcError::Vetoed { hook, message }) = e.downcast_ref::<ThcError>() {
        if json {
            eprintln!("{}", json!({ "error": { "kind": "vetoed", "hook": hook, "message": message } }));
        } else {
            eprintln!("thc: vetoed by hook {hook}: {message} · nothing written");
        }
        return 6;
    }
    let (code, kind, candidates) = match e.downcast_ref::<ThcError>() {
        Some(t) => {
            let c = match t {
                ThcError::Ambiguous { candidates, .. } => Some(candidates.clone()),
                _ => None,
            };
            (t.exit_code(), t.kind(), c)
        }
        None => (1, "error", None),
    };
    if json {
        let mut err = json!({ "kind": kind, "message": format!("{e:#}") });
        if let Some(c) = candidates {
            err["candidates"] = json!(c);
        }
        eprintln!("{}", json!({ "error": err }));
    } else {
        eprintln!("thc: {e:#}");
    }
    code
}

fn parse_actor(s: Option<&str>) -> Actor {
    match s.map(str::trim).filter(|s| !s.is_empty()) {
        None | Some("human") => Actor { kind: "human".into(), name: None },
        Some(s) => match s.split_once(':') {
            Some((kind, name)) => Actor { kind: kind.into(), name: Some(name.into()) },
            None => Actor { kind: "agent".into(), name: Some(s.into()) },
        },
    }
}

/// Why this command uses its vault (`thc doctor`, `thc daemon status`).
pub(crate) static VAULT_SOURCE: std::sync::OnceLock<vault::VaultSource> = std::sync::OnceLock::new();

/// `{"kind", "file"?, "text"}` for --json output.
pub(crate) fn vault_source_json() -> Value {
    match VAULT_SOURCE.get() {
        Some(s) => {
            let mut v = json!({ "kind": s.kind(), "text": s.describe() });
            if let Some(f) = s.file() {
                v["file"] = json!(f);
            }
            v
        }
        None => Value::Null,
    }
}

/// "from /private/tmp/.thc.toml" when a project config chose the vault: prime, today and the TUI
/// name it so a stray one up the tree can't go unnoticed. None for the global config.
pub(crate) fn project_source() -> Option<String> {
    VAULT_SOURCE.get().filter(|s| matches!(s, vault::VaultSource::Project(_))).map(|s| s.describe())
}

struct Ctx {
    vault: Vault,
    board: Option<thc_core::board::Board>,
    out: Out,
    dry_run: bool,
    limit: usize,
    pre: pre::Pre,
    context: ContextState,
    /// Global --yes (policy confirm, prompts).
    yes: bool,
    /// --readonly (THC_READONLY is read by the policy itself).
    readonly: bool,
    /// The write verbs of this command (policy.md §2.1); empty for reads.
    verbs: Vec<String>,
    /// Loaded on the first write only: reads never touch device config (SPEC §8).
    policy: Option<thc_core::policy::Policy>,
    /// A system write (seeding ¶ Views): no tier check.
    system: bool,
}

/// The context for this command (views.md §2): where it came from, and whether it applies.
/// People get it on human output; JSON and agents only when they ask (--context / THC_CONTEXT).
struct ContextState {
    resolved: Option<thc_core::context::Context>,
    applies: bool,
    device: Option<String>,
    active: Option<thc_core::context::Active>,
    /// What this command's filtering removed; None when no listing was filtered (search, pages).
    hidden: std::cell::Cell<Option<usize>>,
}

impl Ctx {
    fn store(&self) -> &thc_core::store::Store {
        &self.vault.store
    }

    /// Filter by the context when it applies; returns (kept, total before).
    fn cfilter(&self, nodes: Vec<Node>) -> (Vec<Node>, usize) {
        match &self.context.active {
            Some(a) => {
                let (kept, total) = a.filter(nodes, |n| n.id.as_str());
                let h = &self.context.hidden;
                h.set(Some(h.get().unwrap_or(0) + total - kept.len()));
                (kept, total)
            }
            None => {
                let n = nodes.len();
                (nodes, n)
            }
        }
    }

    /// `context @work · showing 4 of 13 open · thc context none` (never a hidden filter).
    /// Returns true when it said the context hid everything (the caller skips its own empty line).
    fn context_line(&mut self, shown: usize, total: usize, what: &str, extra: &str) -> bool {
        if self.out.json {
            return false;
        }
        let Some(a) = &self.context.active else { return false };
        let name = format!("@{}", a.ctx.name);
        if extra.contains("search ignores") {
            let head = self.out.dim("context ");
            let rest = self.out.dim(" is on · search ignores contexts");
            self.out.line(format!("{head}{name}{rest}"));
            return false;
        }
        if shown == 0 && total > 0 {
            // Not "showing 0 of 3" followed by "nothing due": one sentence that says both.
            let w = if what.is_empty() { "nothing matches".to_string() } else { format!("nothing {what}") };
            let rest = self.out.dim(&format!(" · {total} more without it · thc context none"));
            self.out.line(format!("{w} in {name}{rest}"));
            return true;
        }
        let rest = self.out.dim(&format!(" · showing {shown} of {total}{} · thc context none{extra}", if what.is_empty() { String::new() } else { format!(" {what}") }));
        let head = self.out.dim("context ");
        self.out.line(format!("{head}{name}{rest}"));
        false
    }

    /// Capture defaults from the context, when it applies: (text with default tags, default
    /// parent, the context's name). Typed `-#tag`, --inbox, --under and --journal override them.
    fn capture_defaults(&self, text: &str, explicit_parent: bool) -> Result<(String, Option<String>, Option<String>)> {
        // The vault's capture target (vaults.md §9): where a capture lands when nothing names a
        // place and no context does (`· under ¶ Issues · vault setting`).
        let vault_target = || -> Option<(String, Option<String>, Option<String>)> {
            if explicit_parent {
                return None;
            }
            let (id, _) = thc_core::settings::capture_target(&thc_core::settings::current(), self.store())?;
            Some((text.to_string(), Some(id), Some("vault setting".to_string())))
        };
        let Some(a) = self.context.active.as_ref().filter(|_| self.context.applies) else { return Ok(vault_target().unwrap_or((text.to_string(), None, None))) };
        let d = a.capture_defaults();
        if d.is_empty() {
            return Ok(vault_target().unwrap_or((text.to_string(), None, None)));
        }
        let (text, added) = d.apply(text);
        let parent = match (&d.under, explicit_parent) {
            (Some(u), false) => Some(match self.store().find_root_by_title(u, false)? {
                Some(id) => id,
                None => self.resolve(u)?,
            }),
            _ => None,
        };
        let used = !added.is_empty() || parent.is_some();
        // The echo names the parent too: `· under ¶ Acme · from context @work`.
        let from = used.then(|| match (&d.under, parent.is_some()) {
            (Some(u), true) => format!("{}\u{1}under ¶ {u}", a.ctx.name),
            _ => a.ctx.name.clone(),
        });
        Ok((text, parent, from))
    }

    /// `added n7n8h  Call the printer #work · from context @work`
    fn report_capture(&mut self, verb: &str, events: &[Event], id: &str, from: Option<String>) -> Result<()> {
        match (from, self.out.json) {
            (Some(from), false) => {
                let n = self.store().must_node(id)?;
                let l = self.out.node_line(self.store(), &n, 0, true);
                let (name, under) = match from.split_once('\u{1}') {
                    Some((n, u)) => (n.to_string(), format!(" · {u}")),
                    None => (from.clone(), String::new()),
                };
                let tail = if name == "vault setting" { self.out.dim(" · the vault's capture target") } else { self.out.dim(&format!("{under} · from context @{name}")) };
                self.out.line(format!("{verb} {l}{tail}"));
                Ok(())
            }
            _ => self.report_write(verb, events, &[id.to_string()]),
        }
    }

    /// `"context":{"name","applied","device"}` for JSON output.
    fn context_json(&self) -> Value {
        let name = self.context.resolved.as_ref().map(|c| c.name.clone()).or_else(|| self.context.device.clone());
        // Always present on listings so agents can rely on the key; `hidden` only when applied.
        match self.context.hidden.get().filter(|_| self.context.active.is_some()) {
            Some(h) => json!({ "name": name, "applied": true, "hidden": h, "device": self.context.device }),
            None => json!({ "name": name, "applied": false, "device": self.context.device }),
        }
    }

    fn resolve(&self, id: &str) -> Result<String> {
        // `acme/k7q2m` (vaults.md §1): this vault's own prefix is fine; another vault's node is
        // reached with --vault until cross-vault writes land.
        if let Some((v, rest)) = id.split_once('/').filter(|(v, _)| thc_core::registry::valid_name(v)) {
            let here = out::VAULT_NAME.get().cloned().unwrap_or_default();
            if v != here {
                return Err(thc_core::error::invalid(format!("{rest} is in the vault {v} · run it with --vault {v}")));
            }
            return self.vault.store.resolve(rest);
        }
        self.vault.store.resolve(id)
    }

    /// Run a write. In dry-run mode prints the ops and returns None.
    fn write<T>(&mut self, f: impl FnOnce(&mut TxBuilder) -> Result<T>) -> Result<Option<(Vec<Event>, T)>> {
        let today = self.out.today;
        let pre = self.pre.clone();
        if self.dry_run {
            // Dry runs are allowed for every tier (policy.md §2.1): they write nothing.
            let mut b = TxBuilder::new(&self.vault.store, today);
            f(&mut b)?;
            let ops = b.finish();
            pre::check(&self.vault.store, &ops, &pre)?;
            self.out.json(&json!({ "dry_run": true, "ops": ops }));
            return Ok(None);
        }
        let guard = self.guard()?;
        // The first write after the upgrade: today's attachment lines get their nodes (once per
        // vault, by thc, its own undoable transaction). Never stops the write it precedes.
        if !self.readonly && self.vault.readonly.is_none() {
            let _ = thc_core::attach::upgrade(&mut self.vault, false);
        }
        let (events, t) = self.vault.transact(|s| {
            let mut b = TxBuilder::new(s, today);
            let t = f(&mut b)?;
            let ops = b.finish();
            pre::check(s, &ops, &pre)?;
            guard.ops(&ops)?;
            Ok((ops, t))
        })?;
        Ok(Some((events, t)))
    }

    /// Read-only, then the tier, for this command (policy.md §1). The returned guard checks
    /// the classes its ops fall in (delete, move-root, apply-large) once they exist.
    fn guard(&mut self) -> Result<Guard> {
        if self.policy.is_none() {
            self.policy = Some(thc_core::policy::Policy::load(&self.vault.actor, self.readonly)?);
        }
        let mut policy = self.policy.clone().unwrap();
        if self.system {
            policy.tier = thc_core::policy::Tier::Full;
            policy.deny.clear();
            policy.confirm.clear();
        }
        let mut verbs = self.verbs.clone();
        if verbs.is_empty() {
            verbs.push("write".into());
        }
        policy.check(&verbs, self.yes)?;
        Ok(Guard { policy, verbs, yes: self.yes })
    }

    /// Standard write result: created/affected nodes.
    fn report_write(&mut self, verb: &str, events: &[Event], ids: &[String]) -> Result<()> {
        let nodes: Vec<Node> = ids.iter().filter_map(|i| self.store().node(i).ok().flatten()).collect();
        if self.out.json {
            let items: Vec<Value> = nodes.iter().map(|n| node_json(self.store(), n)).collect();
            let tx = events.first().map(|e| e.tx.clone());
            self.out.json(&json!({ "ok": true, "tx": tx, "events": events.len(), "nodes": items }));
        } else if events.is_empty() && nodes.is_empty() {
            self.out.line("no changes");
        } else {
            for n in &nodes {
                let l = self.out.node_line(self.store(), n, 0, true);
                self.out.line(format!("{verb} {l}"));
            }
            if nodes.is_empty() {
                self.out.line(format!("{verb} ({} events)", events.len()));
            }
        }
        Ok(())
    }

    fn list(&mut self, title: Option<&str>, nodes: &[Node], context: bool) {
        if self.out.json {
            return;
        }
        if let Some(t) = title {
            if nodes.is_empty() {
                return;
            }
            self.out.heading(t);
        }
        for n in nodes {
            let l = self.out.node_line(self.store(), n, 0, context);
            self.out.line(l);
        }
    }
}

fn run(mut cli: Cli, mut reg: Option<(&'static registry::Spec, clap::ArgMatches)>) -> Result<()> {
    // `--fields` implies JSON; with no value it lists the fields (as gh does).
    if let Some(f) = &cli.fields {
        if f.trim().is_empty() {
            let mut out = Out::new(cli.json);
            if cli.json {
                out.json(&json!({ "fields": schema::NODE_FIELDS.iter().map(|(n, d)| json!({ "name": n, "description": d })).collect::<Vec<_>>() }));
            } else {
                out.line("Fields for --fields (node objects):");
                for (n, d) in schema::NODE_FIELDS {
                    out.line(format!("  {n:<11} {}", out.dim(d)));
                }
            }
            out.flush();
            return Ok(());
        }
        // Unknown field names are a validation error, like query values (exit 6).
        let names: Vec<&str> = schema::NODE_FIELDS.iter().map(|(n, _)| *n).collect();
        for f in f.split(',').map(str::trim).filter(|x| !x.is_empty()) {
            if !names.contains(&f) {
                let hint = match thc_core::query::closest(f, &names) {
                    Some(c) => format!(" · did you mean {c}?"),
                    None => " · thc --fields lists them".to_string(),
                };
                return Err(thc_core::error::invalid(format!("unknown field \"{f}\"{hint}")));
            }
        }
        cli.json = true;
    }
    // Registered commands that run before any vault (setup, update, schema, keys…).
    if let Some(f) = reg.as_ref().and_then(|(s, _)| s.before) {
        return f(&cli, &reg.as_ref().unwrap().1);
    }
    if let Some(Cmd::Team(cmd)) = &cli.cmd { return team_cmd::run(&cli, cmd); }
    let interactive = std::io::stdout().is_terminal() && std::io::stdin().is_terminal();
    if cli.cmd.is_none() && reg.is_none() && !(interactive && !cli.json) {
        let spec = &reads::TODAY;
        reg = Some((spec, (spec.command)().get_matches_from(["today"])));
    }
    let cmd = cli.cmd.take().or(reg.as_ref().map(|_| Cmd::Registered)).unwrap_or(Cmd::Tui { focus: None, review: false, log: false });
    // `acme/k7q2m` (vaults.md §1): an id in another vault runs the command there, as
    // `--vault acme` would. Only commands whose arguments are ids (never text to capture).
    let mut vault_flag = cli.vault.clone();
    let takes_ids = reg.as_ref().is_some_and(|(s, _)| s.ids)
        || matches!(
            cmd,
            Cmd::Rewind { .. } | Cmd::Mv(_)
        );
    if vault_flag.is_none() && takes_ids {
        let reg = thc_core::registry::Registry::load();
        let named: std::collections::BTreeSet<String> = std::env::args()
            .skip(1)
            .filter_map(|a| {
                let (v, rest) = a.split_once('/')?;
                (thc_core::registry::valid_name(v) && rest.len() >= 4 && rest.chars().all(|c| c.is_ascii_alphanumeric()) && reg.find(v).is_some()).then(|| v.to_string())
            })
            .collect();
        if named.len() > 1 {
            return Err(thc_core::error::invalid(format!("these ids are in different vaults ({}) · one vault per command", named.into_iter().collect::<Vec<_>>().join(", "))));
        }
        if let Some(v) = named.into_iter().next() {
            vault_flag = reg.find(&v).map(|e| e.path.clone());
        }
    }
    let board_selection = if reg.as_ref().is_some_and(|(s, _)| s.board) || matches!(cmd, Cmd::Prime { role: Some(_) }) {
        Some(thc_core::board::resolve(cli.board.as_deref(), vault_flag.as_deref())?)
    } else { None };
    let resolved = match board_selection.as_ref().map(|b| Ok((b.paths.clone(), vault::VaultSource::Flag))).unwrap_or_else(|| Paths::resolve_with_source(vault_flag.as_deref())) {
        // The first run of thc on a person's machine sets it up once (setup.md §3.2), then runs
        // the command. Agents, CI, pipes and --json get the plain error instead.
        Err(e) if e.to_string().contains("no vault found") && bootstrap::first_run_allowed(cli.json) => {
            bootstrap::first_run()?;
            Paths::resolve_with_source(vault_flag.as_deref())
        }
        r => r,
    };
    let (paths, source) = resolved?;
    let _ = VAULT_SOURCE.set(source);
    // Registered commands that need only the vault's paths (daemon, config).
    if let Some(f) = reg.as_ref().and_then(|(s, _)| s.with_paths) {
        return f(&cli, &paths, &reg.as_ref().unwrap().1);
    }
    // The TUI's settings are layered with the vault's own (vaults.md §9): load them for this vault
    // before anything reads them. Captures read the vault's capture target, and so does `next`
    // (the queue). (Plain reads never need them.)
    if matches!(cmd, Cmd::J { .. } | Cmd::P { .. } | Cmd::Tui { .. } | Cmd::Prime { role: Some(_) }) || reg.as_ref().is_some_and(|(s, _)| s.settings) {
        thc_core::settings::init(Some(&paths.vault));
    }
    // `thc j [date]` / `thc p <page>`: the TUI, opened on that document in Write.
    let (cmd, open_at) = match cmd {
        // The mode: a flag, else [tui] journal / pages, else the built-in default.
        Cmd::J { date, focus, no_focus } => {
            let cfg = thc_core::tui_config::TuiConfig::load();
            let mode = if focus { "focus" } else if no_focus { "normal" } else if cfg.journal == thc_core::tui_config::Mode::Focus { "focus" } else { "normal" };
            (Cmd::Tui { focus: None, review: false, log: false }, Some(format!("journal:{}|{mode}", date.as_deref().unwrap_or("today"))))
        }
        Cmd::P { page, focus, no_focus } => {
            let cfg = thc_core::tui_config::TuiConfig::load();
            let mode = if focus { "focus" } else if no_focus { "normal" } else if cfg.pages == thc_core::tui_config::Mode::Focus { "focus" } else { "normal" };
            (Cmd::Tui { focus: None, review: false, log: false }, Some(format!("page:{page}|{mode}")))
        }
        c => (c, None),
    };
    if let Cmd::Tui { focus, review, log } = &cmd {
        // WezTerm's key module, if it was set up: current with this thc's keys (thc's own file,
        // the same content every time, so a snapshot may refresh it too).
        let _ = wezterm::refresh_module();
        let start = if *review { Some("review") } else if *log { Some("log") } else { open_at.as_deref() };
        // Snapshots replay keys, and keys write: by default they write to a scratch copy, so
        // `THC_TUI_KEYS=…` never changes a real vault. THC_TUI_SNAPSHOT_WRITE=1 writes for real.
        let snapshot = std::env::var_os("THC_TUI_SNAPSHOT").is_some();
        let real_writes = std::env::var("THC_TUI_SNAPSHOT_WRITE").is_ok_and(|v| v == "1");
        let (paths, origin) = if snapshot && !real_writes {
            if !paths.vault.join(vault::VAULT_MARKER).exists() {
                return Err(usage(format!("{} is not a vault (missing {}); run `thc init`", paths.vault.display(), vault::VAULT_MARKER)));
            }
            (vault::scratch_copy(&paths)?, Some(paths))
        } else {
            (paths, None)
        };
        let scratch = origin.as_ref().map(|_| paths.vault.parent().map(|p| p.to_path_buf()).unwrap_or_default());
        let mut vault = Vault::open(paths, parse_actor(cli.actor.as_deref()), "tui")?;
        vault.origin = origin;
        vault.source_note = project_source();
        if cli.readonly {
            vault.readonly = Some("--readonly");
        }
        let focus = match focus {
            Some(f) => Some(vault.store.resolve(f)?),
            None => None,
        };
        // THC_TUI_SNAPSHOT=120x32 [THC_TUI_KEYS="2j"] renders one frame as text (tests, design review).
        if let Ok(size) = std::env::var("THC_TUI_SNAPSHOT") {
            let (w, h) = size.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))).unwrap_or((80, 24));
            let keys = std::env::var("THC_TUI_KEYS").unwrap_or_default();
            let frame = thc_tui::snapshot(vault, w, h, &keys, focus.as_deref(), start);
            if let Some(dir) = scratch.filter(|d| d.starts_with(std::env::temp_dir())) {
                let _ = std::fs::remove_dir_all(dir);
            }
            print!("{}", frame?);
            return Ok(());
        }
        return thc_tui::run(vault, focus.as_deref(), start);
    }
    let via = if std::env::var_os("THC_VIA").is_some() { std::env::var("THC_VIA").unwrap() } else { "cli".into() };
    let mut vault = Vault::open(paths, parse_actor(cli.actor.as_deref()), &via)?;
    if cli.readonly {
        vault.readonly = Some("--readonly");
    }
    let mut out = Out::new(cli.json);
    out.fields = cli.fields.as_ref().map(|f| f.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect());
    // Which vault, by name (vaults.md §2, §10.2): in JSON always, and a first line on human
    // listings when it isn't home (never a hidden scope).
    let listing = reg.as_ref().is_some_and(|(s, _)| s.listing) || matches!(cmd, Cmd::Q { .. });
    {
        let reg = thc_core::registry::Registry::load();
        let name = reg.name_for(&vault.paths.vault);
        let src = VAULT_SOURCE.get().cloned();
        let _ = out::VAULT_NAME.set(name.clone());
        let _ = out::VAULT_JSON.set(json!({ "name": name, "source": src.as_ref().map(|s| s.kind()), "home": reg.is_home(&vault.paths.vault) }));
        if !cli.json && listing && !reg.vaults.is_empty() && !reg.is_home(&vault.paths.vault) {
            let open = vault.store.query("status:open", out.today, 1_000_000).map(|v| v.len()).unwrap_or(0);
            let how = match (&src, reg.home.as_deref()) {
                (Some(vault::VaultSource::Use(_)), Some(h)) => format!("thc vault use {h}"),
                (Some(s), _) => s.describe(),
                _ => String::new(),
            };
            let head = out.dim("vault ");
            let tail = out.dim(&format!(" · {open} open{}", if how.is_empty() { String::new() } else { format!(" · {how}") }));
            out.line(format!("{head}{name}{tail}"));
        }
    }
    let board = board_selection.as_ref().map(|b| b.finish(&vault.store)).transpose()?;
    let pre = pre::Pre { if_match: cli.if_match.clone(), expect: cli.expect.clone() };
    let resolved = thc_core::context::resolve(cli.context.as_deref(), &vault.paths.cache, &std::env::current_dir()?);
    let applies = resolved.as_ref().is_some_and(|c| c.source.explicit() || (!cli.json && vault.actor.kind == "human"));
    let device = thc_core::context::device(&vault.paths.cache);
    let active = match (&resolved, applies) {
        (Some(c), true) => match thc_core::context::Active::load(&vault.store, c.clone(), out.today) {
            Ok(a) => Some(a),
            Err(e) => {
                eprintln!("thc: warning: {}", format!("{e:#}").trim_start_matches("not found: "));
                None
            }
        },
        _ => None,
    };
    let context = ContextState { resolved, applies, device, active, hidden: Default::default() };
    let verbs = match &reg {
        Some((s, m)) => (s.verbs)(m),
        None => cmd_verbs(&cmd),
    };
    let mut ctx = Ctx { vault, board, out, dry_run: cli.dry_run, limit: cli.limit, pre, context, yes: cli.yes, readonly: cli.readonly, verbs, policy: None, system: false };
    let r = match &reg {
        Some((s, m)) => (s.run)(&mut ctx, m),
        None => dispatch(&mut ctx, cmd),
    };
    for w in &ctx.vault.warnings {
        eprintln!("thc: warning: {w}");
    }
    ctx.out.flush();
    // Skip closing SQLite: the last WAL connection's close checkpoints and removes files
    // (~0.3 ms on every command). Commits are already durable; the next open carries on.
    std::mem::forget(ctx);
    r
}

/// A command's policy, checked again against the ops it built.
pub struct Guard {
    policy: thc_core::policy::Policy,
    verbs: Vec<String>,
    yes: bool,
}

impl Guard {
    pub fn ops(&self, ops: &[Op]) -> Result<()> {
        self.ops_at(ops, None)
    }

    /// Same, naming the batch line in the refusal (`apply line 3`).
    pub fn ops_at(&self, ops: &[Op], at: Option<&str>) -> Result<()> {
        let head = self.verbs[0].as_str();
        // Undoing your own add deletes a node, and `view rm` deletes the view's node, but
        // those are covered by `undo` and `views`, not `delete`.
        let batch = matches!(head, "apply" | "import");
        let classes: Vec<String> =
            thc_core::policy::op_classes(ops, batch).into_iter().filter(|c| !(matches!(head, "undo" | "view") && c == "delete")).collect();
        if classes.is_empty() {
            return Ok(());
        }
        let mut all = self.verbs.clone();
        all.extend(classes);
        self.policy.check_at(&all, self.yes, at)
    }
}

/// The verbs a command writes as (docs/design/policy.md §2.1); empty for reads. Classes that
/// depend on the ops (`delete`, `move-root`, `apply-large`) are added in `Ctx::write`.
fn cmd_verbs(cmd: &Cmd) -> Vec<String> {
    let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    match cmd {
        Cmd::Mv(_) => v(&["mv"]),
        Cmd::Edit { .. } => v(&["edit"]),
        Cmd::Page(PageCmd::New { .. }) => v(&["page"]),
        Cmd::Undo { by: Some(_), .. } => v(&["undo", "undo-bulk"]),
        Cmd::Undo { .. } => v(&["undo"]),
        Cmd::Apply { .. } => v(&["apply"]),
        Cmd::Import { .. } => v(&["import"]),
        Cmd::Rewind { .. } => v(&["rewind"]),
        // Accept is human-only in the format too; agents can't revert others' work either.
        Cmd::Review { cmd: Some(_), .. } => v(&["review"]),
        Cmd::Conflict(ConflictCmd::Resolve { .. }) => v(&["conflict"]),
        Cmd::Ingest => v(&["ingest"]),
        // Not a write-tier verb: repairs are for people.
        Cmd::Doctor { fix: true } => v(&["doctor"]),
        _ => vec![],
    }
}

/// `thc parse`: the capture parser's preview. Pure, so it needs no vault (the panel and the
/// editor call it before one may exist).
fn parse_cmd(out: &mut Out, text: &[String]) -> Result<()> {
    let v = capture::preview(&joined(text), out.today)?;
    if out.json {
        out.json(&v);
        return Ok(());
    }
    let mut bits = vec![format!("{}{}", if v.get("status").is_some() { "[ ] " } else { "" }, v["text"].as_str().unwrap_or(""))];
    for (k, label) in [("scheduled", "sched"), ("due", "due")] {
        if let Some(d) = v.get(k).and_then(|d| d.as_str()) {
            bits.push(format!("{label} {d}"));
        }
    }
    if let Some(p) = v.get("priority").and_then(|p| p.as_str()) {
        bits.push(format!("!{p}"));
    }
    if let Some(r) = v.get("repeat").and_then(|r| r.as_str()) {
        bits.push(format!("↻ {r}"));
    }
    out.line(format!("→ {}", bits.join(" · ")));
    let why = v["bad_why"].as_array().cloned().unwrap_or_default();
    for (i, b) in v["bad"].as_array().into_iter().flatten().enumerate() {
        let reason = why.get(i).and_then(|w| w.as_str()).filter(|w| !w.is_empty()).unwrap_or("not a date or value thc reads");
        let l = out.dim(&format!("  {} · {reason} · it stays as text", b.as_str().unwrap_or("")));
        out.line(l);
    }
    Ok(())
}

fn joined(words: &[String]) -> String {
    words.join(" ")
}

fn dispatch(ctx: &mut Ctx, cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Tui { .. } | Cmd::J { .. } | Cmd::P { .. } => unreachable!(),
        Cmd::Prime { role: None } => prime(ctx)?,
        Cmd::Prime { role: Some(role) } => team_roles::prime(ctx, &role)?,
        Cmd::Team(_) => unreachable!("team commands dispatch before vault resolution"),
        Cmd::Registered => unreachable!("registered commands run through their Spec"),
        Cmd::Mv(m) => {
            let id = ctx.resolve(&m.id)?;
            let under = match &m.under {
                Some(u) => Some(ctx.resolve(u)?),
                None => None,
            };
            let after = match &m.after {
                Some(a) => Some(ctx.resolve(a)?),
                None => None,
            };
            let before = match &m.before {
                Some(a) => Some(ctx.resolve(a)?),
                None => None,
            };
            let (root, journal) = (m.root, m.journal.clone());
            if under.is_none() && !root && journal.is_none() && after.is_none() && before.is_none() {
                return Err(usage("say where: --under <id>, --journal <date>, --root, --after <id> or --before <id>"));
            }
            let current_parent = ctx.store().must_node(&id)?.parent;
            let r = ctx.write(|b| {
                let parent = if root {
                    None
                } else if let Some(j) = &journal {
                    Some(b.journal(dates::parse(j, b.today)?.date())?)
                } else if under.is_some() {
                    under.clone()
                } else {
                    current_parent.clone()
                };
                b.move_to(&id, parent, after.as_deref(), before.as_deref())
            })?;
            if let Some((ev, ())) = r {
                ctx.report_write("moved", &ev, &[id])?;
            }
        }
        Cmd::Edit { id } => edit_cmd(ctx, id)?,
        Cmd::Page(PageCmd::New { title, tag }) => {
            let title = joined(&title);
            let r = ctx.write(|b| b.create_page(&title, &tag))?;
            if let Some((ev, id)) = r {
                ctx.report_write("page", &ev, &[id])?;
            }
        }
        Cmd::Page(PageCmd::Ls) => {
            let nodes = ctx.store().nodes_where(
                &format!("n.parent IS NULL AND n.title IS NOT NULL AND n.is_tag=0 AND n.deleted=0 AND {} ORDER BY n.title COLLATE NOCASE LIMIT {}", thc_core::views::HIDDEN_SQL, ctx.limit),
                &[],
            )?;
            emit_list(ctx, "Pages", &nodes, false);
        }
        Cmd::Q { expr, as_of, explain } => {
            let q = joined(&expr);
            // `thc q @today`: a sectioned view runs its sections (vaults.md §3.2).
            if let Some(name) = q.trim().strip_prefix('@').filter(|n| !n.contains(char::is_whitespace)) {
                if let Some(v) = view_cmd::find_sectioned(ctx, name)? {
                    return sectioned_cmd(ctx, &v);
                }
            }
            // `vault:acme`, `vault:*`, `-vault:side` (vaults.md §3.1): across vaults.
            let (scope, rest) = thc_core::federated::split_scope(&q)?;
            if let Some(scope) = scope {
                if as_of.is_some() {
                    return Err(usage("--as-of reads one vault · leave out vault:"));
                }
                return federated_q(ctx, &scope, &rest, explain.is_some());
            }
            if let Some(mode) = explain {
                if as_of.is_some() {
                    return Err(usage("--explain doesn't combine with --as-of"));
                }
                return explain_cmd(ctx, &q, mode == "sql");
            }
            let nodes = match as_of {
                Some(w) => {
                    review_cmd::reject_bare_m(&w)?;
                    let ms = when_to_ms(&w, ctx.out.today)?;
                    let mem = ctx.vault.as_of(ms)?;
                    let nodes = thc_core::messages::query(&mem, &q, ctx.out.today, ctx.limit, &msg::identity(ctx))?;
                    if ctx.out.json {
                        let items: Vec<Value> = nodes.iter().map(|n| node_json(&mem, n)).collect();
                        ctx.out.json(&json!({ "as_of": out::ms_to_local(ms as i64), "count": items.len(), "items": items }));
                    } else {
                        for n in &nodes {
                            let l = ctx.out.node_line(&mem, n, 0, true);
                            ctx.out.line(l);
                        }
                    }
                    return Ok(());
                }
                None => thc_core::messages::query(ctx.store(), &q, ctx.out.today, ctx.limit, &msg::identity(ctx))?,
            };
            let (nodes, total) = ctx.cfilter(nodes);
            let all_hidden = ctx.context_line(nodes.len(), total, "", "");
            let (_, used) = thc_core::views::expand(ctx.store(), &q)?;
            if all_hidden && !ctx.out.json {
                return Ok(());
            }
            if let Some((by, sorted)) = thc_core::query::explain_for(&q, ctx.store(), ctx.out.today, &msg::identity(ctx)).ok().and_then(|e| e.group.map(|g| (g, !e.sorts.is_empty()))) {
                return grouped_list(ctx, &by, sorted, nodes, &used, &q);
            }
            if used.is_empty() {
                emit_list(ctx, "", &nodes, true);
            } else if ctx.out.json {
                let items: Vec<Value> = nodes.iter().map(|n| node_json(ctx.store(), n)).collect();
                let mut v = json!({ "count": items.len(), "items": items, "context": ctx.context_json(), "views": used.iter().map(|v| json!({ "name": v.name, "query": v.query })).collect::<Vec<_>>() });
                if used.len() == 1 && q.trim() == format!("@{}", used[0].name) {
                    v["view"] = json!({ "name": used[0].name, "query": used[0].query });
                }
                ctx.out.json(&v);
            } else {
                // One muted line naming what ran (a filter you didn't type is always visible).
                let names: Vec<String> = used.iter().map(|v| format!("@{}", v.name)).collect();
                let what = if used.len() == 1 && q.trim() == names[0] { format!("{} · {}", names[0], used[0].query) } else { format!("{q} · {}", used.iter().map(|v| format!("@{} = {}", v.name, v.query)).collect::<Vec<_>>().join(" · ")) };
                let head = ctx.out.dim(&format!("{what} · {}", nodes.len()));
                ctx.out.line(head);
                if nodes.is_empty() {
                    ctx.out.line("nothing matches");
                } else {
                    ctx.list(None, &nodes, true);
                }
            }
        }
        Cmd::Log { by: actor, since } => {
            let mut where_sql = String::from("1=1");
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![];
            if let Some(a) = &actor {
                params.push(Box::new(format!("%{a}%")));
                where_sql.push_str(&format!(" AND actor LIKE ?{}", params.len()));
            }
            if let Some(s) = &since {
                review_cmd::reject_bare_m(s)?;
                let d = dates::parse_duration(s)?;
                let ms = chrono::Utc::now().timestamp_millis() - d.num_milliseconds().abs();
                params.push(Box::new(ms));
                where_sql.push_str(&format!(" AND ms >= ?{}", params.len()));
            }
            where_sql.push_str(&format!(" ORDER BY okey DESC LIMIT {}", ctx.limit * 4));
            let p: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
            let h = ctx.store().history_where(&where_sql, &p)?;
            if ctx.out.json {
                ctx.out.json(&json!({ "events": h }));
            } else {
                let mut last_tx = String::new();
                for e in &h {
                    if e.tx != last_tx {
                        let when = out::ms_to_local(e.ms).replace('T', " ");
                        let head = format!("{}  {}  via {}  tx {}", when, e.actor, e.via, &e.tx[e.tx.len() - 6..]);
                        let head = ctx.out.bold(&head);
                        ctx.out.line(head);
                        last_tx = e.tx.clone();
                    }
                    let short = if e.entity.len() >= 5 { ctx.store().short(&e.entity) } else { e.entity.clone() };
                    ctx.out.line(format!("    {:<14} {:<6} {}", e.op, short, summarize(&e.body)));
                }
            }
        }
        Cmd::Context { name } => context_cmd(ctx, name)?,
        Cmd::Apply { file } => {
            let yes = ctx.yes;
            apply::apply(ctx, &file, yes)?
        }
        Cmd::Import { file, under, journal, page } => apply::import(ctx, &file, under, journal, page)?,
        Cmd::Review { cmd, by, since } => review_cmd::review(ctx, cmd, by, since)?,
        Cmd::Rewind { id, to } => {
            let yes = ctx.yes;
            review_cmd::rewind(ctx, &id, &to, yes)?
        }
        Cmd::Undo { by: Some(by), since: Some(since), force, .. } => {
            let yes = ctx.yes;
            review_cmd::undo_bulk(ctx, &by, &since, yes, force)?
        }
        Cmd::Undo { tx, .. } => {
            let tx = match tx {
                Some(t) => {
                    let found: Option<String> = ctx
                        .store()
                        .conn
                        .query_row("SELECT tx FROM events WHERE tx LIKE ?1 ORDER BY okey DESC LIMIT 1", [format!("%{t}")], |r| r.get(0))
                        .ok();
                    found.ok_or_else(|| thc_core::error::not_found(format!("transaction {t}")))?
                }
                None => ctx
                    .store()
                    .conn
                    .query_row("SELECT tx FROM events ORDER BY okey DESC LIMIT 1", [], |r| r.get(0))
                    .map_err(|_| thc_core::error::not_found("nothing to undo"))?,
            };
            let inverse = thc_core::review::undo_ops(ctx.store(), &tx)?;
            if inverse.is_empty() {
                return Err(invalid("that transaction has nothing to undo"));
            }
            // Undoing someone else's transaction is its own verb class (policy.md §2.1).
            let by: String = ctx.store().conn.query_row("SELECT actor FROM events WHERE tx=?1 LIMIT 1", [&tx], |r| r.get(0)).unwrap_or_default();
            if by != ctx.vault.actor.label() && !(by == "human" && ctx.vault.actor.kind == "human") {
                ctx.verbs.push("undo-others".into());
            }
            let r = ctx.write(|b| {
                b.ops.extend(inverse);
                Ok(())
            })?;
            if let Some((ev, ())) = r {
                if ctx.out.json {
                    ctx.out.json(&json!({ "ok": true, "undone": tx, "tx": ev.first().map(|e| &e.tx), "events": ev.len() }));
                } else {
                    ctx.out.line(format!("undid tx {} ({} change(s)); `thc undo` again to redo", &tx[tx.len() - 6..], ev.len()));
                }
            }
        }
        Cmd::Conflict(ConflictCmd::Ls) => {
            let details = ctx.store().conflict_details(None)?;
            if ctx.out.json {
                let items: Vec<Value> = details.iter().map(|d| conflict_json(ctx, d)).collect();
                ctx.out.json(&json!({ "conflicts": items }));
            } else if details.is_empty() {
                ctx.out.line("no conflicts");
            } else {
                for (i, d) in details.iter().enumerate() {
                    if i > 0 {
                        ctx.out.line("");
                    }
                    for l in conflict_block(ctx, d, false) {
                        ctx.out.line(l);
                    }
                }
            }
        }
        Cmd::Conflict(ConflictCmd::Resolve { id, keep, delete }) => {
            let id = ctx.resolve(&id)?;
            let details = ctx.store().conflict_details(Some(&id))?;
            let Some(d) = details.first().cloned() else {
                return Err(thc_core::error::not_found(format!("{} has no open conflict", ctx.store().short(&id))));
            };
            let short = ctx.store().short(&id);
            if d.kind == "rehomed" {
                let under = d.current.as_ref().map(|c| c.text.clone()).filter(|t| !t.is_empty());
                let place = under.as_deref().map(|u| place_label(ctx, u)).unwrap_or_else(|| "the inbox".into());
                let dev = d.other.as_ref().map(|o| if o.dev == ctx.vault.device { "this device".to_string() } else { o.dev.clone() }).unwrap_or_default();
                if delete {
                    let r = ctx.write(|b| b.delete(&id).map(|_| ()))?;
                    if let Some((ev, ())) = r {
                        if ctx.out.json {
                            ctx.out.json(&json!({ "ok": true, "tx": ev.first().map(|e| &e.tx), "deleted": id }));
                        } else {
                            ctx.out.line(format!("deleted {short} (as {dev} meant){}", ctx.out.dim(&format!(" · thc restore {short} brings it back"))));
                        }
                    }
                    return Ok(());
                }
                if keep.as_deref() != Some("here") {
                    return Err(usage(format!("{short} was moved here because its parent was deleted · --keep here, or --delete")));
                }
                // Keeping it here makes the move real: its parent becomes where it shows.
                let n = ctx.store().must_node(&id)?;
                let ord = n.ord.clone();
                let r = ctx.write(|b| {
                    b.ops.push(Op::NodeMove { id: id.clone(), parent: under.clone(), order: ord });
                    Ok(())
                })?;
                if let Some((ev, ())) = r {
                    let tx = ev.first().map(|e| e.tx.clone()).unwrap_or_default();
                    if ctx.out.json {
                        ctx.out.json(&json!({ "ok": true, "tx": tx, "kept": "here", "under": under }));
                    } else {
                        let suffix = &tx[tx.len().saturating_sub(5)..];
                        ctx.out.line(format!("kept {short} under {place} · flag cleared{}", ctx.out.dim(&format!(" · thc undo --tx {suffix}"))));
                    }
                }
                return Ok(());
            }
            if delete {
                return Err(usage("--delete is for a line moved here (its parent deleted elsewhere)"));
            }
            if d.kind == "move" {
                let n = ctx.store().must_node(&id)?;
                let (parent, ord) = (n.parent.clone(), n.ord.clone());
                let r = ctx.write(|b| {
                    b.ops.push(Op::NodeMove { id: id.clone(), parent, order: ord });
                    Ok(())
                })?;
                if let Some((ev, ())) = r {
                    let place = n.parent.as_deref().map(|p| ctx.store().short(p)).unwrap_or_else(|| "inbox".into());
                    if ctx.out.json {
                        ctx.out.json(&json!({ "ok": true, "tx": ev.first().map(|e| &e.tx), "dismissed": id }));
                    } else {
                        ctx.out.line(format!("dismissed {short} · it stays under {place}"));
                    }
                }
                return Ok(());
            }
            let keep = match keep.as_deref() {
                Some("current") | Some("winner") => "current",
                Some("other") | Some("loser") => "other",
                Some("both") => "both",
                _ => return Err(usage("text conflicts need --keep current|other|both")),
            };
            let mut new_note = None;
            let nn = &mut new_note;
            let r = ctx.write(|b| {
                *nn = b.resolve_text_conflict(&id, keep, "current")?;
                Ok(())
            })?;
            if let Some((ev, ())) = r {
                let what = match keep {
                    "other" => "kept other".to_string(),
                    "both" => format!("kept both · new note {} below", new_note.as_deref().map(|n| ctx.store().short(n)).unwrap_or_default()),
                    _ => "kept current".to_string(),
                };
                if ctx.out.json {
                    ctx.out.json(&json!({ "ok": true, "tx": ev.first().map(|e| &e.tx), "kept": keep, "new_note": new_note }));
                } else {
                    ctx.out.line(format!("resolved {short} · {what} · undo with thc undo"));
                }
            }
        }
        Cmd::Export => {
            if ctx.out.json {
                let snap = export::json_snapshot(ctx.store())?;
                ctx.out.json(&snap);
            } else {
                let dir = ctx.vault.paths.vault.join("export");
                let s = export::markdown(ctx.store(), &dir, ctx.out.today)?;
                ctx.out.line(format!("exported {} page(s) and {} journal day(s) to {}", s.pages, s.journals, dir.display()));
            }
        }
        Cmd::Ingest => {
            let drop_dir = ctx.vault.paths.vault.join("drop");
            let files = ingest::claim(&drop_dir, &ctx.vault.device)?;
            let mut total = vec![];
            for f in &files {
                let path = f.clone();
                let r = ctx.write(|b| ingest::ingest_file(b, &path))?;
                if let Some((_, got)) = r {
                    total.extend(got.ids);
                    ingest::record_notices(&ctx.vault.paths.cache, &got.kept)?;
                    for k in &got.kept {
                        eprintln!("thc: {k}");
                    }
                    ingest::archive(f, &drop_dir)?;
                }
            }
            if ctx.out.json {
                ctx.out.json(&json!({ "ok": true, "files": files.len(), "created": total }));
            } else {
                ctx.out.line(format!("ingested {} file(s) into {} inbox item(s)", files.len(), total.len()));
            }
        }
        Cmd::Doctor { fix: false } => doctor(ctx)?,
        Cmd::Doctor { fix: true } => doctor_fix(ctx)?,
        Cmd::Rebuild => {
            let n = ctx.vault.rebuild()?;
            if ctx.out.json {
                ctx.out.json(&json!({ "ok": true, "events": n }));
            } else {
                ctx.out.line(format!("rebuilt store from {n} events"));
            }
        }
    }
    Ok(())
}

/// Alert JSON with the effective state (`pending|fired|snoozed|acked`) and this device's `fired_at`.
fn alert_json(store: &thc_core::store::Store, a: &thc_core::model::Alert) -> Value {
    let mut v = json!(a);
    v["state"] = json!(store.alert_state(a));
    v["fired_at"] = json!(store.fired_at(a));
    v
}

fn version_label(ctx: &Ctx, v: &thc_core::model::ConflictVersion) -> String {
    let actor = match v.actor.strip_prefix("agent:") {
        Some(a) => format!("◆ {a}"),
        None => v.actor.clone(),
    };
    let dev = if v.dev == ctx.vault.device { "this device".to_string() } else { v.dev.clone() };
    let when = out::ms_to_local(v.ms);
    format!("{actor} on {dev} · {}", when.get(11..).unwrap_or(&when))
}

/// The human block for a conflict (`conflict ls`, and `show` with `in_show`).
/// `¶ Offsite notes`, `§ Oct 4`, or a short id and its text for a line.
fn place_label(ctx: &Ctx, id: &str) -> String {
    let s = ctx.store();
    match s.node(id).ok().flatten() {
        Some(n) if n.journal.is_some() => format!("§ {}", n.journal.unwrap_or_default()),
        Some(n) if n.parent.is_none() && n.title.is_some() => format!("¶ {}", s.render_text(&n.label())),
        Some(n) => format!("{} \"{}\"", s.short(id), s.render_text(&n.label())),
        None => id.to_string(),
    }
}

fn conflict_block(ctx: &Ctx, d: &thc_core::model::ConflictDetail, in_show: bool) -> Vec<String> {
    let s = ctx.store();
    let short = s.short(&d.node);
    let mut out = Vec::new();
    let pad = if in_show { "    " } else { "         " };
    if d.kind == "rehomed" {
        // daemon.md §4.0a: `k7q2m  moved here  "Book the venue" · parent "Plan the offsite"
        // deleted on studio-mini 14:02 · now under ¶ Offsite notes`.
        let label = s.node(&d.node).ok().flatten().map(|n| s.render_text(&n.label())).unwrap_or_default();
        let parent = d.other.as_ref().map(|o| s.node(&o.text).ok().flatten().map(|n| s.render_text(&n.label())).unwrap_or_else(|| o.text.clone())).unwrap_or_default();
        let dev = d.other.as_ref().map(|o| if o.dev == ctx.vault.device { "this device".to_string() } else { o.dev.clone() }).unwrap_or_default();
        let at = d.other.as_ref().map(|o| out::ms_to_local(o.ms)).map(|t| t.get(11..16).unwrap_or("").to_string()).unwrap_or_default();
        let under = d.current.as_ref().map(|c| c.text.clone()).filter(|t| !t.is_empty()).map(|u| place_label(ctx, &u)).unwrap_or_else(|| "the inbox".into());
        let why = format!("parent \"{parent}\" deleted on {dev} {at} · now under {under}");
        if in_show {
            out.push(format!("  {} moved here · {why}", ctx.out.magenta("≠")));
        } else {
            out.push(format!("{} {short}  moved here  \"{label}\" · {why}", ctx.out.magenta("≠")));
        }
        out.push(ctx.out.dim(&format!("{pad}thc conflict resolve {short} --keep here   or   --delete")));
        return out;
    }
    if d.kind == "move" {
        let label = s.node(&d.node).ok().flatten().map(|n| s.render_text(&n.label())).unwrap_or_default();
        let title = |id: &str| s.node(id).ok().flatten().map(|n| format!("\"{}\"", s.render_text(&n.label()))).unwrap_or_else(|| id.to_string());
        let target = d.other.as_ref().map(|o| title(&o.text)).unwrap_or_default();
        let who = d.other.as_ref().map(|o| if o.dev == ctx.vault.device { "this device".to_string() } else { o.dev.clone() }).unwrap_or_default();
        let parent = s.node(&d.node).ok().flatten().and_then(|n| n.parent).map(|p| title(&p)).unwrap_or_else(|| "the inbox".into());
        if in_show {
            out.push(format!("  {} conflict · a move was skipped", ctx.out.magenta("≠")));
        } else {
            out.push(format!("{} {short}  move  {label}", ctx.out.magenta("≠")));
        }
        out.push(format!("{pad}{who} moved it under {target}, which would loop."));
        out.push(format!("{pad}The move was skipped, so it stays under {parent}."));
        out.push(ctx.out.dim(&format!("{pad}thc conflict resolve {short}   (dismiss)")));
        return out;
    }
    let cur = d.current.as_ref();
    let oth = d.other.as_ref();
    let cur_text = cur.map(|c| s.render_text(&c.text)).unwrap_or_default();
    let oth_text = oth.map(|c| s.render_text(&c.text)).unwrap_or_default();
    if in_show {
        out.push(format!("  {} conflict · text edited on two devices", ctx.out.magenta("≠")));
        out.push(format!("    current  {cur_text}"));
        if let Some(c) = cur {
            out.push(ctx.out.dim(&format!("             {}", version_label(ctx, c))));
        }
        out.push(format!("    other    {oth_text}"));
        if let Some(o) = oth {
            out.push(ctx.out.dim(&format!("             {}", version_label(ctx, o))));
        }
        if let Some(b) = &d.base {
            out.push(ctx.out.dim(&format!("    base     {}", s.render_text(b))));
        }
    } else {
        out.push(format!("{} {short}  text  {cur_text}", ctx.out.magenta("≠")));
        if let Some(c) = cur {
            out.push(format!("{pad}current  {}", ctx.out.dim(&version_label(ctx, c))));
        }
        if let Some(o) = oth {
            out.push(format!("{pad}other    {}", ctx.out.dim(&version_label(ctx, o))));
        }
        out.push(format!("{pad}         {oth_text}"));
    }
    out.push(ctx.out.dim(&format!("{pad}thc conflict resolve {short} --keep current|other|both")));
    out
}

fn conflict_json(ctx: &Ctx, d: &thc_core::model::ConflictDetail) -> Value {
    let s = ctx.store();
    let ver = |v: &Option<thc_core::model::ConflictVersion>| {
        v.as_ref().map(|v| {
            json!({
                "text": if d.kind == "text" { s.render_text(&v.text) } else { v.text.clone() },
                "actor": v.actor, "dev": v.dev, "at": (v.ms > 0).then(|| out::ms_to_local(v.ms)), "eid": v.eid,
            })
        })
    };
    let since = [d.current.as_ref(), d.other.as_ref()].iter().flatten().map(|v| v.ms).max().map(out::ms_to_local);
    let mut v = json!({ "node": d.node, "kind": d.kind, "since": since, "current": ver(&d.current), "other": ver(&d.other), "base": d.base.as_deref().map(|b| s.render_text(b)) });
    if d.kind == "rehomed" {
        let title = |id: &str| s.node(id).ok().flatten().map(|n| s.render_text(&n.label()));
        if let Some(o) = &d.other {
            v["deleted_parent"] = json!({ "id": o.text, "title": title(&o.text), "device": o.dev, "at": out::ms_to_local(o.ms) });
        }
        let under = d.current.as_ref().map(|c| c.text.clone()).filter(|t| !t.is_empty());
        v["now_under"] = json!({ "id": under, "title": under.as_deref().and_then(title) });
    }
    v
}

/// A sectioned view (`@today` edited, `thc q @name`): each section's query across the view's
/// scope (the current vault only for an agent), merged in its own order, rows naming their
/// vault when more than one answered. JSON: `sections[]`, plus `overdue`, `today` and `done` when
/// sections of those names exist (what `thc today --json` always had).
fn sectioned_cmd(ctx: &mut Ctx, v: &thc_core::views::Sectioned) -> Result<()> {
    use thc_core::federated;
    let reg = thc_core::registry::Registry::load();
    let current = out::VAULT_NAME.get().cloned().unwrap_or_default();
    let agent = ctx.vault.actor.kind == "agent";
    // The vaults in scope: opened here (the current one is ctx's).
    let entries: Vec<thc_core::registry::Entry> = match (&v.scope, agent) {
        (Some(sc), false) => {
            let (scope, _) = federated::split_scope(sc)?;
            match scope {
                Some(scope) => federated::resolve(&scope, &reg, &current)?.0,
                None => vec![],
            }
        }
        _ => vec![],
    };
    let others: Vec<(String, Vault)> = entries
        .iter()
        .filter(|e| e.name != current)
        .filter_map(|e| Vault::open(Paths { vault: e.path.clone(), cache: vault::default_cache(&e.path) }, ctx.vault.actor.clone(), "cli").ok().map(|x| (e.name.clone(), x)))
        .collect();
    let here_in = entries.is_empty() || entries.iter().any(|e| e.name == current);
    let mut names: Vec<String> = Vec::new();
    let mut stores: Vec<&thc_core::store::Store> = Vec::new();
    if here_in {
        names.push(current.clone());
        stores.push(&ctx.vault.store);
    }
    for (n, x) in &others {
        names.push(n.clone());
        stores.push(&x.store);
    }
    let day = ctx.out.today;
    let rows = federated::run_sections(&v.sections, &stores, day, ctx.limit)?;
    let many = rows.iter().flatten().map(|(i, _)| *i).collect::<std::collections::BTreeSet<_>>().len() > 1;
    if ctx.out.json {
        let item = |(i, n): &(usize, Node)| {
            let mut j = node_json(stores[*i], n);
            j["vault"] = json!(names[*i]);
            j
        };
        let secs: Vec<Value> = v.sections.iter().zip(&rows).map(|(s, r)| json!({ "title": s.title, "query": s.query, "items": r.iter().map(item).collect::<Vec<_>>() })).collect();
        let mut out = json!({ "view": v.name, "edited": v.edited, "scope": v.scope, "sections": secs, "vaults": names });
        for (key, title) in [("overdue", "Overdue"), ("today", "Today"), ("done", "Done today")] {
            if let Some(k) = v.sections.iter().position(|s| s.title == title) {
                out[key] = json!(rows[k].iter().map(item).collect::<Vec<_>>());
            }
        }
        ctx.out.json(&out);
        return Ok(());
    }
    ctx.out.line(ctx.out.dim(&format!("@{}{} · {}", v.name, if v.edited { " (yours)" } else { "" }, if names.len() > 1 { format!("{} vaults · {}", names.len(), names.join(", ")) } else { names.join("") })));
    let mut any = false;
    for (s, r) in v.sections.iter().zip(&rows) {
        if r.is_empty() {
            continue;
        }
        any = true;
        let h = if s.title == "Overdue" { ctx.out.red(&s.title) } else { ctx.out.bold(&s.title) };
        ctx.out.line(format!("{h} {}", ctx.out.dim(&r.len().to_string())));
        for (i, n) in r {
            let marker = many.then(|| ctx.out.dim(&names[*i]));
            let l = ctx.out.node_line_marked(stores[*i], n, 1, true, marker);
            ctx.out.line(l);
        }
    }
    if !any {
        ctx.out.line(ctx.out.dim("  nothing in any section"));
    }
    Ok(())
}

/// `thc q 'vault:… …'`: the query in each vault named, in parallel, merged in its own order.
/// When rows come from more than one vault, each says which (`acme · due fri`).
fn federated_q(ctx: &mut Ctx, scope: &thc_core::federated::Scope, rest: &str, explain: bool) -> Result<()> {
    use thc_core::federated;
    let reg = thc_core::registry::Registry::load();
    let current = out::VAULT_NAME.get().cloned().unwrap_or_default();
    let (entries, missing) = federated::resolve(scope, &reg, &current)?;
    let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    let read_line = format!("{} ({} of {} registered)", names.join(", "), names.len(), reg.vaults.len());
    if explain {
        if ctx.out.json {
            let e = thc_core::query::explain_for(rest, ctx.store(), ctx.out.today, &msg::identity(ctx)).map_err(|e| e.error)?;
            ctx.out.json(&json!({ "vaults": names, "missing": missing, "query": rest, "ast": e.ast, "sorts": e.sorts, "group": e.group }));
            return Ok(());
        }
        ctx.out.line(format!("{}{read_line}", ctx.out.dim("vault    ")));
        for m in &missing {
            ctx.out.line(ctx.out.dim(&format!("         {m} isn't on this device")));
        }
        return explain_cmd(ctx, rest, false);
    }
    let cur_paths = ctx.vault.paths.clone();
    let paths_for = |e: &thc_core::registry::Entry| {
        if e.name == current { cur_paths.clone() } else { Paths { vault: e.path.clone(), cache: vault::default_cache(&e.path) } }
    };
    let today = ctx.out.today;
    let parts = federated::run(&entries, &paths_for, &ctx.vault.actor, rest, today, ctx.limit)?;
    let order = match parts.first() {
        Some(p) => thc_core::query::compile(rest, &p.vault.store, today)?.order_sql,
        None => String::new(),
    };
    let group = parts.first().and_then(|p| thc_core::query::explain(rest, &p.vault.store, today).ok()).and_then(|e| e.group);
    if group.as_deref().is_some_and(|g| g != "vault") {
        return Err(usage(format!("group:{} works within one vault for now · group:vault groups across them", group.unwrap_or_default())));
    }
    let picked = if parts.is_empty() { vec![] } else { federated::merge(&parts, &order, ctx.limit)? };
    let span: std::collections::BTreeSet<usize> = picked.iter().map(|(p, _)| *p).collect();
    let many = span.len() > 1;
    let json_of = |pi: usize, ri: usize| {
        let p = &parts[pi];
        let mut v = node_json(&p.vault.store, &p.nodes[ri]);
        v["vault"] = json!(p.name);
        v
    };
    if ctx.out.json {
        let mut v = if group.is_some() {
            let groups: Vec<Value> = parts
                .iter()
                .enumerate()
                .filter(|(pi, _)| span.contains(pi))
                .map(|(pi, p)| {
                    let items: Vec<Value> = picked.iter().filter(|(x, _)| *x == pi).map(|(x, r)| json_of(*x, *r)).collect();
                    json!({ "key": p.name, "label": p.name, "count": items.len(), "items": items })
                })
                .collect();
            json!({ "group": "vault", "count": picked.len(), "groups": groups })
        } else {
            let items: Vec<Value> = picked.iter().map(|(p, r)| json_of(*p, *r)).collect();
            json!({ "count": items.len(), "items": items })
        };
        v["vault"] = json!({ "names": names, "missing": missing, "source": "query" });
        ctx.out.json(&v);
        return Ok(());
    }
    ctx.out.line(ctx.out.dim(&format!("vault {read_line}")));
    let line = |out: &Out, pi: usize, ri: usize| {
        let p = &parts[pi];
        let marker = (many && group.is_none()).then(|| out.dim(&p.name));
        out.node_line_marked(&p.vault.store, &p.nodes[ri], 0, true, marker)
    };
    if picked.is_empty() {
        ctx.out.line("nothing matches");
    } else if group.is_some() {
        for (i, (pi, p)) in parts.iter().enumerate().filter(|(pi, _)| span.contains(pi)).enumerate() {
            if i > 0 {
                ctx.out.line("");
            }
            let n = picked.iter().filter(|(x, _)| *x == pi).count();
            ctx.out.line(format!("{} {}", ctx.out.bold(&p.name), ctx.out.dim(&n.to_string())));
            for (x, r) in picked.iter().filter(|(x, _)| *x == pi) {
                let s = line(&ctx.out, *x, *r);
                ctx.out.line(s);
            }
        }
    } else {
        for (pi, ri) in &picked {
            let s = line(&ctx.out, *pi, *ri);
            ctx.out.line(s);
        }
    }
    for m in &missing {
        ctx.out.line(ctx.out.dim(&format!("{m} isn't on this device")));
    }
    Ok(())
}

/// `thc q '… group:parent'` (views.md §3.2): headings like Today's sections, items under them.
fn grouped_list(ctx: &mut Ctx, by: &str, sorted: bool, nodes: Vec<Node>, used: &[thc_core::views::View], q: &str) -> Result<()> {
    let count = nodes.len();
    let mut g = thc_core::group::group(ctx.store(), nodes, by, ctx.out.today, sorted)?;
    if by == "vault" {
        for x in g.groups.iter_mut() {
            x.label = out::VAULT_NAME.get().cloned().unwrap_or_else(|| x.label.clone());
            x.key = x.label.clone();
        }
    }
    if ctx.out.json {
        let groups: Vec<Value> = g
            .groups
            .iter()
            .map(|x| json!({ "key": x.key, "label": x.label, "count": x.items.len(), "items": x.items.iter().map(|n| node_json(ctx.store(), n)).collect::<Vec<_>>() }))
            .collect();
        let cj = ctx.context_json();
        ctx.out.json(&json!({ "group": by, "count": count, "groups": groups, "repeated": g.repeated, "context": cj }));
        return Ok(());
    }
    if !used.is_empty() {
        let head = ctx.out.dim(&format!("{q} · {} · {count}", used.iter().map(|v| format!("@{} = {}", v.name, v.query)).collect::<Vec<_>>().join(" · ")));
        ctx.out.line(head);
    }
    if g.groups.is_empty() {
        ctx.out.line("nothing matches");
        return Ok(());
    }
    for (i, grp) in g.groups.iter().enumerate() {
        if i > 0 {
            ctx.out.line("");
        }
        let label = if grp.overdue { ctx.out.red(&grp.label) } else { ctx.out.bold(&grp.label) };
        let n = ctx.out.dim(&grp.items.len().to_string());
        ctx.out.line(format!("{label}  {n}"));
        for node in &grp.items {
            // `in ¶ Q4 Planning` would repeat the heading; `in Draft Q4 OKRs` still says something.
            let repeats = by == "parent" && node.parent.as_deref() == Some(grp.key.as_str());
            let l = ctx.out.node_line(ctx.store(), node, 1, !repeats);
            ctx.out.line(l);
        }
    }
    if g.repeated {
        let note = ctx.out.dim("nodes with several tags appear once per tag");
        ctx.out.line("");
        ctx.out.line(note);
    }
    Ok(())
}

/// `thc q … --explain` (views.md §3.1): the query in plain words, what ran, and the SQL on request.
/// `thc keys --edit` (keymap.md §8.2a): the block in config.toml, made or refreshed, then
/// $EDITOR at it (or at `--context`'s table), then the result checked. `--print`: the block only.
fn keys_edit_cmd(context: Option<&str>, print: bool) -> Result<()> {
    if print {
        let bindings = thc_tui::keys_json();
        let file = thc_core::vault::global_config_path().ok_or_else(|| anyhow::anyhow!("no config path (HOME isn't set)"))?;
        let old = std::fs::read_to_string(file).unwrap_or_default();
        let next = keys_edit::with_block(&old, &bindings);
        let a = next.find(keys_edit::BEGIN).unwrap_or(0);
        let b = next.find(keys_edit::END).map_or(next.len(), |b| b + keys_edit::END.len());
        println!("{}", &next[a..b]);
        return Ok(());
    }
    println!("{}", keys_edit::edit(context)?);
    Ok(())
}

fn explain_cmd(ctx: &mut Ctx, q: &str, sql: bool) -> Result<()> {
    let today = ctx.out.today;
    let e = match thc_core::query::explain_for(q, ctx.store(), today, &msg::identity(ctx)) {
        Ok(e) => e,
        Err(err) => {
            // The parse error with a caret under the bad token, the same text as Tasks.
            let msg = err.error.to_string();
            let msg = msg.strip_prefix("invalid: ").unwrap_or(&msg).to_string();
            if ctx.out.json {
                let kind = err.error.downcast_ref::<ThcError>().map(|t| t.kind()).unwrap_or("error");
                eprintln!("{}", json!({ "error": { "kind": kind, "message": msg, "query": q, "span": err.span.map(|(a, b)| [a, b]) } }));
            } else {
                eprintln!("{}{q}", ctx.out.dim("query    "));
                match err.span {
                    Some((a, b)) => {
                        // Under the value when the field is fine (`status:opn` → `opn`).
                        let tok = &q[a..b];
                        let (a, b) = match tok.find([':', '=', '<', '>']) {
                            Some(i) if !msg.starts_with("bad field") && !msg.starts_with("unexpected") => {
                                let v = tok[i..].trim_start_matches([':', '=', '<', '>', '!']);
                                (b - v.len(), b)
                            }
                            _ => (a, b),
                        };
                        let pad = 9 + q[..a].chars().count();
                        eprintln!("{}{} {msg}", " ".repeat(pad), "^".repeat(q[a..b].chars().count().max(1)));
                    }
                    None => eprintln!("         {msg}"),
                }
            }
            return Err(err.error.context(daemon_cmd::Quiet));
        }
    };
    let t = std::time::Instant::now();
    let all = thc_core::messages::query(ctx.store(), q, today, 1_000_000, &msg::identity(ctx))?;
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let total = all.len();
    let (kept, _) = ctx.cfilter(all);
    let ctx_view = ctx.context.active.as_ref().map(|a| (a.ctx.name.clone(), a.view.query.clone()));
    let means = e.lines();
    let ms = (ms * 10.0).round() / 10.0;
    if ctx.out.json {
        let cj = ctx.context_json();
        ctx.out.json(&json!({
            "query": q,
            "means": means,
            "short": e.short(),
            "ast": e.ast,
            "sorts": e.sorts,
            "views": e.views.iter().map(|v| json!({ "name": v.name, "query": v.query })).collect::<Vec<_>>(),
            "sql": e.sql(),
            "matches": kept.len(),
            "ms": ms,
            "context": cj,
        }));
        return Ok(());
    }
    let label = |o: &Out, l: &str| o.dim(&format!("{l:<9}"));
    let l = label(&ctx.out, "query");
    ctx.out.line(format!("{l}{q}"));
    for v in &e.views {
        let l = label(&ctx.out, &format!("@{}", v.name));
        let body = ctx.out.dim(&format!("({})", v.query));
        ctx.out.line(format!("{l}{body}"));
    }
    for (i, m) in means.iter().enumerate() {
        let l = if i == 0 { label(&ctx.out, "means") } else { " ".repeat(9) };
        ctx.out.line(format!("{l}{m}"));
    }
    if let Some((name, vq)) = &ctx_view {
        let l = label(&ctx.out, "context");
        let rest = ctx.out.dim(&format!(" also applies ({vq})"));
        ctx.out.line(format!("{l}@{name}{rest}"));
    }
    let l = label(&ctx.out, "matches");
    let n = if ctx_view.is_some() && kept.len() != total { format!("{} of {total}", kept.len()) } else { kept.len().to_string() };
    let tail = ctx.out.dim(&format!(" · {ms} ms"));
    ctx.out.line(format!("{l}{n}{tail}"));
    let l = label(&ctx.out, "sql");
    if sql {
        ctx.out.line(format!("{l}{}", e.sql()));
    } else {
        let hint = ctx.out.dim("add --explain=sql to see it");
        ctx.out.line(format!("{l}{hint}"));
    }
    Ok(())
}

fn emit_list(ctx: &mut Ctx, title: &str, nodes: &[Node], context: bool) {
    if ctx.out.json {
        let items: Vec<Value> = nodes.iter().map(|n| node_json(ctx.store(), n)).collect();
        // A filter the reader didn't type is always visible, to agents too.
        let cj = ctx.context_json();
        ctx.out.json(&json!({ "count": items.len(), "items": items, "context": cj }));
    } else if nodes.is_empty() {
        ctx.out.line(if title.is_empty() { "nothing matches".to_string() } else { format!("{title}: nothing here") });
    } else {
        ctx.list(if title.is_empty() { None } else { Some(title) }, nodes, context);
    }
}

fn summarize(body: &Value) -> String {
    let mut parts = vec![];
    for k in ["title", "text", "props", "parent", "rel", "dst", "trigger", "next", "at", "verdict", "txs"] {
        if let Some(v) = body.get(k) {
            if v.is_null() {
                continue;
            }
            let s = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let s: String = s.chars().take(60).collect();
            parts.push(format!("{k}={s}"));
        }
    }
    parts.join(" ")
}

fn when_to_ms(w: &str, today: chrono::NaiveDate) -> Result<u64> {
    let dv = dates::parse(w, today)?;
    let dt = match dv {
        // End of that day, so "as of tuesday" includes tuesday's changes.
        dates::DateVal::Date(d) => d.and_hms_opt(23, 59, 59).unwrap(),
        dates::DateVal::DateTime(dt) => dt,
    };
    let local = dt.and_local_timezone(chrono::Local).single().ok_or_else(|| anyhow!("ambiguous local time"))?;
    Ok(local.timestamp_millis().max(0) as u64)
}

fn tree_json(ctx: &Ctx, nodes: &[Node], depth: usize) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for n in nodes {
        let mut v = node_json(ctx.store(), n);
        if depth > 0 {
            let kids = ctx.store().children(&n.id)?;
            if !kids.is_empty() {
                v["children"] = json!(tree_json(ctx, &kids, depth - 1)?);
            }
        }
        out.push(v);
    }
    Ok(out)
}

fn print_tree(ctx: &mut Ctx, n: &Node, indent: usize, depth: usize) -> Result<()> {
    let l = ctx.out.node_line(ctx.store(), n, indent, false);
    ctx.out.line(l);
    let text = ctx.store().render_text(&n.text);
    for extra in text.lines().skip(1) {
        let pad = "  ".repeat(indent);
        let l = ctx.out.dim(&format!("{pad}        {extra}"));
        ctx.out.line(l);
    }
    if depth > 0 {
        for c in ctx.store().children(&n.id)? {
            print_tree(ctx, &c, indent + 1, depth - 1)?;
        }
    }
    Ok(())
}

fn show(ctx: &mut Ctx, id: Option<String>, depth: usize) -> Result<()> {
    let id = match id {
        Some(i) => ctx.resolve(&i)?,
        None => {
            let key = ctx.out.today.format("%Y-%m-%d").to_string();
            match ctx.store().journal_node(&key)? {
                Some(j) => j,
                None => return Err(thc_core::error::not_found("today's journal is empty")),
            }
        }
    };
    let n = ctx.store().must_node(&id)?;
    let children = ctx.store().children(&id)?;
    let backlinks = ctx.store().backlinks(&id)?;
    let alerts = ctx.store().alerts_of(&id)?;
    // A task that's been started or done: its times (status.md §3.4).
    let times = match n.status.as_deref() {
        Some("doing" | "done") => thc_core::status::times(ctx.store(), std::slice::from_ref(&id))?.remove(&id),
        _ => None,
    };
    // "Now" honours THC_NOW, as `thc status` does, so the day shown and an open task's worked time agree.
    let now_ms = {
        use chrono::TimeZone;
        chrono::Local.from_local_datetime(&thc_core::dates::now_local()).earliest().map_or_else(|| chrono::Local::now().timestamp_millis(), |d| d.timestamp_millis())
    };
    if ctx.out.json {
        let mut v = node_json(ctx.store(), &n);
        if let Some(t) = &times {
            let mut tj = json!({ "filed": thc_core::status::rfc3339(t.filed) });
            if let Some(s) = t.started { tj["started"] = json!(thc_core::status::rfc3339(s)); }
            if let Some(d) = t.done.filter(|_| n.status.as_deref() == Some("done")) { tj["done"] = json!(thc_core::status::rfc3339(d)); }
            if let Some(w) = t.worked(now_ms) { tj["worked"] = json!(thc_core::status::minutes(w)); }
            if let Some(w) = t.waited() { tj["waited"] = json!(thc_core::status::minutes(w)); }
            if t.started.is_none() { tj["untracked"] = json!(true); }
            if t.reopened > 0 { tj["reopened"] = json!(t.reopened); }
            v["times"] = tj;
        }
        v["children"] = json!(tree_json(ctx, &children, depth.saturating_sub(1))?);
        v["backlinks"] = json!(backlinks.iter().map(|b| json!({"id": b.id, "label": b.label()})).collect::<Vec<_>>());
        v["alerts"] = json!(alerts.iter().map(|a| alert_json(ctx.store(), a)).collect::<Vec<_>>());
        let cd = ctx.store().conflict_details(Some(&id))?;
        if !cd.is_empty() {
            v["conflicts"] = json!(cd.iter().map(|d| conflict_json(ctx, d)).collect::<Vec<_>>());
        }
        let edges = ctx.store().edges_from(&id)?;
        let rels: Vec<Value> = edges.iter().filter(|(r, _)| r != "tag" && r != "mention" && r != "embed").map(|(r, d)| json!({"rel": r, "dst": d})).collect();
        if !rels.is_empty() {
            v["relations"] = json!(rels);
        }
        // Attachments (attachments.md §4): in the note's text and its children's, with `abs` so an
        // agent with file access reads the file itself.
        let vault = ctx.vault.paths.vault.clone();
        let mut atts = Vec::new();
        for (owner, text) in std::iter::once((n.id.clone(), n.text.clone())).chain(children.iter().map(|c| (c.id.clone(), c.text.clone()))) {
            for (caption, path) in thc_core::attach::refs(&text) {
                let st = thc_core::attach::describe(&vault, &path, None);
                // `id`: the file's attachment node (FORMAT.md "Attachments"), keyed from its path.
                let mut a = json!({ "node": owner, "id": thc_core::id::from_key(&format!("file:{path}")), "path": st.path, "abs": st.abs, "caption": caption, "bytes": st.bytes, "mime": st.mime, "missing": !st.abs.exists() });
                if let (Some(w), Some(h)) = (st.w, st.h) {
                    a["w"] = json!(w);
                    a["h"] = json!(h);
                }
                if let Some(p) = thc_core::attach::cached_thumbnail(&ctx.vault.paths.cache, &path) { a["thumbnail"] = json!(p); }
                let aid = thc_core::attach::node_id(&path);
                if let Some(ocr) = ctx.store().props_of(&aid)?.get("ocr") { a["ocr"] = ocr.clone(); }
                atts.push(a);
            }
        }
        if !atts.is_empty() {
            v["attachments"] = json!(atts);
        }
        // An attachment node: the file, and the notes that show it ("used in").
        let props = ctx.store().props_of(&id)?;
        if props.get("system").and_then(|s| s.as_str()) == Some("attachment") {
            if let Some(ocr) = props.get("ocr") { v["ocr"] = ocr.clone(); }
            if let Some(path) = props.get("path").and_then(|p| p.as_str()) {
                let st = thc_core::attach::describe(&vault, path, None);
                v["file"] = json!({ "kind": props.get("kind"), "path": path, "abs": st.abs, "bytes": st.bytes, "mime": st.mime, "w": st.w, "h": st.h, "missing": !st.abs.exists() });
                if let Some(p) = thc_core::attach::cached_thumbnail(&ctx.vault.paths.cache, path) { v["file"]["thumbnail"] = json!(p); }
            }
            let used: Vec<Value> = ctx.store().embedders(&id)?.iter().map(|e| json!({ "id": e.id, "short": ctx.store().short(&e.id), "text": ctx.store().render_text(&e.text) })).collect();
            v["embeds"] = json!(used);
        }
        ctx.out.json(&v);
        return Ok(());
    }
    print_tree(ctx, &n, 0, 0)?;
    if let Some(t) = &times {
        let tokens = thc_core::status::Tokens::of(&ctx.store().props_of(&id)?);
        let l = ctx.out.dim(&format!("  {}", thc_core::status::times_line(t, tokens.as_ref(), n.status.as_deref() != Some("done"), now_ms)));
        ctx.out.line(l);
    }
    // Blockers belong with the header, before the children.
    for b in &ctx.store().open_blockers(&id)? {
        let short = ctx.out.dim(&ctx.store().short(&b.id));
        ctx.out.line(format!("  blocked by {short} {}", ctx.store().render_text(&b.label())));
    }
    if depth > 0 {
        for c in &children {
            print_tree(ctx, c, 1, depth - 1)?;
        }
    }
    let props = ctx.store().props_of(&id)?;
    if !props.is_empty() {
        let s = props.iter().map(|(k, v)| format!("{k}={}", v.as_str().map(str::to_string).unwrap_or(v.to_string()))).collect::<Vec<_>>().join("  ");
        let l = ctx.out.dim(&format!("  props: {s}"));
        ctx.out.line(l);
    }
    for a in &alerts {
        let l = ctx.out.dim(&format!("  alert {} at {} ({})", ctx.store().short(&a.id), a.fire_at.clone().unwrap_or("-".into()).replace('T', " "), ctx.store().alert_state(a)));
        ctx.out.line(l);
    }
    for d in ctx.store().conflict_details(Some(&id))? {
        for l in conflict_block(ctx, &d, true) {
            ctx.out.line(l);
        }
    }
    let rels: Vec<(String, String)> = ctx.store().edges_from(&id)?.into_iter().filter(|(r, _)| r != "tag" && r != "mention").collect();
    for (rel, dst) in &rels {
        let label = ctx.store().node(dst)?.map(|n| ctx.store().render_text(&n.label())).unwrap_or_default();
        let l = ctx.out.dim(&format!("  {rel} {} {label}", ctx.store().short(dst)));
        ctx.out.line(l);
    }
    if !backlinks.is_empty() {
        ctx.out.heading("  linked from");
        for b in &backlinks {
            let l = ctx.out.node_line(ctx.store(), b, 2, true);
            ctx.out.line(l);
        }
    }
    let meta = ctx.out.dim(&format!(
        "  {} · created {} by {} · updated {}",
        n.id,
        out::ms_to_local(n.created_ms).replace('T', " "),
        n.created_by,
        out::ms_to_local(n.updated_ms).replace('T', " ")
    ));
    ctx.out.line(meta);
    Ok(())
}

fn edit_cmd(ctx: &mut Ctx, id: Option<String>) -> Result<()> {
    let id = match id {
        Some(i) => ctx.resolve(&i)?,
        None => {
            let key = ctx.out.today.format("%Y-%m-%d").to_string();
            match ctx.store().journal_node(&key)? {
                Some(j) => j,
                None => match ctx.write(|b| b.journal(b.today))? {
                    Some((_, j)) => j,
                    None => return Ok(()),
                },
            }
        }
    };
    let rendered = edit::render(ctx.store(), &id)?;
    let path = ctx.vault.paths.cache.join(format!("edit-{}.md", &id[..5]));
    std::fs::write(&path, &rendered.text)?;
    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "vi".into());
    let interactive = std::io::stdin().is_terminal();
    let mut attempt = 0;
    loop {
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("{editor} \"$1\""))
            .arg("thc-edit")
            .arg(&path)
            .status()
            .with_context(|| format!("launching editor {editor:?}"))?;
        if !status.success() {
            return Err(anyhow!("editor exited with {status}; nothing changed (file kept at {})", path.display()));
        }
        let edited = std::fs::read_to_string(&path)?;
        if edited == rendered.text {
            ctx.out.line("no changes");
            let _ = std::fs::remove_file(&path);
            return Ok(());
        }
        let result = ctx.write(|b| edit::apply(b, &rendered, &edited));
        match result {
            Ok(Some((ev, summary))) => {
                if ctx.out.json {
                    ctx.out.json(&json!({ "ok": true, "events": ev.len(), "summary": summary }));
                } else {
                    ctx.out.line(format!(
                        "saved: {} created, {} updated, {} moved, {} deleted",
                        summary.created, summary.updated, summary.moved, summary.deleted
                    ));
                }
                let _ = std::fs::remove_file(&path);
                return Ok(());
            }
            Ok(None) => return Ok(()),
            Err(e) if interactive && attempt < 3 => {
                attempt += 1;
                let body: String = edited.lines().filter(|l| !l.starts_with("<!-- thc error:")).collect::<Vec<_>>().join("\n");
                std::fs::write(&path, format!("<!-- thc error: {e:#} -->\n{body}\n"))?;
            }
            Err(e) => return Err(e.context(format!("edit not applied; your text is kept at {}", path.display()))),
        }
    }
}

/// One vault's Today (views.md §3.3): overdue (with fired alerts), today (with alert rows by
/// time), alerts, done today, and how many journal entries.
struct DayParts {
    overdue: Vec<Node>,
    today: Vec<Node>,
    alerts: Vec<thc_core::model::Alert>,
    done: Vec<Node>,
    journal: usize,
    fired: std::collections::HashMap<String, String>,
    alert_marks: std::collections::HashMap<String, String>,
}

fn day_parts(store: &thc_core::store::Store, day: chrono::NaiveDate) -> Result<DayParts> {
    let t = day.format("%Y-%m-%d").to_string();
    let open = "n.status IN ('todo','doing','waiting') AND n.deleted=0";
    let overdue = store.nodes_where(&format!("{open} AND substr(n.due,1,10) < ?1 ORDER BY n.due"), &[&t])?;
    let t1 = (day + chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
    // ISO ranges, so each branch uses an index (thc_core::query's date_range): 5 ms → 0.1 at 50k.
    let today_items = store.nodes_where(
        &format!(
            "n.deleted=0 AND n.is_tag=0 AND (n.status IS NULL OR n.status IN ('todo','doing','waiting')) AND n.id IN ( \
               SELECT id FROM nodes WHERE {open} AND scheduled < ?2 AND (due IS NULL OR due >= ?1) \
               UNION SELECT id FROM nodes WHERE due >= ?1 AND due < ?2 \
               UNION SELECT id FROM nodes WHERE status IS NULL AND scheduled >= ?1 AND scheduled < ?2) \
             ORDER BY n.scheduled IS NULL, n.scheduled, CASE n.priority WHEN 'high' THEN 0 WHEN 'med' THEN 1 ELSE 2 END",
            open = "status IN ('todo','doing','waiting') AND deleted=0"
        ),
        &[&t, &t1],
    )?;
    let alerts = store.alerts_where("deleted=0 AND state IN ('pending','snoozed') AND substr(fire_at,1,10) <= ?1 ORDER BY fire_at", &[&t])?;
    // Fired alerts surface under Overdue until handled (daemon.md §2.8).
    let mut overdue = overdue;
    let mut fired: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for a in &alerts {
        if let Some(at) = store.fired_at(a) {
            let when = a.fire_at.as_deref().and_then(|f| f.get(11..16)).unwrap_or(&at[11..16.min(at.len())]).to_string();
            fired.insert(a.node.clone(), when);
            if !overdue.iter().any(|n| n.id == a.node) {
                if let Some(n) = store.node(&a.node)? {
                    if !matches!(n.status.as_deref(), Some("done" | "cancelled")) {
                        overdue.push(n);
                    }
                }
            }
        }
    }
    let mut today_items: Vec<Node> = today_items.into_iter().filter(|n| !fired.contains_key(&n.id)).collect();
    // In Today only because an alert fires today: ordinary Today rows, by alert time (§3.3).
    let shown: std::collections::HashSet<String> = overdue.iter().chain(today_items.iter()).map(|n| n.id.clone()).collect();
    let mut alert_marks: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (n, time) in store.alert_rows(day, &shown)? {
        alert_marks.insert(n.id.clone(), time);
        today_items.push(n);
    }
    let done = store.nodes_where("n.deleted=0 AND n.status='done' AND n.done_at >= ?1 AND n.done_at < ?2 ORDER BY n.done_at", &[&t, &t1])?;
    let journal = store.journal_entries(&t)?.len();
    Ok(DayParts { overdue, today: today_items, alerts, done, journal, fired, alert_marks })
}

/// The other vaults a person's Today and Agenda read (vaults.md §3.2: they span `vault:*`,
/// because they're about you). Agents read the current vault only, as does a device with one
/// vault. Opened in parallel; one that can't be opened is skipped.
fn other_vaults(ctx: &Ctx) -> Vec<(String, Vault)> {
    if ctx.vault.actor.kind == "agent" {
        return vec![];
    }
    let reg = thc_core::registry::Registry::load();
    let here = out::VAULT_NAME.get().cloned().unwrap_or_default();
    let others: Vec<thc_core::registry::Entry> = reg.vaults.into_iter().filter(|e| e.name != here && e.path.join(vault::VAULT_MARKER).exists()).collect();
    if others.is_empty() {
        return vec![];
    }
    let actor = ctx.vault.actor.clone();
    std::thread::scope(|s| {
        let hs: Vec<_> = others
            .iter()
            .map(|e| {
                let actor = actor.clone();
                s.spawn(move || Vault::open(Paths { vault: e.path.clone(), cache: vault::default_cache(&e.path) }, actor, "cli").ok().map(|v| (e.name.clone(), v)))
            })
            .collect();
        hs.into_iter().filter_map(|h| h.join().ok().flatten()).collect()
    })
}

/// A row's place in Today's order (the SQL's): scheduled first, then by date, then priority.
fn today_key(n: &Node) -> (bool, String, u8) {
    let p = match n.priority.as_deref() {
        Some("high") => 0,
        Some("med") => 1,
        _ => 2,
    };
    (n.scheduled.is_none(), n.scheduled.clone().unwrap_or_default(), p)
}

fn today(ctx: &mut Ctx, why: bool) -> Result<()> {
    // Your @today (thc view set today --section …): its sections, across its scope.
    if let Some(v) = view_cmd::edited_sectioned(ctx, "today")? {
        return sectioned_cmd(ctx, &v);
    }
    let day = ctx.out.today;
    let t = day.format("%Y-%m-%d").to_string();
    let mut cur = day_parts(ctx.store(), day)?;
    let (o, total_o) = ctx.cfilter(std::mem::take(&mut cur.overdue));
    let (ti, total_t) = ctx.cfilter(std::mem::take(&mut cur.today));
    // Done items don't count toward `hidden`, which matches the human line's "N more".
    let before = ctx.context.hidden.get();
    let (dn, _) = ctx.cfilter(std::mem::take(&mut cur.done));
    ctx.context.hidden.set(before);
    cur.overdue = o;
    cur.today = ti;
    cur.done = dn;
    if let Some(a) = &ctx.context.active {
        cur.alerts.retain(|al| a.allows(&al.node));
    }
    // (What needs all of ctx, before the stores are borrowed for the rows.)
    let cj = ctx.context_json();
    let shown_here = cur.overdue.len() + cur.today.len();
    let all_hidden = if ctx.out.json { false } else { ctx.context_line(shown_here, total_o + total_t, "due", "") };
    // Every vault, for a person (the context filters only the current one).
    let others = other_vaults(ctx);
    let mut parts: Vec<(String, &thc_core::store::Store, DayParts)> = vec![(out::VAULT_NAME.get().cloned().unwrap_or_default(), &ctx.vault.store, cur)];
    for (name, v) in &others {
        parts.push((name.clone(), &v.store, day_parts(&v.store, day)?));
    }
    let contributing = parts.iter().filter(|(_, _, p)| !p.overdue.is_empty() || !p.today.is_empty()).count();
    let many = parts.len() > 1 && contributing > 1;
    // Sections merged across vaults in Today's own order: overdue by due, today by schedule
    // and priority, alert rows after (by time), each row with its vault.
    let mut overdue: Vec<(usize, &Node)> = parts.iter().enumerate().flat_map(|(i, (_, _, p))| p.overdue.iter().map(move |n| (i, n))).collect();
    overdue.sort_by(|a, b| a.1.due.cmp(&b.1.due));
    let mut today_rows: Vec<(usize, &Node)> = parts.iter().enumerate().flat_map(|(i, (_, _, p))| p.today.iter().filter(|n| !p.alert_marks.contains_key(&n.id)).map(move |n| (i, n))).collect();
    today_rows.sort_by_key(|(_, n)| today_key(n));
    let mut alert_rows: Vec<(usize, &Node)> = parts.iter().enumerate().flat_map(|(i, (_, _, p))| p.today.iter().filter(|n| p.alert_marks.contains_key(&n.id)).map(move |n| (i, n))).collect();
    alert_rows.sort_by(|a, b| parts[a.0].2.alert_marks.get(&a.1.id).cmp(&parts[b.0].2.alert_marks.get(&b.1.id)));
    today_rows.extend(alert_rows);
    let done_n: usize = parts.iter().map(|(_, _, p)| p.done.len()).sum();
    let journal_n: usize = parts.iter().map(|(_, _, p)| p.journal).sum();
    if ctx.out.json {
        let row = |(i, n): &(usize, &Node)| {
            let (name, store, _) = &parts[*i];
            let al = thc_core::why::Alerts::load(store);
            let mut v = row_json(store, &al, n, day, day);
            v["vault"] = json!(name);
            v
        };
        let done: Vec<Value> = parts.iter().flat_map(|(name, store, p)| {
            let al = thc_core::why::Alerts::load(store);
            p.done.iter().map(move |n| {
                let mut v = row_json(store, &al, n, day, day);
                v["vault"] = json!(name);
                v
            }).collect::<Vec<_>>()
        }).collect();
        let alerts: Vec<Value> = parts.iter().flat_map(|(name, store, p)| p.alerts.iter().map(move |a| {
            let mut v = alert_json(store, a);
            v["vault"] = json!(name);
            v
        }).collect::<Vec<_>>()).collect();
        let v = json!({
            "context": cj,
            "date": t,
            "overdue": overdue.iter().map(row).collect::<Vec<_>>(),
            "today": today_rows.iter().map(row).collect::<Vec<_>>(),
            "alerts": alerts,
            "done": done,
            "journal_entries": journal_n,
            "vault_source": vault_source_json(),
            "vaults": parts.iter().map(|(n, _, _)| n.clone()).collect::<Vec<_>>(),
        });
        ctx.out.json(&v);
        return Ok(());
    }
    let title = format!("Today · {}", day.format("%A, %B %-d"));
    ctx.out.heading(&title);
    if let Some(from) = project_source() {
        let l = ctx.out.dim(&format!("vault {} ({from})", vault::tilde(&ctx.vault.paths.vault)));
        ctx.out.line(l);
    }
    if parts.len() > 1 {
        let names: Vec<String> = parts.iter().map(|(n, _, _)| n.clone()).collect();
        let l = ctx.out.dim(&format!("{} vaults · {}", parts.len(), names.join(", ")));
        ctx.out.line(l);
    }
    if overdue.is_empty() && today_rows.is_empty() && !all_hidden {
        let l = ctx.out.dim("  Nothing due. thc j to write in today's journal · thc add \"…\" to capture");
        ctx.out.line(l);
    }
    // `acme · ◎ fired 14:00`: the vault (when rows come from more than one), then the mark.
    let mark = |out: &Out, i: usize, extra: Option<String>| -> Option<String> {
        let v = many.then(|| out.dim(&parts[i].0));
        match (v, extra) {
            (Some(v), Some(e)) => Some(format!("{v}{}{e}", out.dim(" · "))),
            (v, e) => v.or(e),
        }
    };
    if !overdue.is_empty() {
        let h = ctx.out.red("Overdue");
        ctx.out.line(h);
        for (i, n) in &overdue {
            let fired = parts[*i].2.fired.get(&n.id).map(|when| ctx.out.red(&format!("◎ fired {when}")));
            let l = ctx.out.node_line_marked(parts[*i].1, n, 1, true, mark(&ctx.out, *i, fired));
            let l = with_why_in(&ctx.out, parts[*i].1, l, n, day, why);
            ctx.out.line(l);
        }
    }
    if !today_rows.is_empty() {
        let h = ctx.out.yellow("Today");
        ctx.out.line(h);
        for (i, n) in &today_rows {
            let at = parts[*i].2.alert_marks.get(&n.id).map(|t| ctx.out.yellow(&format!("◎ {t}")));
            let l = ctx.out.node_line_marked(parts[*i].1, n, 1, true, mark(&ctx.out, *i, at));
            let l = with_why_in(&ctx.out, parts[*i].1, l, n, day, why);
            ctx.out.line(l);
        }
    }
    if done_n > 0 {
        let l = ctx.out.dim(&format!("Done today: {done_n}"));
        ctx.out.line(l);
    }
    let l = ctx.out.dim(&format!("Journal: {journal_n} entr{} today", if journal_n == 1 { "y" } else { "ies" }));
    ctx.out.line(l);
    let conflicts: usize = parts.iter().map(|(_, s, _)| s.open_conflicts().map(|c| c.len()).unwrap_or(0)).sum();
    drop(parts);
    drop(others);
    if conflicts > 0 {
        let l = ctx.out.magenta(&format!("≠ {conflicts} conflict{} · thc conflict ls", if conflicts == 1 { "" } else { "s" }));
        ctx.out.line(l);
    }
    paused_hint(ctx);
    // A quiet line when the last release check found something newer (never for agents).
    if let Some(v) = thc_core::release::available(env!("CARGO_PKG_VERSION")) {
        if std::env::var("THC_ACTOR").map(|a| a.is_empty() || a == "human").unwrap_or(true) {
            let l = ctx.out.dim(&format!("update {v} available · thc update"));
            ctx.out.line(l);
        }
    }
    Ok(())
}

/// A today/agenda row: the node plus `reasons[]` (views.md §3.3), always in JSON.
fn row_json(store: &thc_core::store::Store, alerts: &thc_core::why::Alerts, n: &Node, day: chrono::NaiveDate, today: chrono::NaiveDate) -> Value {
    let mut v = node_json(store, n);
    v["reasons"] = json!(thc_core::why::codes(&thc_core::why::reasons_with(alerts, n, day, today)));
    v
}

/// `--why`: a muted `   ← scheduled · repeating` after the row's meta (from its own vault's store).
fn with_why_in(out: &Out, store: &thc_core::store::Store, line: String, n: &Node, day: chrono::NaiveDate, why: bool) -> String {
    if !why {
        return line;
    }
    let today = out.today;
    let rs = thc_core::why::reasons(store, n, day, today);
    if rs.is_empty() {
        return line;
    }
    let words = thc_core::why::describe(&rs, day, today);
    format!("{line}{}", out.dim(&format!("   ← {words}")))
}

/// `thc prime`: what an agent sees at session start (agents.md §2.1). State first, then rules.
fn prime(ctx: &mut Ctx) -> Result<()> {
    let s = ctx.store();
    let t = ctx.out.today.format("%Y-%m-%d").to_string();
    let count = |sql: &str, p: &[&dyn rusqlite::ToSql]| s.nodes_where(sql, p).map(|v| v.len()).unwrap_or(0);
    let open = count("n.status IN ('todo','doing','waiting') AND n.deleted=0", &[]);
    let overdue = count("n.status IN ('todo','doing','waiting') AND n.deleted=0 AND substr(n.due,1,10) < ?1", &[&t]);
    let today_n = count(
        "n.status IN ('todo','doing','waiting') AND n.deleted=0 AND (substr(n.scheduled,1,10) = ?1 OR substr(n.due,1,10) = ?1)",
        &[&t],
    );
    let inbox = count("n.parent IS NULL AND n.title IS NULL AND n.journal IS NULL AND n.is_tag=0 AND n.deleted=0", &[]);
    let conflicts = s.open_conflicts()?.len();
    // Fired, unacknowledged alerts count as overdue (the badge rule).
    let fired: Vec<(String, String)> = s
        .alerts_where("deleted=0 AND state IN ('pending','snoozed')", &[])?
        .iter()
        .filter(|a| s.fired_at(a).is_some())
        .filter_map(|a| s.node(&a.node).ok().flatten().filter(|n| !matches!(n.status.as_deref(), Some("done" | "cancelled"))).map(|n| (n.id.clone(), thc_core::alerts::plain_title(s, &n))))
        .collect();
    let overdue_ids: std::collections::HashSet<String> = s
        .nodes_where("n.status IN ('todo','doing','waiting') AND n.deleted=0 AND substr(n.due,1,10) < ?1", &[&t])?
        .into_iter()
        .map(|n| n.id)
        .collect();
    let overdue = overdue + fired.iter().filter(|(id, _)| !overdue_ids.contains(id)).count();
    let actor = ctx.vault.actor.name.clone().filter(|_| ctx.vault.actor.kind == "agent");
    let live = !daemon_cmd::offline(&ctx.vault.paths);
    let home = std::env::var("HOME").unwrap_or_default();
    let vault = ctx.vault.paths.vault.display().to_string();
    let vault = match vault.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => vault,
    };
    let vault_shown = match project_source() {
        Some(from) => format!("{vault} ({from})"),
        None => vault.clone(),
    };
    let version = env!("CARGO_PKG_VERSION");
    // The agent's own last transaction in the past 7 days.
    let last = match &actor {
        Some(a) => {
            let since = chrono::Utc::now().timestamp_millis() - 7 * 86_400_000;
            s.history_where("actor = ?1 AND ms >= ?2 ORDER BY okey DESC LIMIT 20", &[&format!("agent:{a}"), &since])?
        }
        None => vec![],
    };
    let last_tx = last.first().map(|e| {
        let entries: Vec<_> = last.iter().filter(|x| x.tx == e.tx).collect();
        let main = entries.iter().rev().find(|x| x.op == "node.create").or(entries.last()).copied().unwrap_or(e);
        let label = s.node(&main.entity).ok().flatten().map(|n| s.render_text(&n.label())).unwrap_or_default();
        let verb = if main.op == "node.create" { "added" } else { main.op.trim_start_matches("node.") };
        let label: String = label.chars().take(40).collect();
        (e.tx[e.tx.len().saturating_sub(6)..].to_string(), out::ms_to_local(e.ms), format!("{verb} {} \"{label}\"", s.short(&main.entity)))
    });
    let mut stale = setup::stale_lines(&std::env::current_dir()?);
    stale.extend(bootstrap::stale_user_lines());
    let to_review = thc_core::review::pending_count(s)?;
    // Nothing written yet: agents shouldn't fill a new vault on their own.
    let empty = s.conn.query_row("SELECT count(*) FROM nodes WHERE deleted=0 AND is_tag=0 AND trim(text) <> ''", [], |r| r.get::<_, i64>(0)).unwrap_or(1) == 0;
    let yours_to_review = match &actor {
        Some(a) => thc_core::review::queue_txs(s, Some(a), None)?.len(),
        None => 0,
    };
    // The vaults on this device, by name (vaults.md §6): the agent works in this one.
    let reg = thc_core::registry::Registry::load();
    let vault_name = reg.name_for(&ctx.vault.paths.vault);
    let vault_names: Vec<String> = reg.vaults.iter().map(|e| e.name.clone()).collect();
    if ctx.out.json {
        let v = json!({
            "version": version, "actor": actor, "vault": vault, "vault_name": vault_name, "vaults": vault_names, "vault_source": vault_source_json(), "daemon": if live { "live" } else { "offline" },
            "counts": { "overdue": overdue, "today": today_n, "open": open, "inbox": inbox },
            "conflicts": conflicts,
            "empty": empty,
            "to_review": to_review,
            "yours_to_review": yours_to_review,
            "alerts_fired": fired.iter().map(|(id, t)| json!({ "node": id, "title": t })).collect::<Vec<_>>(),
            "last_tx": last_tx.as_ref().map(|(tx, at, summary)| json!({ "short": tx, "at": at, "summary": summary })),
            "stale": stale,
            "topics": instructions::TOPICS.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        });
        ctx.out.json(&v);
        return Ok(());
    }
    let who = match &actor {
        Some(a) => format!("you are {a}"),
        None => "THC_ACTOR is not set · export THC_ACTOR=<your name> so your writes are yours".into(),
    };
    let daemon = if live { "daemon live" } else { "daemon offline (fine; reads and writes work)" };
    ctx.out.line(format!("thc {version} · {who} · vault {vault_name} {vault_shown} · {daemon}"));
    if vault_names.len() > 1 {
        let list: Vec<String> = vault_names.iter().map(|n| if *n == vault_name { format!("{n} (current)") } else { n.clone() }).collect();
        ctx.out.line(format!("vaults    {} · you work in {vault_name} unless told otherwise (--vault <name>)", list.join(" · ")));
    }
    ctx.out.line(format!(
        "now       {} · {overdue} overdue · {today_n} today · {open} open · {inbox} inbox",
        dates::now_local().format("%a %b %-d %H:%M")
    ));
    if empty {
        ctx.out.line("empty     a new vault · the person writes with thc j; add tasks only when asked".to_string());
    }
    let mut heads = Vec::new();
    if !fired.is_empty() {
        let titles: Vec<&str> = fired.iter().map(|(_, t)| t.as_str()).take(3).collect();
        heads.push(format!("◎ {} alert{} fired ({})", fired.len(), if fired.len() == 1 { "" } else { "s" }, titles.join(", ")));
    }
    if conflicts > 0 {
        heads.push(format!("≠ {conflicts} conflict{} (tell the human, don't resolve)", if conflicts == 1 { "" } else { "s" }));
    }
    if actor.is_none() && to_review > 0 {
        heads.push(format!("◆ {to_review} agent change{} to review (thc review)", if to_review == 1 { "" } else { "s" }));
    }
    if let Some(w) = dates::pinned_warning() {
        heads.insert(0, w);
    }
    // A person's context (file or device) never filters an agent's --json reads (views.md §2.3).
    let human_context = ctx.context.resolved.as_ref().filter(|c| !c.source.explicit()).map(|c| c.name.clone());
    if !heads.is_empty() {
        ctx.out.line(format!("heads-up  {}", heads.join(" · ")));
    }
    if let Some(c) = &human_context {
        ctx.out.line(format!("context   @{c} is on for the human (your --json reads ignore it)"));
    }
    if let Some((tx, at, summary)) = &last_tx {
        let wait = if yours_to_review > 0 {
            format!(" · {yours_to_review} of your changes await review")
        } else {
            String::new()
        };
        ctx.out.line(format!("yours     last: {tx} {summary} ({}){wait}", at.get(11..).unwrap_or(at)));
    }
    ctx.out.line("");
    for l in [
        "rules     --json to parse · refer by ID (4+ char prefix) · --key <k> makes adds retry-safe",
        "          --if-match <rev> guards writes · --dry-run first · exit 0 ok 2 usage 3 not found 4 conflict/stale 5 ambiguous 6 invalid",
        "capture   thc add \"text due:fri sched:mon at:\\\"tue 2pm\\\" every:2w #tag !high [[Page]]\"",
        "          default → today's journal · --inbox · --under <id> · --journal <date>",
        "read      thc today|agenda|show <id> --json · thc q 'status:open due<=+3d #work sort:due'",
        "          next task: thc q is:ready --json (open, startable, unblocked)",
        "dates     today tomorrow fri \"next fri\" +3d +2h +30min +2mo eom \"nov 1 9am\" 2026-10-10 (a bare m is rejected)",
        "more      thc instructions [capture|query|dates|conflicts|exit-codes|json|commands]",
    ] {
        ctx.out.line(l);
    }
    for st in stale {
        ctx.out.line(st);
    }
    Ok(())
}

/// `alerts are paused (daemon offline) · thc daemon start` when an alert is due within 24 h
/// and no daemon runs (human output of today/agenda only, daemon.md §3.5).
fn paused_hint(ctx: &mut Ctx) {
    if ctx.out.json || !daemon_cmd::offline(&ctx.vault.paths) {
        return;
    }
    let soon = ctx.store().pending_alerts_within(thc_core::alerts::now(), chrono::Duration::hours(24)).unwrap_or(0);
    if soon > 0 {
        let l = ctx.out.dim("alerts are paused (daemon offline) · thc daemon start");
        ctx.out.line(l);
    }
}

/// `thc context [name|none]` (views.md §2.1).
fn context_cmd(ctx: &mut Ctx, name: Option<String>) -> Result<()> {
    let cache = ctx.vault.paths.cache.clone();
    match name.as_deref() {
        None => {
            let r = ctx.context.resolved.clone();
            if ctx.out.json {
                ctx.out.json(&json!({ "name": r.as_ref().map(|c| &c.name), "source": r.as_ref().map(|c| c.source.describe()), "device": ctx.context.device }));
            } else {
                match r {
                    Some(c) => ctx.out.line(format!("@{} ({})", c.name, c.source.describe())),
                    None => ctx.out.line("none"),
                }
            }
        }
        Some("none") | Some("off") => {
            thc_core::context::set_device(&cache, None)?;
            if ctx.out.json {
                ctx.out.json(&json!({ "ok": true, "device": null }));
            } else {
                ctx.out.line("context off");
                if let Some(c) = ctx.context.resolved.as_ref().filter(|c| c.source != thc_core::context::Source::Device) {
                    let l = ctx.out.dim(&format!("@{} is still on ({})", c.name, c.source.describe()));
                    ctx.out.line(l);
                }
            }
        }
        Some(n) => {
            let n = thc_core::views::normalize_name(n)?;
            let v = thc_core::views::find(ctx.store(), &n)?.ok_or_else(|| {
                let hint = thc_core::views::closest(ctx.store(), &n).map(|c| format!(" · did you mean @{c}?")).unwrap_or_default();
                thc_core::error::not_found(format!("no view @{n}{hint}"))
            })?;
            thc_core::context::set_device(&cache, Some(&n))?;
            if ctx.out.json {
                ctx.out.json(&json!({ "ok": true, "device": n, "view": v }));
            } else {
                let d = thc_core::context::CaptureDefaults::parse(v.capture.as_deref().unwrap_or(""));
                let caps = if d.is_empty() {
                    String::new()
                } else {
                    let mut bits: Vec<String> = d.tags.iter().map(|t| format!("#{t}")).collect();
                    if let Some(u) = &d.under {
                        bits.push(format!("under ¶ {u}"));
                    }
                    format!(" · captures get {}", bits.join(" "))
                };
                ctx.out.line(format!("context @{n} is on · listings show only {}{caps}", v.query));
                let l = ctx.out.dim("thc context none to turn it off");
                ctx.out.line(l);
            }
        }
    }
    Ok(())
}

/// One vault's agenda: overdue, then each day's scheduled and due rows.
fn agenda_parts(store: &thc_core::store::Store, today: chrono::NaiveDate, days: i64) -> Result<(Vec<Node>, Vec<(chrono::NaiveDate, Vec<Node>)>)> {
    let t = today.format("%Y-%m-%d").to_string();
    let overdue = store.nodes_where("n.status IN ('todo','doing','waiting') AND n.deleted=0 AND substr(n.due,1,10) < ?1 ORDER BY n.due", &[&t])?;
    let mut days_out = Vec::new();
    for i in 0..days.max(1) {
        let d = today + chrono::Duration::days(i);
        let ds = d.format("%Y-%m-%d").to_string();
        let ds1 = (d + chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
        // ISO ranges so each branch uses an index (33 ms → see SPEC §8 for 7 days at 50k).
        let items = store.nodes_where(
            "n.deleted=0 AND n.is_tag=0 AND (n.status IS NULL OR n.status IN ('todo','doing','waiting')) AND n.id IN ( \
               SELECT id FROM nodes WHERE scheduled >= ?1 AND scheduled < ?2 \
               UNION SELECT id FROM nodes WHERE due >= ?1 AND due < ?2) \
             ORDER BY coalesce(n.scheduled, n.due)",
            &[&ds, &ds1],
        )?;
        days_out.push((d, items));
    }
    Ok((overdue, days_out))
}

fn agenda(ctx: &mut Ctx, days: i64, why: bool) -> Result<()> {
    if let Some(v) = view_cmd::edited_sectioned(ctx, "agenda")? {
        return sectioned_cmd(ctx, &v);
    }
    let today = ctx.out.today;
    let (overdue, days_out) = agenda_parts(ctx.store(), today, days)?;
    let (overdue, mut total) = ctx.cfilter(overdue);
    let mut shown = overdue.len();
    let days_out: Vec<(chrono::NaiveDate, Vec<Node>)> = days_out
        .into_iter()
        .map(|(d, items)| {
            let (kept, all) = ctx.cfilter(items);
            total += all;
            shown += kept.len();
            (d, kept)
        })
        .collect();
    let cj = ctx.context_json();
    if !ctx.out.json && ctx.context_line(shown, total, "due", "") {
        return Ok(());
    }
    // Every vault, for a person (vaults.md §3.2); the context filters only the current one.
    let others = other_vaults(ctx);
    let mut parts: Vec<(String, &thc_core::store::Store, Vec<Node>, Vec<(chrono::NaiveDate, Vec<Node>)>)> =
        vec![(out::VAULT_NAME.get().cloned().unwrap_or_default(), &ctx.vault.store, overdue, days_out)];
    for (name, v) in &others {
        let (o, d) = agenda_parts(&v.store, today, days)?;
        parts.push((name.clone(), &v.store, o, d));
    }
    let contributing = parts.iter().filter(|p| !p.2.is_empty() || p.3.iter().any(|(_, x)| !x.is_empty())).count();
    let many = parts.len() > 1 && contributing > 1;
    let mut overdue: Vec<(usize, &Node)> = parts.iter().enumerate().flat_map(|(i, p)| p.2.iter().map(move |n| (i, n))).collect();
    overdue.sort_by(|a, b| a.1.due.cmp(&b.1.due));
    let ndays = parts[0].3.len();
    let day_rows: Vec<(chrono::NaiveDate, Vec<(usize, &Node)>)> = (0..ndays)
        .map(|k| {
            let d = parts[0].3[k].0;
            let mut rows: Vec<(usize, &Node)> = parts.iter().enumerate().flat_map(|(i, p)| p.3[k].1.iter().map(move |n| (i, n))).collect();
            rows.sort_by(|a, b| a.1.scheduled.as_ref().or(a.1.due.as_ref()).cmp(&b.1.scheduled.as_ref().or(b.1.due.as_ref())));
            (d, rows)
        })
        .collect();
    if ctx.out.json {
        let row = |(i, n): &(usize, &Node), d: chrono::NaiveDate| {
            let (name, store, _, _) = &parts[*i];
            let al = thc_core::why::Alerts::load(store);
            let mut v = row_json(store, &al, n, d, today);
            v["vault"] = json!(name);
            v
        };
        let v = json!({
            "context": cj,
            "overdue": overdue.iter().map(|r| row(r, today)).collect::<Vec<_>>(),
            "days": day_rows.iter().map(|(d, items)| json!({
                "date": d.format("%Y-%m-%d").to_string(),
                "items": items.iter().map(|r| row(r, *d)).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "vaults": parts.iter().map(|p| p.0.clone()).collect::<Vec<_>>(),
        });
        ctx.out.json(&v);
        return Ok(());
    }
    if parts.len() > 1 {
        let names: Vec<String> = parts.iter().map(|p| p.0.clone()).collect();
        let l = ctx.out.dim(&format!("{} vaults · {}", parts.len(), names.join(", ")));
        ctx.out.line(l);
    }
    let mark = |out: &Out, i: usize| many.then(|| out.dim(&parts[i].0));
    if !overdue.is_empty() {
        let h = ctx.out.red("Overdue");
        ctx.out.line(h);
        for (i, n) in &overdue {
            let l = ctx.out.node_line_marked(parts[*i].1, n, 1, true, mark(&ctx.out, *i));
            let l = with_why_in(&ctx.out, parts[*i].1, l, n, today, why);
            ctx.out.line(l);
        }
    }
    for (d, items) in &day_rows {
        let label = format!("{} · {}", d.format("%a %b %-d"), dates::relative(*d, today));
        let h = if *d == today { ctx.out.yellow(&label) } else { ctx.out.bold(&label) };
        ctx.out.line(h);
        if items.is_empty() {
            let l = ctx.out.dim("  -");
            ctx.out.line(l);
        }
        for (i, n) in items {
            let l = ctx.out.node_line_marked(parts[*i].1, n, 1, true, mark(&ctx.out, *i));
            let l = with_why_in(&ctx.out, parts[*i].1, l, n, *d, why);
            ctx.out.line(l);
        }
    }
    drop(day_rows);
    drop(overdue);
    drop(parts);
    drop(others);
    paused_hint(ctx);
    Ok(())
}


fn resolve_alert(ctx: &Ctx, id: &str) -> Result<String> {
    let p = thc_core::id::normalize(id);
    let hi = format!("{p}~");
    let found = ctx.store().alerts_where("id >= ?1 AND id < ?2 AND deleted=0", &[&p, &hi])?;
    match found.len() {
        1 => Ok(found[0].id.clone()),
        0 => {
            // Allow addressing by node: the node's single alert.
            let node = ctx.resolve(id)?;
            let a = ctx.store().alerts_of(&node)?;
            match a.len() {
                1 => Ok(a[0].id.clone()),
                0 => Err(thc_core::error::not_found(format!("no alert {id}"))),
                _ => Err(ThcError::Ambiguous { prefix: p, candidates: a.iter().map(|x| x.id.clone()).collect() }.into()),
            }
        }
        _ => Err(ThcError::Ambiguous { prefix: p, candidates: found.iter().map(|x| x.id.clone()).collect() }.into()),
    }
}

fn doctor(ctx: &mut Ctx) -> Result<()> {
    let s = ctx.store();
    let count = |sql: &str| -> i64 { s.conn.query_row(sql, [], |r| r.get(0)).unwrap_or(-1) };
    let events = count("SELECT count(*) FROM events");
    let nodes = count("SELECT count(*) FROM nodes WHERE deleted=0");
    let tasks_open = count("SELECT count(*) FROM nodes WHERE deleted=0 AND status IN ('todo','doing','waiting')");
    let conflicts = s.open_conflicts()?.len();
    let dup_journals: Vec<String> = {
        let mut st = s.conn.prepare("SELECT journal FROM nodes WHERE journal IS NOT NULL AND deleted=0 GROUP BY journal HAVING count(*) > 1")?;
        st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    let files = ctx.vault.log.files()?;
    let devices: std::collections::BTreeSet<String> = files.iter().map(|(f, _)| f.split('/').next().unwrap_or("").to_string()).collect();
    let mut issues: Vec<String> = vec![];
    let mut notices: Vec<String> = vec![];
    if conflicts > 0 {
        notices.push(format!("≠ {conflicts} open conflict{} · thc conflict ls", if conflicts == 1 { "" } else { "s" }));
    }
    for j in &dup_journals {
        issues.push(format!("journal {j} exists twice (made on two devices) · thc doctor --fix merges them"));
    }
    issues.extend(ctx.vault.warnings.iter().map(|w| format!("bad log line: {w}")));
    let drop_pending = std::fs::read_dir(ctx.vault.paths.vault.join("drop"))
        .map(|rd| rd.filter_map(|e| e.ok()).filter(|e| !e.file_name().to_string_lossy().starts_with('.')).count())
        .unwrap_or(0);
    if drop_pending > 0 {
        issues.push(format!("{drop_pending} file(s) waiting in drop/: run `thc ingest`"));
    }
    if let Some(w) = dates::pinned_warning() {
        issues.push(w);
    }
    // Before 0.6.2 a heading marker (`## Sub`) was read as a tag, stored as an empty-named tag.
    let heading_tags = count(
        "SELECT count(*) FROM edges e JOIN nodes t ON t.id = e.dst JOIN nodes n ON n.id = e.src \
         WHERE e.rel = 'tag' AND n.deleted = 0 AND coalesce(t.title, '') = ''",
    );
    if heading_tags > 0 {
        notices.push(format!(
            "{heading_tags} note{} carry an empty tag read from a heading marker (`## …`) · thc doctor --fix removes it",
            if heading_tags == 1 { "" } else { "s" }
        ));
    }
    // Vault settings that were ignored (vaults.md §9.3).
    notices.extend(thc_core::settings::load(Some(&ctx.vault.paths.vault)).notices);
    // Tokens lenient paths kept as text (drop/ ingest), newest first.
    for (at, n) in ingest::read_notices(&ctx.vault.paths.cache).into_iter().take(10) {
        notices.push(format!("{n} ({})", at.replace('T', " ")));
    }
    // Attachments (attachments.md §1): a referenced file that's gone, and files nothing refers to.
    let (missing, orphans) = attachment_check(ctx)?;
    for m in &missing {
        issues.push(format!("▣ missing: {m}"));
    }
    for o in &orphans {
        notices.push(format!("{o} isn't referenced · thc doctor --fix --yes moves it to files/.orphans"));
    }
    // Attachment nodes (FORMAT.md "Attachments"): a note whose text and embeds disagree, and a
    // node nothing shows.
    let drift = thc_core::attach::drift(ctx.store())?;
    if !drift.notes.is_empty() {
        let n = drift.notes.len();
        issues.push(format!("{n} note{}' attachments and embeds disagree · thc doctor --fix --yes", if n == 1 { "" } else { "s" }));
    }
    for o in &drift.orphans {
        notices.push(format!("attachment {} isn't shown anywhere · thc show {} --json", ctx.store().short(o), ctx.store().short(o)));
    }
    let v = json!({
        "vault": ctx.vault.paths.vault,
        "vault_source": vault_source_json(),
        "cache": ctx.vault.paths.cache,
        "device": ctx.vault.device,
        "devices": devices,
        "log_files": files.len(),
        "events": events,
        "nodes": nodes,
        "open_tasks": tasks_open,
        "issues": issues,
        "notices": notices,
    });
    if ctx.out.json {
        ctx.out.json(&v);
    } else {
        let why = VAULT_SOURCE.get().map(|s| ctx.out.dim(&format!("  · {}", s.describe()))).unwrap_or_default();
        ctx.out.line(format!("vault    {}{why}", ctx.vault.paths.vault.display()));
        ctx.out.line(format!("cache    {}", ctx.vault.paths.cache.display()));
        ctx.out.line(format!("device   {}  ({} device(s) in log)", ctx.vault.device, devices.len()));
        ctx.out.line(format!("events   {events} in {} log file(s)", files.len()));
        ctx.out.line(format!("nodes    {nodes} ({tasks_open} open tasks)"));
        for n in &notices {
            let l = ctx.out.dim(&format!("· {n}"));
            ctx.out.line(l);
        }
        if issues.is_empty() {
            let mut ok = ctx.out.green("ok: nothing broken");
            if conflicts > 0 {
                ok.push_str(&ctx.out.magenta(&format!(" · {conflicts} conflict{} to look at", if conflicts == 1 { "" } else { "s" })));
            }
            ctx.out.line(ok);
        } else {
            for i in issues {
                let l = ctx.out.yellow(&format!("! {i}"));
                ctx.out.line(l);
            }
        }
    }
    Ok(())
}

/// `thc config --effective [--vault v] [--json]` and `--why <key>` (vaults.md §9.3): every
/// setting in effect with where it came from, or every layer for one key.
fn config_cmd(out: &mut Out, vault: &std::path::Path, effective: bool, why: Option<&str>) -> Result<()> {
    let eff = thc_core::settings::load(Some(vault));
    if let Some(key) = why {
        let key = key.trim();
        let layers = eff.layers.get(key).cloned().unwrap_or_default();
        if out.json {
            let ls: Vec<Value> = layers.iter().map(|e| json!({ "value": toml_json(&e.value), "source": e.source.kind(), "label": e.source.label(), "file": e.file })).collect();
            out.json(&json!({ "key": key, "value": eff.get(key).map(toml_json), "layers": ls, "notices": eff.notices }));
            return Ok(());
        }
        if layers.is_empty() {
            out.line(format!("{key}  (not set: the built-in default)"));
        }
        let n = layers.len();
        for (i, e) in layers.iter().enumerate() {
            let line = format!("{key}  {}  {}  ({})", e.value, e.source.label(), vault::tilde(&e.file));
            out.line(if i + 1 == n { line } else { out.dim(&format!("{line}  · overridden")) });
        }
        for n in &eff.notices {
            out.line(out.dim(&format!("· {n}")));
        }
        return Ok(());
    }
    let _ = effective;
    let rows: Vec<(String, thc_core::settings::Entry)> = eff.layers.iter().filter_map(|(k, v)| v.last().cloned().map(|e| (k.clone(), e))).collect();
    if out.json {
        let items: Vec<Value> = rows.iter().map(|(k, e)| json!({ "key": k, "value": toml_json(&e.value), "source": e.source.kind(), "label": e.source.label(), "file": e.file })).collect();
        out.json(&json!({ "vault": eff.vault_name, "settings": items, "notices": eff.notices }));
        return Ok(());
    }
    if rows.is_empty() {
        out.line("nothing set: every setting is the built-in default");
    }
    let kw = rows.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0).min(36);
    let vw = rows.iter().map(|(_, e)| e.value.to_string().chars().count()).max().unwrap_or(0).min(24);
    for (k, e) in &rows {
        let src = out.dim(&format!("{}  ({})", e.source.label(), vault::tilde(&e.file)));
        out.line(format!("{k:<kw$}  {:<vw$}  {src}", e.value.to_string()));
    }
    for n in &eff.notices {
        out.line(out.dim(&format!("· {n}")));
    }
    Ok(())
}

fn toml_json(v: &toml::Value) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// `thc doctor --fix [--yes]` (data-model-review.md §1, §5): a preview by default, even on a
/// terminal; `--yes` writes every repair as one transaction, which `thc undo` reverses.
/// Attachments referenced but missing, and files under files/ nothing refers to.
fn attachment_check(ctx: &Ctx) -> Result<(Vec<String>, Vec<String>)> {
    let vault = &ctx.vault.paths.vault;
    let mut st = ctx.store().conn.prepare("SELECT text FROM nodes WHERE deleted=0 AND text LIKE '%](files/%'")?;
    let texts: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let referenced: std::collections::BTreeSet<String> = texts.iter().flat_map(|t| thc_core::attach::refs(t)).map(|(_, p)| p).collect();
    let missing = referenced.iter().filter(|p| !vault.join(p).exists()).cloned().collect();
    let orphans = thc_core::attach::all_files(vault).into_iter().filter(|f| !referenced.contains(f)).collect();
    Ok((missing, orphans))
}

fn doctor_fix(ctx: &mut Ctx) -> Result<()> {
    // Attachment nodes and embeds, from the notes' text: one transaction by thc, undoable.
    let drift = thc_core::attach::drift(ctx.store())?;
    if !drift.notes.is_empty() {
        if ctx.yes && !ctx.dry_run {
            let n = thc_core::attach::upgrade(&mut ctx.vault, true)?;
            ctx.out.line(format!("attachments: {n} note{} given their attachment nodes and embeds · thc undo reverts it", if n == 1 { "" } else { "s" }));
        } else {
            ctx.out.line(format!("would give {} note{} their attachment nodes and embeds · --yes to do it", drift.notes.len(), if drift.notes.len() == 1 { "" } else { "s" }));
        }
    }
    // Orphaned attachments move aside (never deleted).
    let (_, orphans) = attachment_check(ctx)?;
    if !orphans.is_empty() {
        if ctx.yes && !ctx.dry_run {
            let vault = ctx.vault.paths.vault.clone();
            for o in &orphans {
                let to = thc_core::attach::orphans_dir(&vault).join(o.trim_start_matches("files/"));
                if let Some(d) = to.parent() {
                    std::fs::create_dir_all(d)?;
                }
                std::fs::rename(vault.join(o), &to)?;
            }
            ctx.out.line(format!("moved {} unreferenced file{} to files/.orphans", orphans.len(), if orphans.len() == 1 { "" } else { "s" }));
        } else {
            ctx.out.line(format!("would move {} unreferenced file{} to files/.orphans · --yes to do it", orphans.len(), if orphans.len() == 1 { "" } else { "s" }));
        }
    }
    let plan = thc_core::repair::plan(ctx.store())?;
    let ops = thc_core::repair::ops(ctx.store(), &plan)?;
    let plural = |n: usize, one: &str, many: &str| if n == 1 { format!("{n} {one}") } else { format!("{n} {many}") };
    // What a person recognises: `Sun Oct 4`, and every spelling of a tag (`#lisbon, #Lisbon`).
    let label = |m: &thc_core::repair::Merge| match m.kind {
        "day" => chrono::NaiveDate::parse_from_str(&m.name, "%Y-%m-%d").map(|d| d.format("%a %b %-d").to_string()).unwrap_or_else(|_| m.name.clone()),
        _ => m.spellings.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(", "),
    };
    let names = |kind: &str| {
        let all: Vec<String> = plan.merges.iter().filter(|m| m.kind == kind).map(label).collect();
        let more = all.len().saturating_sub(3);
        let mut s = all.into_iter().take(3).collect::<Vec<_>>().join("; ");
        if more > 0 {
            s.push_str(&format!(" +{more}"));
        }
        s
    };
    let (days, tags, empty) = (plan.count("day"), plan.count("tag"), plan.empty_tags.len());
    let merges: usize = plan.merges.iter().map(|m| m.merged.len()).sum();
    let merges_json: Vec<Value> = plan.merges.iter().map(|m| json!({ "kind": m.kind, "name": m.name, "keep": m.keep, "merged": m.merged, "spellings": m.spellings })).collect();
    let vault = vault::tilde(&ctx.vault.paths.vault);
    if plan.is_empty() || !ctx.yes || ctx.dry_run {
        if ctx.out.json {
            ctx.out.json(&json!({ "ok": true, "applied": false, "merges": merges_json, "empty_tags": plan.empty_tags, "shared_titles": plan.shared_titles, "ops": ops.len() }));
            return Ok(());
        }
        let nodes: i64 = ctx.store().conn.query_row("SELECT count(*) FROM nodes WHERE deleted=0", [], |r| r.get(0)).unwrap_or(0);
        let l = ctx.out.dim(&format!("checking {vault} · {nodes} nodes"));
        ctx.out.line(l);
        let check = |ok: bool, good: &str, bad: String| if ok { format!("✓ {good}") } else { format!("! {bad}") };
        let lines = [
            check(days == 0, "days: one each", format!("{} twice ({}) · merge into the oldest", plural(days, "day appears", "days appear"), names("day"))),
            check(tags == 0, "tags: one each", format!("{} twice ({}) · merge into the oldest", plural(tags, "tag appears", "tags appear"), names("tag"))),
            check(empty == 0, "no empty tags", format!("{} (left by old heading markers) · remove", plural(empty, "empty tag", "empty tags"))),
            check(
                plan.shared_titles.is_empty(),
                "pages: one per title",
                format!("{} used twice ({}) · not merged: open both and decide", plural(plan.shared_titles.len(), "page title", "page titles"), plan.shared_titles.join(", ")),
            ),
        ];
        for l in lines {
            let l = if l.starts_with('!') { ctx.out.yellow(&l) } else { l };
            ctx.out.line(l);
        }
        if plan.is_empty() {
            let tail = if plan.shared_titles.is_empty() { "nothing to fix" } else { "nothing thc can fix on its own" };
            let l = ctx.out.green(&format!("✓ {vault} is healthy · {tail}"));
            ctx.out.line(l);
            return Ok(());
        }
        let removals = if empty > 0 { format!(" · {}", plural(empty, "removal", "removals")) } else { String::new() };
        ctx.out.line(format!("would write 1 transaction · {}{removals}", plural(merges, "merge", "merges")));
        ctx.out.line("nothing you wrote is lost · history keeps both ids".to_string());
        let l = ctx.out.dim("thc doctor --fix --yes to apply · thc undo reverses it");
        ctx.out.line(l);
        return Ok(());
    }
    let Some((events, ())) = ctx.write(|b| {
        b.ops.extend(ops);
        Ok(())
    })?
    else {
        return Ok(());
    };
    let tx = events.first().map(|e| e.tx.clone()).unwrap_or_default();
    let short = tx[tx.len().saturating_sub(6)..].to_string();
    if ctx.out.json {
        ctx.out.json(&json!({ "ok": true, "applied": true, "tx": tx, "merges": merges_json, "empty_tags": plan.empty_tags, "shared_titles": plan.shared_titles }));
        return Ok(());
    }
    let mut what = Vec::new();
    if days > 0 {
        what.push(plural(days, "day", "days"));
    }
    if tags > 0 {
        what.push(plural(tags, "tag", "tags"));
    }
    let mut done = if what.is_empty() { String::new() } else { format!("merged {}", what.join(" and ")) };
    if empty > 0 {
        done = format!("{done}{}removed {}", if done.is_empty() { "" } else { ", " }, plural(empty, "empty tag", "empty tags"));
    }
    ctx.out.line(format!("{done} in 1 transaction · thc undo --tx {short}"));
    Ok(())
}

/// `thc release-tool keygen|sign` (CI and the one-time key setup). The private key never reaches
/// stdout or logs.
fn release_tool(cmd: &cli::ReleaseToolCmd) -> Result<()> {
    use ed25519_dalek::{Signer, SigningKey};
    match cmd {
        cli::ReleaseToolCmd::Keygen { file } => {
            if file.exists() {
                return Err(usage(format!("{} exists · not overwriting a key", file.display())));
            }
            let mut seed = [0u8; 32];
            use std::io::Read;
            std::fs::File::open("/dev/urandom")?.read_exact(&mut seed)?;
            let key = SigningKey::from_bytes(&seed);
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(file)?;
            use std::io::Write;
            writeln!(f, "{}", update::hex(&seed))?;
            println!("{}", update::hex(key.verifying_key().as_bytes()));
            Ok(())
        }
        cli::ReleaseToolCmd::Public => {
            let hex = std::env::var("CLI_UPDATE_KEY").map_err(|_| usage("CLI_UPDATE_KEY isn't set"))?;
            let seed: [u8; 32] = update::unhex(hex.trim()).and_then(|b| b.try_into().ok()).ok_or_else(|| usage("CLI_UPDATE_KEY must be 32 bytes of hex"))?;
            println!("{}", update::hex(SigningKey::from_bytes(&seed).verifying_key().as_bytes()));
            Ok(())
        }
        cli::ReleaseToolCmd::Verify { manifest, dir } => update::verify_manifest(manifest, dir),
        cli::ReleaseToolCmd::Sign { version, target, sha256 } => {
            let hex = std::env::var("CLI_UPDATE_KEY").map_err(|_| usage("CLI_UPDATE_KEY isn't set"))?;
            let seed: [u8; 32] = update::unhex(hex.trim()).and_then(|b| b.try_into().ok()).ok_or_else(|| usage("CLI_UPDATE_KEY must be 32 bytes of hex"))?;
            let key = SigningKey::from_bytes(&seed);
            println!("{}", update::hex(&key.sign(update::signed_message(version, target, sha256).as_bytes()).to_bytes()));
            Ok(())
        }
    }
}

fn print_role(name: &str, json: bool) -> Result<()> {
    let card = instructions::role_card(name)?;
    let mut out = Out::new(json);
    if json { out.json(&json!({ "role": name, "card": card })); }
    else { out.line(format!("{name}\n{}", card.trim_end())); }
    out.flush();
    Ok(())
}
