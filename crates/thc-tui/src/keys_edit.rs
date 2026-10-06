//! `thc keys --edit` (keymap.md §8.2a): remapping made findable. A marked block
//! in config.toml lists every binding, commented out and grouped by context, ready to uncomment
//! and change. Later runs regenerate only the commented lines; lines you uncommented stay where
//! they are, under their table. After the editor closes the result is validated (§8.3) and the
//! problems are named with their line numbers.

use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Shared CLI/TUI editor round-trip. The TUI restores the terminal before calling this.
pub fn edit(context: Option<&str>) -> anyhow::Result<String> {
    let file = thc_core::vault::global_config_path().ok_or_else(|| anyhow::anyhow!("no config path (HOME isn't set)"))?;
    let old = std::fs::read_to_string(&file).unwrap_or_default();
    let next = with_block(&old, &crate::keys_json());
    if let Some(c) = context {
        if line_of_context(&next, c).is_none() {
            return Err(thc_core::error::invalid(format!("no keys context {c:?} · global, list, today, inbox, tasks, pages, journal, search, log, write")));
        }
    }
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d)?;
    }
    if next != old {
        std::fs::write(&file, &next)?;
    }
    let line = context.and_then(|c| line_of_context(&next, c)).or_else(|| next.lines().position(|l| l == BEGIN).map(|i| i + 1)).unwrap_or(1);
    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "vi".into());
    let at = if ["vi", "vim", "nvim", "nano", "micro", "emacs", "kak", "hx"].iter().any(|e| editor.split_whitespace().next().is_some_and(|x| x.ends_with(e))) { format!(" +{line}") } else { String::new() };
    let status = std::process::Command::new("sh").arg("-c").arg(format!("{editor}{at} \"$1\"")).arg("thc-keys").arg(&file).status()?;
    if !status.success() {
        anyhow::bail!("editor exited with {status}");
    }
    let text = std::fs::read_to_string(&file)?;
    let problems = validate(&file, &text);
    if problems.is_empty() {
        Ok(format!("keys ok · {} remapped · {}", remapped(&text), thc_core::vault::tilde(&file)))
    } else {
        Err(thc_core::error::invalid(format!("{} · thc keys --edit to fix", problems.join(" · "))))
    }
}

/// Runtime-only change detection; rendering and update never read config files.
#[derive(Default)]
pub(crate) struct Watch(Vec<(std::path::PathBuf, Option<(std::time::SystemTime, u64)>)>);

impl Watch {
    pub(crate) fn changed(&mut self, vault: &Path) -> bool {
        let files = thc_core::vault::global_config_path().into_iter().chain([vault.join("settings.toml")]);
        let next = files.map(|path| {
            let stamp = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok().map(|at| (at, m.len())));
            (path, stamp)
        }).collect();
        if self.0 == next { return false; }
        self.0 = next;
        true
    }
}

pub const BEGIN: &str = "# ── Keys (thc keys --edit) ─────────────────────────────────────────────────────";
pub const END: &str = "# ── end of keys ───────────────────────────────────────────────────────────────";

/// The contexts a remap can name (keymap.md §8.1), in the order the block lists them.
const CTXS: [&str; 10] = ["global", "list", "today", "inbox", "tasks", "pages", "journal", "search", "log", "write"];

