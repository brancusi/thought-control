//! `thc setup wezterm` (keymap.md §12.5): macOS and VS Code editing keys in
//! WezTerm, scoped to thc. `--yes` writes thc's module, `~/.config/wezterm/thc_keys.lua`, and
//! adds one marked block to the config WezTerm loads (backed up first, never duplicated);
//! `--undo --yes` takes both out.

use crate::out::Out;
use anyhow::Result;
use std::path::PathBuf;

/// The module. Every key is scoped to thc: while thc is the pane's foreground process it sends
/// the sequence thc reads (Home / End, C-Home / C-End, ⌃U, word moves, kitty's ⌘V); anywhere
/// else the key goes through untouched (`SendKey`, ⌘V a normal paste), so zsh, editors and SSH
/// keep their own behaviour. It also turns on WezTerm's kitty keyboard protocol, for every
/// program, so ⇧Enter is told from Enter.
pub const MODULE: &str = r#"-- thc_keys.lua: macOS / VS Code editing keys for thc (written by `thc setup wezterm`).
-- Used from your WezTerm config by the block `thc setup wezterm --yes` adds (marked
-- `-- thc: begin` … `-- thc: end`); `thc setup wezterm --undo --yes` removes both.
-- Rewritten by `thc setup wezterm --yes`; edit your own wezterm.lua instead of this file.
-- Every key here acts only while thc is the pane's foreground process; elsewhere the key is
-- passed through as if this file weren't there.
local wezterm = require 'wezterm'
local act = wezterm.action
local M = {}

-- Whether a foreground process (its path, as WezTerm reports it) is thc.
function M.is_thc(process)
  process = process or ''
  return process:match('/thc$') ~= nil or process == 'thc'
end

local seqs = {}

-- What a key does for a foreground process: { send = '<seq>' } in thc, else { pass = true }
-- (the key as typed, or ⌘V's normal paste).
function M.decide(key, mods, process)
  local seq = seqs[key .. '+' .. mods]
  if seq and M.is_thc(process) then
    return { send = seq }
  end
  return { pass = true }
end

-- In thc: `seq`. Anywhere else: the key itself (or `other`, an action).
local function thc_only(key, mods, seq, other)
  seqs[key .. '+' .. mods] = seq
  return {
    key = key, mods = mods,
    action = wezterm.action_callback(function(window, pane)
      local d = M.decide(key, mods, pane:get_foreground_process_name())
      if d.send then
        pane:send_text(d.send)
      else
        window:perform_action(other or act.SendKey { key = key, mods = mods }, pane)
      end
    end),
  }
end

M.keys = {
  thc_only('LeftArrow',  'CMD',       '\x1b[H'),     -- line start (Home)
  thc_only('RightArrow', 'CMD',       '\x1b[F'),     -- line end (End)
  thc_only('LeftArrow',  'CMD|SHIFT', '\x1b[1;2H'),  -- select to line start
  thc_only('RightArrow', 'CMD|SHIFT', '\x1b[1;2F'),  -- select to line end
  thc_only('UpArrow',    'CMD',       '\x1b[1;5H'),  -- document start (C-Home)
  thc_only('DownArrow',  'CMD',       '\x1b[1;5F'),  -- document end (C-End)
  thc_only('UpArrow',    'CMD|SHIFT', '\x1b[1;6H'),  -- select to document start
  thc_only('DownArrow',  'CMD|SHIFT', '\x1b[1;6F'),  -- select to document end
  thc_only('LeftArrow',  'OPT',       '\x1b[1;3D'),  -- word left
  thc_only('RightArrow', 'OPT',       '\x1b[1;3C'),  -- word right
  thc_only('LeftArrow',  'OPT|SHIFT', '\x1b[1;4D'),  -- select word left
  thc_only('RightArrow', 'OPT|SHIFT', '\x1b[1;4C'),  -- select word right
  thc_only('Backspace',  'CMD',       '\x15'),       -- delete to line start (⌃U)
  thc_only('Backspace',  'OPT',       '\x1b\x7f'),   -- delete word back
  -- ⌘V: in thc (on this Mac) thc reads the clipboard itself, so a copied screenshot is attached
  -- and text is pasted; anywhere else, WezTerm's own paste.
  thc_only('v',          'CMD',       '\x1b[118;9u', act.PasteFrom 'Clipboard'),
  -- ⌘X ⌘A ⌘Z ⇧⌘Z: thc's cut, select all, undo and redo (kitty's encoding of the keys).
  thc_only('x',          'CMD',       '\x1b[120;9u'),
  thc_only('a',          'CMD',       '\x1b[97;9u'),
  thc_only('z',          'CMD',       '\x1b[122;9u'),
  thc_only('z',          'CMD|SHIFT', '\x1b[122;10u'),
  -- ⌘[ ⌘]: back and forward through the places you've been (navigation.md §7).
  thc_only('[',          'CMD',       '\x1b[91;9u'),
  thc_only(']',          'CMD',       '\x1b[93;9u'),
  -- ⌘C: thc copies its own selection (as Markdown); a selection made in WezTerm itself
  -- (⌥-drag) is still copied the WezTerm way.
  {
    key = 'c', mods = 'CMD',
    action = wezterm.action_callback(function(window, pane)
      local wez = window:get_selection_text_for_pane(pane) or ''
      if M.is_thc(pane:get_foreground_process_name()) and wez == '' then
        pane:send_text('\x1b[99;9u')
      else
        window:perform_action(act.CopyTo 'Clipboard', pane)
      end
    end),
  },
}

seqs['c+CMD'] = '\x1b[99;9u'

function M.apply(config)
  config.keys = config.keys or {}
  for _, k in ipairs(M.keys) do
    table.insert(config.keys, k)
  end
  -- ⇧Enter told from Enter (a line break inside an item). This applies to every program in
  -- WezTerm, not only thc; thc asks only for the protocol's first flag.
  config.enable_kitty_keyboard = true
  return config
end

return M
"#;

/// The marked block thc keeps in your WezTerm config (one, recognised by its markers).
pub const BLOCK_BEGIN: &str = "-- thc: begin (macOS editing keys and ⌘V screenshots inside thc; delete this block to undo)";
pub const BLOCK_END: &str = "-- thc: end";

fn block() -> String {
    format!("{BLOCK_BEGIN}\npackage.path = wezterm.home_dir .. '/.config/wezterm/?.lua;' .. package.path\nrequire('thc_keys').apply(config)\n{BLOCK_END}\n")
}

/// The module's home: ~/.config/wezterm (the block's package.path names it).
fn config_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/wezterm"))
}

/// The config WezTerm actually loads: $WEZTERM_CONFIG_FILE, else ~/.config/wezterm/wezterm.lua,
/// else ~/.wezterm.lua.
pub fn user_file() -> Option<PathBuf> {
    if let Some(f) = std::env::var_os("WEZTERM_CONFIG_FILE").filter(|f| !f.is_empty()) {
        return Some(PathBuf::from(f));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    [home.join(".config/wezterm/wezterm.lua"), home.join(".wezterm.lua")].into_iter().find(|p| p.exists())
}

/// `text` with thc's block in place: replaced if it's there, else inserted before the last
/// `return config`. None: no `return config` to put it before.
pub fn with_block(text: &str) -> Option<String> {
    if let (Some(a), Some(b)) = (text.find(BLOCK_BEGIN), text.find(BLOCK_END)) {
        let end = b + BLOCK_END.len() + usize::from(text[b + BLOCK_END.len()..].starts_with('\n'));
        return Some(format!("{}{}{}", &text[..a], block(), &text[end..]));
    }
    let i = text.rfind("return config")?;
    let line_start = text[..i].rfind('\n').map_or(0, |n| n + 1);
    Some(format!("{}{}\n{}", &text[..line_start], block(), &text[line_start..]))
}

/// `text` without thc's block.
pub fn without_block(text: &str) -> String {
    match (text.find(BLOCK_BEGIN), text.find(BLOCK_END)) {
        (Some(a), Some(b)) if b > a => {
            let mut end = b + BLOCK_END.len();
            if text[end..].starts_with('\n') {
                end += 1;
            }
            // The blank line setup put after it, too.
            if text[end..].starts_with('\n') {
                end += 1;
            }
            format!("{}{}", &text[..a], &text[end..])
        }
        _ => text.to_string(),
    }
}

fn backup(f: &std::path::Path) -> Result<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let b = PathBuf::from(format!("{}.bak-{stamp}", f.display()));
    std::fs::copy(f, &b)?;
    Ok(b)
}

/// thc's own module, kept current: once `thc setup wezterm` has written it, a newer thc rewrites
/// it when its keys change (⌘[ ⌘] came later), and WezTerm reloads it. Only the module, never
/// the user's wezterm.lua; nothing when it was never set up. True when it was rewritten.
pub fn refresh_module() -> bool {
    let Some(module) = config_dir().map(|d| d.join("thc_keys.lua")) else { return false };
    match std::fs::read_to_string(&module) {
        Ok(cur) if cur != MODULE && cur.starts_with("-- thc_keys.lua") => {
            thc_core::sandbox::check(&module);
            std::fs::write(&module, MODULE).is_ok()
        }
        _ => false,
    }
}