/// The block, regenerated: the comments from the current bindings (`keys_json`), your
/// uncommented lines kept under their table. `old` is the block as it is (between the markers).
pub fn block(bindings: &serde_json::Value, old: Option<&str>) -> String {
    let mut preamble: Vec<String> = Vec::new();
    let mut mine: HashMap<String, Vec<String>> = HashMap::new();
    let mut tables: HashSet<String> = HashSet::new();
    let mut cur: Option<String> = None;
    for l in old.unwrap_or("").lines() {
        let t = l.trim();
        if let Some(c) = t.strip_prefix("[keys.").and_then(|r| r.strip_suffix(']')) {
            cur = Some(c.to_string());
            tables.insert(c.to_string());
        } else if let Some(c) = t.strip_prefix("# [keys.").and_then(|r| r.strip_suffix(']')) {
            cur = Some(c.to_string());
        } else if !t.is_empty() && !t.starts_with('#') {
            match &cur {
                Some(c) => mine.entry(c.clone()).or_default().push(l.to_string()),
                None => preamble.push(l.to_string()),
            }
        }
    }
    let rows = bindings["bindings"].as_array().cloned().unwrap_or_default();
    let mut out = vec![
        BEGIN.to_string(),
        "# Remap a key: uncomment a line and its [keys.…] line, and change the key on the left.".into(),
        "# \"no_op\" unbinds a key. Notation: C- ⌃  A- ⌥  S- ⇧  Cmd- ⌘ · check: thc keys --conflicts".into(),
        "# Examples: \"q\" = \"no_op\" (unbind) · \"F9\" = \"focus.toggle\" (a second key) ·".into(),
        "#           \"C-g\" = \"doc.open\" (move an action). Every action: thc keys --json".into(),
    ];
    out.extend(preamble);
    for ctx in CTXS {
        out.push(String::new());
        if tables.contains(ctx) {
            out.push(format!("[keys.{ctx}]"));
            out.extend(mine.remove(ctx).unwrap_or_default());
        } else {
            out.push(format!("# [keys.{ctx}]"));
            // (Uncommented lines under a still-commented header: kept, so nothing you wrote goes.)
            out.extend(mine.remove(ctx).unwrap_or_default());
        }
        for b in rows.iter().filter(|b| b["context"] == ctx) {
            let pair = format!("\"{}\" = \"{}\"", b["keys"].as_str().unwrap_or(""), b["action"].as_str().unwrap_or(""));
            let shown = b["shown"].as_str().unwrap_or("");
            let label = b["label"].as_str().unwrap_or("");
            out.push(format!("# {pair:<34} # {shown:<8} {label}").trim_end().to_string());
        }
    }
    // A table the block doesn't know (a context added later, say): kept as it was.
    for (ctx, lines) in mine {
        out.push(String::new());
        out.push(format!("[keys.{ctx}]"));
        out.extend(lines);
    }
    out.push(END.to_string());
    out.join("\n") + "\n"
}

/// The config with the block put in (replacing the old one, or appended).
pub fn with_block(config: &str, bindings: &serde_json::Value) -> String {
    match (config.find(BEGIN), config.find(END)) {
        (Some(a), Some(b)) if b > a => {
            let inner = &config[a + BEGIN.len()..b];
            let rest = &config[b + END.len()..];
            format!("{}{}{}", &config[..a], block(bindings, Some(inner)).trim_end_matches('\n'), rest)
        }
        _ => {
            let sep = if config.is_empty() || config.ends_with("\n\n") { "" } else if config.ends_with('\n') { "\n" } else { "\n\n" };
            format!("{config}{sep}{}", block(bindings, None))
        }
    }
}

/// The 1-based line of `[keys.<ctx>]` (or its commented form) in `text`.
pub fn line_of_context(text: &str, ctx: &str) -> Option<usize> {
    text.lines().position(|l| {
        let t = l.trim();
        t == format!("[keys.{ctx}]") || t == format!("# [keys.{ctx}]")
    }).map(|i| i + 1)
}

/// How many remaps are on: uncommented `"key" = "action"` lines under a `[keys.…]` table.
pub fn remapped(text: &str) -> usize {
    let mut in_keys = false;
    let mut n = 0;
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            in_keys = t.starts_with("[keys.");
        } else if in_keys && !t.starts_with('#') && t.contains('=') {
            n += 1;
        }
    }
    n
}

/// Validate the config as written: TOML first, then the remaps (§8.3) as a fresh thc reads them.
/// Problems come back with the line they're about, where one can be found.
pub fn validate(file: &Path, text: &str) -> Vec<String> {
    if let Err(e) = text.parse::<toml::Table>() {
        let line = e.span().map(|s| text[..s.start.min(text.len())].lines().count().max(1));
        let msg = e.message().to_string();
        return vec![match line {
            Some(n) => format!("line {n}: {msg}"),
            None => msg,
        }];
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| "thc".into());
    let mut c = std::process::Command::new(exe);
    c.args(["--json", "keys", "--conflicts"]);
    // The file just edited, wherever it is.
    if let Some(dir) = file.parent() {
        c.env("THC_CONFIG_DIR", dir);
    }
    let out = match c.output() {
        Ok(o) => o,
        Err(e) => return vec![format!("couldn't check the keys: {e}")],
    };
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let problems: Vec<String> = v["conflicts"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    // (The shipped table has none, keymap tests keep it so: every problem is a remap's.)
    problems
        .into_iter()
        .map(|p| match quoted(&p).and_then(|q| text.lines().position(|l| !l.trim().starts_with('#') && l.contains(&q))) {
            Some(i) => format!("line {}: {p}", i + 1),
            None => p,
        })
        .collect()
}

/// The first `"…"` in a message (the key or action it's about).
fn quoted(s: &str) -> Option<String> {
    let a = s.find('"')?;
    let b = s[a + 1..].find('"')? + a + 1;
    Some(s[a..=b].to_string())
}