pub fn setup(out: &mut Out, yes: bool, dry_run: bool, undo: bool) -> Result<()> {
    let dir = config_dir().ok_or_else(|| anyhow::anyhow!("no HOME"))?;
    let module = dir.join("thc_keys.lua");
    let yours = user_file();
    let text = yours.as_ref().and_then(|f| std::fs::read_to_string(f).ok());
    let wired = text.as_deref().is_some_and(|t| t.contains(BLOCK_BEGIN));
    let f = yours.as_ref().map(|f| thc_core::vault::tilde(f)).unwrap_or_else(|| "no WezTerm config found".into());
    let note = "the keys act only while thc runs in the pane; it also turns on WezTerm's kitty keyboard mode (enable_kitty_keyboard), for every program";
    if undo {
        if !yes || dry_run {
            out.line(format!("thc setup wezterm --undo --yes removes thc's block from {f} and {}", thc_core::vault::tilde(&module)));
            return Ok(());
        }
        if let (Some(file), Some(t)) = (&yours, &text) {
            if t.contains(BLOCK_BEGIN) {
                let b = backup(file)?;
                std::fs::write(file, without_block(t))?;
                out.line(format!("removed thc's block from {f} · backup {}", thc_core::vault::tilde(&b)));
            }
        }
        if module.exists() {
            std::fs::remove_file(&module)?;
            out.line(format!("removed {}", thc_core::vault::tilde(&module)));
        }
        return Ok(());
    }
    if out.json {
        out.json(&serde_json::json!({ "module": module, "config": yours, "wired": wired, "kitty_keyboard": "enable_kitty_keyboard = true, for every program" }));
    }
    if !yes || dry_run {
        if !out.json {
            out.line(format!("thc setup wezterm --yes writes {} and adds a marked block to {f} (backed up first)", thc_core::vault::tilde(&module)));
            out.line(format!("({note} · already set up: {})", if wired { "yes" } else { "no" }));
        }
        return Ok(());
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&module, MODULE)?;
    out.line(format!("wrote {} · ⌘←→ ⌘↑↓ (⇧ selects), ⌥←→, ⌘⌫ ⌥⌫, ⌘C ⌘X ⌘V ⌘A ⌘Z ⇧⌘Z, ⇧Enter; ⌘V pastes screenshots", thc_core::vault::tilde(&module)));
    out.line(note.to_string());
    match (&yours, &text) {
        (Some(file), Some(t)) => match with_block(t) {
            Some(new) if new == *t => out.line(format!("{f} already has thc's block")),
            Some(new) => {
                let b = backup(file)?;
                std::fs::write(file, new)?;
                out.line(format!("added thc's block to {f} · backup {} · WezTerm reloads it on save · thc setup wezterm --undo removes it", thc_core::vault::tilde(&b)));
            }
            None => {
                out.line(format!("{f} has no `return config` to put it before · add this yourself:"));
                for l in block().lines() {
                    out.line(format!("  {l}"));
                }
            }
        },
        _ => {
            out.line("no WezTerm config found · add this to your wezterm.lua, before `return config`:");
            for l in block().lines() {
                out.line(format!("  {l}"));
            }
        }
    }
    Ok(())
}

/// Machine setup's one question about WezTerm (zero-setup install): asked once, only in a
/// terminal, only when WezTerm is in use and thc's block isn't in its config yet. A yes runs
/// `thc setup wezterm --yes`; nothing is ever written without one.
pub fn offer(out: &mut Out) -> Result<()> {
    use std::io::{IsTerminal, Write};
    let in_wezterm = std::env::var("TERM_PROGRAM").is_ok_and(|t| t == "WezTerm") || std::env::var_os("WEZTERM_PANE").is_some();
    let wired = user_file().and_then(|f| std::fs::read_to_string(f).ok()).is_some_and(|t| t.contains(BLOCK_BEGIN));
    let Some(marker) = thc_core::vault::global_config_path().and_then(|p| p.parent().map(|d| d.join(".wezterm-asked"))) else { return Ok(()) };
    if !in_wezterm || wired || marker.exists() || !std::io::stdin().is_terminal() {
        return Ok(());
    }
    let f = user_file().map(|f| thc_core::vault::tilde(&f)).unwrap_or_else(|| "~/.wezterm.lua".into());
    eprint!("Set up WezTerm keys for thc (⌘←→ ⌘↑↓, ⌘V screenshots)? It adds a marked block to {f}, backed up first. [Y/n] ");
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    let read = std::io::stdin().read_line(&mut answer).unwrap_or(0);
    if let Some(d) = marker.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&marker, "asked\n");
    let a = answer.trim().to_lowercase();
    if read > 0 && answer.contains('\n') && (a.is_empty() || a.starts_with('y')) {
        return setup(out, true, false, false);
    }
    eprintln!("not now · thc setup wezterm --yes any time");
    Ok(())
}
