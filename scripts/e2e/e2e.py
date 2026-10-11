#!/usr/bin/env python3
"""Real-terminal and headless end-to-end runs of thc's editing and navigation goldens.

    scripts/e2e/run.sh [--headless] [--keep] [target] [scenario…]

Targets: thc-tui (thc's TUI on a scratch vault), fictty-notes (fictty's showcase notes editor,
built from the local fictty checkout) and thc-screen (thc's fictty screen, once it exists).

A real run starts its own WezTerm (its own config, class, HOME and socket, `--always-new-process`)
with the kitty keyboard protocol on and thc's ⌘ forwards (`thc setup wezterm --yes` into the
scratch HOME), so it tests what a user gets. Keys go through WezTerm's own key handling (the
config's bindings, then its defaults, then its encoder, via a lua queue), raw bytes and mouse
reports through `wezterm cli send-text`, and the result is read back from the terminal
(`wezterm cli get-text`) and from the app's own state (`thc ui`, `fictty get --state`).

Nothing touches the user's vaults, config, WezTerm config or daemon: everything runs under a
scratch HOME, THC_VAULT, THC_CONFIG_DIR, THC_CACHE_DIR, TMPDIR and FICTTY_SOCKET. The system
clipboard is the one shared thing (OSC 52 reaches it through WezTerm): its text is saved before a
real run and put back after (`--no-system-clipboard` leaves it alone and skips those checks).
"""

import argparse
import base64
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ in the checkout
sys.path.insert(0, str(Path(__file__).resolve().parent))
import notation  # noqa: E402

E2E = Path(__file__).resolve().parent
REPO = E2E.parent.parent
SCEN = E2E / "scenarios"
BUILD = REPO / "target" / "e2e"
WEZ = os.environ.get("WEZTERM", "/Applications/WezTerm.app/Contents/MacOS/wezterm")
FICTTY_DIR = Path(os.environ.get("FICTTY_DIR", Path.home() / "Documents/projects/fictty"))
TARGETS = ["thc-tui", "fictty-notes", "thc-screen"]
# How long a check may take to come true after the last input (a frame, a save, a refetch), in ms.
GRACE = int(os.environ.get("E2E_GRACE", "600"))
EDITOR_TARGETS = TARGETS

# ---- keys ------------------------------------------------------------------------------------------

NAMED = {"left": "LeftArrow", "right": "RightArrow", "up": "UpArrow", "down": "DownArrow", "bs": "Backspace",
         "del": "Delete", "enter": "Enter", "esc": "Escape", "tab": "Tab", "space": " ", "home": "Home",
         "end": "End", "pgup": "PageUp", "pgdn": "PageDown"}
THC_BASE = {"enter": "cr"}


def split_key(name):
    """`d-s-z` → ({'d', 's'}, 'z'). Modifiers: c ⌃, a ⌥, d ⌘, s ⇧ (fictty's names, any order)."""
    mods = set()
    while len(name) > 2 and name[1] == "-" and name[0] in "cads":
        mods.add(name[0])
        name = name[2:]
    return mods, name


def fictty_key(name):
    """fictty's canonical order: c- a- d- s-."""
    mods, base = split_key(name)
    if len(base) == 1 and base.isupper() and not mods:
        return base
    if len(base) == 1 and base.isupper():
        mods.add("s")
        base = base.lower()
    return "".join(f"{m}-" for m in "cads" if m in mods) + base


def thc_key(name):
    """A thc key-script token: ⌃ ⌥ ⇧ ⌘ in that order (`<c-m-s-d-x>`)."""
    mods, base = split_key(name)
    base = THC_BASE.get(base, base)
    if not mods and len(base) == 1:
        return "<lt>" if base == "<" else base
    pre = "".join({"c": "c-", "a": "m-", "s": "s-", "d": "d-"}[m] for m in "casd" if m in mods)
    return f"<{pre}{base}>"


def wez_key(name):
    """(key, mods) for WezTerm's SendKey, as macOS delivers a press: a shifted letter is its capital."""
    mods, base = split_key(name)
    key = NAMED.get(base, base)
    if re.fullmatch(r"f\d+", base):
        key = base.upper()
    if len(base) == 1 and base.isupper():
        mods.add("s")
    m = "|".join({"c": "CTRL", "a": "OPT", "d": "CMD", "s": "SHIFT"}[x] for x in "cads" if x in mods)
    return key, m or "NONE"


# Legacy sequences every decoder reads, for setting up a selection quickly.
SETUP_BYTES = {"c-home": "\x1b[1;5H", "right": "\x1b[C", "s-right": "\x1b[1;2C", "s-left": "\x1b[1;2D",
               "tab": "\t", "esc": "\x1b", "enter": "\r"}


def show_bytes(b):
    return "".join(c if 32 <= ord(c) < 127 else f"\\x{ord(c):02x}" for c in b)


# ---- scenarios -------------------------------------------------------------------------------------

def load_scenarios(names=()):
    out = []
    for f in sorted(SCEN.glob("*.json")):
        s = json.loads(f.read_text())
        s["_file"] = str(f.relative_to(REPO))
        if names and not any(n.lower() in (s["id"].lower(), f.stem.lower()) or f.stem.lower().startswith(n.lower() + "-") for n in names):
            continue
        out.append(s)
    return out


def targets_of(s):
    return s.get("targets", EDITOR_TARGETS if "setup" in s else ["thc-tui"])


def steps_of(s, target):
    """A scenario's steps for a target: its own (`steps_by_target`) where opening a page differs."""
    return s.get("steps_by_target", {}).get(target, s.get("steps", []))


def needs_terminal(s):
    return any("bytes" in st for st in s.get("steps", []) + [x for v in s.get("steps_by_target", {}).values() for x in v])


def for_target(e, target):
    """An expectation for one target: its own block (keyed by target name) over the shared keys."""
    out = {k: v for k, v in e.items() if k not in TARGETS}
    out.update(e.get(target, {}))
    return out


def expect_for(s, target):
    e = for_target(s.get("expect", {}), target)
    e.update(s.get("expect_by_target", {}).get(target, {}))
    return e


def seed_vault(s, thc):
    """A scenario's `vault`: thc command lines, {"apply": [ops]} for one `thc apply`, or
    {"page": TITLE, "notes": [TEXT | {"text", "id"}]} for a page with notes."""
    for l in s.get("vault", []):
        if isinstance(l, dict) and "apply" in l:
            thc("apply", "-", input="\n".join(json.dumps(x) for x in l["apply"]))
        elif isinstance(l, dict) and "page" in l:
            pid = json.loads(thc("--json", "page", "new", l["page"]))["nodes"][0]["id"]
            for t in l.get("notes", []):
                # A note is its text, or {"text", "id"} to know its id afterwards.
                t = t if isinstance(t, dict) else {"text": t}
                thc("add", "--plain", t["text"], "--under", pid, *(["--id", t["id"]] if "id" in t else []))
        else:
            thc(*l)


def known_for(s, target, mode):
    k = s.get("known", {})
    return k.get(f"{target}/{mode}") or k.get(target)


# ---- assertions ------------------------------------------------------------------------------------

def subset(want, got, path=""):
    """Every key of `want` is in `got` with the same value (recursively). Problems, as sentences."""
    if isinstance(want, dict):
        if not isinstance(got, dict):
            return [f"{path or '.'}: want an object, got {json.dumps(got)[:80]}"]
        probs = []
        for k, v in want.items():
            if k not in got:
                probs.append(f"{path}.{k}: missing")
            else:
                probs += subset(v, got[k], f"{path}.{k}")
        return probs
    return [] if want == got else [f"{path}: want {json.dumps(want)}, got {json.dumps(got)[:200]}"]


class Fail(Exception):
    pass


def check(cond, msg):
    if not cond:
        raise Fail(msg)


def check_vault(e, thc):
    """`vault`: [{q, count?, status?, text?, due?}], each a `thc q` and what its rows hold."""
    for q in e.get("vault", []):
        items = json.loads(thc("--json", "q", q["q"]))["items"]
        if "count" in q:
            check(len(items) == q["count"], f"vault {q['q']!r}: want {q['count']} items, got {[i['text'] for i in items]}")
        for k in ("status", "text", "due", "id"):
            if k in q:
                check(items and items[0].get(k) == q[k], f"vault {q['q']!r}: want {k}={q[k]!r}, got {[i.get(k) for i in items]}")


def visual_row(screen, needle, last):
    """The text of the screen row holding `needle` (its last such row, or its first), between the
    borders around it: one visual row of a wrapped editor line."""
    rows = [r for r in screen.split("\n") if needle in r]
    if not rows:
        raise Fail(f"{needle!r} isn't on screen:\n{compact(screen)}")
    row = rows[-1] if last else rows[0]
    i = row.rindex(needle) if last else row.index(needle)
    start = max(row.rfind(b, 0, i) for b in "│┃║") + 1
    ends = [j for j in (row.find(b, i) for b in "│┃║") if j >= 0]
    return row[start:min(ends) if ends else len(row)].strip()


def check_rows(e, screen, text, head):
    """`caret_at_row_start: W` / `caret_at_row_end: W`: the caret is at the start of the last
    visual row showing W, or at the end of the first, as drawn now (any width)."""
    if "caret_at_row_start" in e:
        seg = visual_row(screen, e["caret_at_row_start"], True)
        check(text[head:].startswith(seg), f"caret: want it at the start of the row {seg!r}, it's before {text[head:head + 30]!r}")
    if "caret_at_row_end" in e:
        seg = visual_row(screen, e["caret_at_row_end"], False)
        check(text[:head].rstrip(" ").endswith(seg) and len(text[:head]) - len(text[:head].rstrip(" ")) <= 1,
              f"caret: want it at the end of the row {seg!r}, it's after {text[max(0, head - 30):head]!r}")


def check_caret_line(e, line):
    """`caret_line_min` / `caret_line_max`: the caret's line (0-based; a note in thc's TUI)."""
    if "caret_line_min" in e:
        check(line >= e["caret_line_min"], f"caret: want it on line {e['caret_line_min']} or later, it's on {line}")
    if "caret_line_max" in e:
        check(line <= e["caret_line_max"], f"caret: want it on line {e['caret_line_max']} or earlier, it's on {line}")


def compact(screen):
    """A screen for a failure message: its non-blank rows, trimmed."""
    rows = [r.rstrip()[:160] for r in screen.split("\n") if r.strip()]
    rows = rows if len(rows) <= 40 else rows[:30] + ["…"] + rows[-8:]
    return "\n".join(rows)


def check_common(e, screen, clip=None, osc52=None):
    for t in ([e["screen"]] if isinstance(e.get("screen"), str) else e.get("screen", [])):
        check(t in screen, f"screen: want {t!r} on screen:\n{compact(screen)}")
    for t in ([e["no_screen"]] if isinstance(e.get("no_screen"), str) else e.get("no_screen", [])):
        check(t not in screen, f"screen: want no {t!r}:\n{compact(screen)}")
    # A row of the screen starts or ends with this (borders, bars and padding aside): where a
    # wrapped line's visual row begins or ends, whatever the width.
    rows = [r.strip(" │▌┃║") for r in screen.split("\n")]
    if "row_starts" in e:
        check(any(r.startswith(e["row_starts"]) for r in rows), f"screen: want a row starting {e['row_starts']!r}:\n{compact(screen)}")
    if "row_ends" in e:
        check(any(r.endswith(e["row_ends"]) for r in rows), f"screen: want a row ending {e['row_ends']!r}:\n{compact(screen)}")
    if "clip" in e and clip is not None:
        check(clip == e["clip"], f"clipboard: want {e['clip']!r}, got {clip!r}")
    if "clip" in e and osc52 is not None:
        check(osc52 == e["clip"], f"system clipboard (OSC 52 through WezTerm): want {e['clip']!r}, got {osc52!r}")


# ---- fictty ----------------------------------------------------------------------------------------

def fictty_rev():
    return os.environ.get("FICTTY_REV") or subprocess.run(["git", "-C", str(FICTTY_DIR), "rev-parse", "--short", "main"], capture_output=True, text=True, check=True).stdout.strip()


def setup_thc():
    """The thc whose `thc setup wezterm` writes the terminal's keys: E2E_SETUP_THC, else thc
    screen's build (the newest modules: thc_keys chained to fictty's), else the thc under test."""
    if os.environ.get("E2E_SETUP_THC"):
        return Path(os.environ["E2E_SETUP_THC"])
    b = build_screen()[0]
    return b or thc_bin()


def fictty_cli():
    """The fictty binary whose `setup wezterm` module the terminal installs: local main's."""
    return build_fictty(probe_too=False)[1]


def build_fictty(rev=None, probe_too=True):
    """The fictty binary and the probe, built from an export of a fictty commit (never in the
    checkout itself). Returns (dir, fictty binary, probe binary, rev)."""
    rev = rev or fictty_rev()
    d = BUILD / f"fictty-{rev}"
    src = d / "src"
    if not (src / "Cargo.toml").exists():
        src.mkdir(parents=True, exist_ok=True)
        tar = d / "src.tar"
        subprocess.run(["git", "-C", str(FICTTY_DIR), "archive", "--format=tar", "-o", str(tar), rev], check=True)
        subprocess.run(["tar", "-xf", str(tar), "-C", str(src)], check=True)
        # Built inside thc's tree (its gitignored target/), so it's its own workspace.
        toml = src / "Cargo.toml"
        if "[workspace]" not in toml.read_text():
            toml.write_text(toml.read_text() + "\n[workspace]\n")
    probe = d / "probe"
    probe.mkdir(exist_ok=True)
    (probe / "src").mkdir(exist_ok=True)
    shutil.copy(E2E / "fictty-probe" / "main.rs", probe / "src" / "main.rs")
    shutil.copy(E2E / "fictty-probe" / "app_fictty.rs", probe / "src" / "app.rs")
    (probe / "Cargo.toml").write_text(
        '[package]\nname = "fictty-probe"\nversion = "0.0.0"\nedition = "2024"\npublish = false\n\n'
        '[dependencies]\nfictty = { path = "../src" }\nserde_json = "1"\nanyhow = "1"\n\n[workspace]\n')
    env = dict(os.environ, CARGO_TARGET_DIR=str(d / "target"))
    for cwd in (src, probe) if probe_too else (src,):
        r = subprocess.run(["cargo", "build", "--release", "--quiet"], cwd=cwd, env=env, capture_output=True, text=True)
        if r.returncode:
            sys.exit(f"building {cwd} failed:\n{r.stderr[-3000:]}")
    return d, d / "target/release/fictty", d / "target/release/fictty-probe", rev


def fictty_store(text, extra=None):
    store = {"notes": [{"id": "e2e", "title": "E2E", "tag": "test"},
                       {"id": "groceries", "title": "Groceries", "tag": "home"},
                       {"id": "ideas", "title": "Ideas for the site", "tag": "work"}],
             "note/e2e": text if text is not None else "# E2E\n\nA note for the e2e harness.\n",
             "note/groceries": "# Groceries\n\n- Oat milk\n- Lemons\n",
             "note/ideas": "# Ideas for the site\n\n- A live demo\n"}
    store.update(extra or {})
    return store


def fictty_doc(state, editor="body", key="note/e2e"):
    """(text, anchor, head) of editor `editor` showing store entry `key`."""
    entry = state["store"].get(key)
    if entry is None:
        raise Fail(f"no store entry {key}")
    text = (entry.get("doc") or {}).get("text")
    if text is None:
        raise Fail(f"store entry {key} isn't a text")
    view = state["local"].get(editor, {}).get("editors", {}).get(key)
    if view is None:
        raise Fail(f"editor {editor} has no view of {key}")
    sel = view["selection"]
    r = sel["ranges"][sel.get("primary_index", sel.get("primary", 0))] if "ranges" in sel else sel
    return text, r["anchor"], r["head"]


# ---- thc -------------------------------------------------------------------------------------------

def thc_bin():
    b = Path(os.environ.get("THC_BIN", REPO / "target/debug/thc"))
    if not b.exists():
        sys.exit(f"no thc at {b}: cargo build -p thc first")
    return b


def thc_apply_lines(setup, page_title):
    """`thc apply` lines making a page whose notes are the golden's (ids keyed, so known)."""
    blocks, _, _ = notation.notes(setup)
    lines = [{"cmd": "add", "text": page_title, "inbox": True, "as": "page"}]
    # A page is a node with a title; `thc page new` makes one. The notes go under it.
    parents = []
    for i, b in enumerate(blocks):
        parents = parents[: b["depth"]]
        under = parents[-1] if parents else "$page"
        lines.append({"cmd": "add", "text": b["text"], "under": under, "as": f"n{i}"})
        props = {"style": "para"} if b["kind"] == "para" else {}
        if b["kind"] == "task":
            props["status"] = b["status"]
        if props:
            lines.append({"cmd": "set", "id": f"$n{i}", "props": props})
        parents.append(f"$n{i}")
    return blocks, lines[1:]


# ---- the real terminal -----------------------------------------------------------------------------

LUA = r"""
-- The e2e harness's WezTerm config (scripts/e2e). It runs a driver shell, takes key presses
-- from a queue file and gives each the treatment a real press gets: the config's own bindings
-- (thc's forwards among them), then WezTerm's defaults, then its key encoder (SendKey).
local wezterm = require 'wezterm'
local act = wezterm.action
local config = wezterm.config_builder()
local root = os.getenv('E2E_ROOT')
local env = wezterm.json_parse(io.open(root .. '/env.json'):read('*a'))

config.enable_kitty_keyboard = true
config.term = 'xterm-256color'
config.window_close_confirmation = 'NeverPrompt'
config.check_for_updates = false
config.initial_cols = 120
config.initial_rows = 36
config.font_size = 11
config.default_prog = { '/bin/bash', root .. '/driver.sh' }
config.set_environment_variables = env
config.keys = {}

local function norm(mods)
  local set = {}
  for m in (mods or ''):gmatch('[^|]+') do
    m = m:upper()
    if m == 'CMD' or m == 'SUPER' then m = 'SUPER' end
    if m == 'OPT' or m == 'META' or m == 'ALT' then m = 'ALT' end
    if m == 'CTRL' then m = 'CTRL' end
    if m ~= 'NONE' and m ~= '' then set[m] = true end
  end
  local out = {}
  for _, m in ipairs({ 'CTRL', 'ALT', 'SUPER', 'SHIFT' }) do
    if set[m] then table.insert(out, m) end
  end
  return table.concat(out, '|')
end

-- A capital letter is its letter with SHIFT (`K` + CTRL is ⌃⇧K, never ⌃K).
local function canon(key, mods)
  key = (key or ''):gsub('^mapped:', ''):gsub('^phys:', '')
  if #key == 1 and key:match('%u') then
    key = key:lower()
    mods = (mods or '') .. '|SHIFT'
  end
  return key, norm(mods)
end

-- WezTerm builds its key map from the list in order, so a later entry for the same key wins.
local function find(tbl, key, mods)
  local want_k, want_m = canon(key, mods)
  local found
  for _, k in ipairs(tbl or {}) do
    local kk, km = canon(k.key, k.mods)
    if kk == want_k and km == want_m then
      found = k
    end
  end
  return found
end

local function press(window, pane, key, mods)
  local m = mods
  local hit = find(config.keys, key, m)
  local how = 'encoder'
  if hit then
    how = 'binding'
  else
    hit = find(wezterm.gui.default_keys(), key, m)
    if hit then how = 'default' end
  end
  if hit then
    window:perform_action(hit.action, pane)
  else
    window:perform_action(act.SendKey { key = key, mods = mods }, pane)
  end
  return how
end

-- WezTerm reloads this config when a file it read changes (thc rewrites thc_keys.lua when its
-- copy differs), and a reload drops the old Lua state's timers: the poll starts again on
-- `window-config-reloaded`, and an older chain stops when it sees a newer generation.

local function poll()
  local f = io.open(root .. '/keyq', 'r')
  if f then
    local body = f:read('*a')
    f:close()
    os.remove(root .. '/keyq')
    local w = wezterm.gui and wezterm.gui.gui_windows()[1]
    local seq, hows = '0', {}
    for line in body:gmatch('[^\n]+') do
      local s, k, m = line:match('^(%d+)\t(.-)\t(.*)$')
      if s then
        seq = s
        local ok, how = pcall(press, w, w:active_pane(), k, m)
        table.insert(hows, ok and how or ("error: " .. tostring(how):gsub("[\t\n]", " ")))
      end
    end
    local a = io.open(root .. '/keyack.tmp', 'w')
    a:write(seq .. '\t' .. table.concat(hows, ','))
    a:close()
    os.rename(root .. '/keyack.tmp', root .. '/keyack')
  end
end

local function start()
  wezterm.GLOBAL.e2e_gen = (wezterm.GLOBAL.e2e_gen or 0) + 1
  local gen = wezterm.GLOBAL.e2e_gen
  local function tick()
    if wezterm.GLOBAL.e2e_gen ~= gen then
      return
    end
    pcall(poll)
    wezterm.time.call_after(0.02, tick)
  end
  wezterm.time.call_after(0.05, tick)
end

wezterm.on('window-config-reloaded', function(window, pane)
  start()
end)

wezterm.on('gui-startup', function(cmd)
  wezterm.mux.spawn_window(cmd or {})
  start()
end)

return config
"""

DRIVER = r"""#!/bin/bash
# The pane's shell: runs one command at a time from the harness, in the foreground with job
# control (so WezTerm sees the program, thc or fictty, as the pane's foreground process).
set -m
root="$E2E_ROOT"
echo "e2e driver ready"
n=0
while true; do
  read -r line < "$root/cmd.fifo" || continue
  [ "$line" = "exit" ] && exit 0
  n=$((n + 1))
  eval "$line"
  echo "$? $n" > "$root/done.tmp" && mv "$root/done.tmp" "$root/done"
  # Whatever the program left on: kitty keys, mouse modes, paste, the alternate screen.
  printf '\e[<u\e[?1000l\e[?1002l\e[?1003l\e[?1006l\e[?2004l\e[?1049l\e[2J\e[H'
  echo "e2e driver ready"
done
"""


class Terminal:
    """One WezTerm process with its own config, class, HOME and socket."""

    def __init__(self, target, keep=False, fictty=None, setup=None):
        # A short path: WezTerm's and thc's sockets live under it (sun_path is 104 bytes).
        self.root = Path(tempfile.mkdtemp(prefix="thce2e.", dir="/tmp"))
        self.home = self.root / "h"
        self.keep = keep
        self.seq = 0
        (self.home / ".config/wezterm").mkdir(parents=True)
        os.mkfifo(self.root / "cmd.fifo")
        (self.root / "driver.sh").write_text(DRIVER)
        self.env = pane_env(self.root, self.home)
        (self.root / "env.json").write_text(json.dumps(self.env))
        cfg = self.home / ".config/wezterm/wezterm.lua"
        cfg.write_text(LUA)
        # The keys exactly as `thc setup wezterm --yes` writes them for a user: thc_keys.lua, and
        # (from the screens branch on) fictty.lua beside it, which thc_keys chains to in a pane
        # showing a fictty screen. `setup` is the thc whose setup runs (default: thc screen's).
        r = subprocess.run([str(setup or thc_bin()), "setup", "wezterm", "--yes"], env=dict(self.sandbox_env(), WEZTERM_CONFIG_FILE=str(cfg)), capture_output=True, text=True)
        if r.returncode or "thc: begin" not in cfg.read_text():
            raise RuntimeError(f"thc setup wezterm failed: {r.stdout}{r.stderr}")
        self.modules = [f"thc_keys ({Path(str(setup or thc_bin())).parent.parent.parent.name or 'thc'})"]
        if (self.home / ".config/wezterm/fictty.lua").exists():
            self.modules.append("fictty.lua (written and chained by thc setup)")
            fictty = None
        # Older thc: fictty's own `fictty setup wezterm`, wired in after thc's block as its
        # comment says (`require('fictty').apply(config)`); WezTerm keeps the later binding.
        if fictty:
            r = subprocess.run([str(fictty), "setup", "wezterm"], env=dict(self.sandbox_env(), WEZTERM_CONFIG_FILE=str(cfg)), capture_output=True, text=True)
            mod = self.home / ".config/wezterm/fictty.lua"
            if not mod.exists():
                mod.write_text(subprocess.run([str(fictty), "setup", "wezterm", "--print"], capture_output=True, text=True, check=True).stdout)
            t = cfg.read_text()
            if "require('fictty')" not in t and 'require("fictty")' not in t:
                i = t.rindex("return config")
                t = t[:i] + "package.path = wezterm.home_dir .. '/.config/wezterm/?.lua;' .. package.path\nrequire('fictty').apply(config)\n" + t[i:]
                cfg.write_text(t)
            self.modules.append("fictty")
        self.cfg = cfg
        wenv = {k: v for k, v in os.environ.items() if not k.startswith("WEZTERM_")}
        wenv.update(HOME=str(self.home), E2E_ROOT=str(self.root))
        self.log = open(self.root / "wezterm.log", "w")
        self.proc = subprocess.Popen([WEZ, "--config-file", str(cfg), "start", "--always-new-process", "--class", f"dev.thought.e2e.{target}"],
                                     env=wenv, stdout=self.log, stderr=subprocess.STDOUT, start_new_session=True)
        self.sock = None
        deadline = time.time() + 20
        while time.time() < deadline:
            socks = list((self.home / ".local/share/wezterm").glob("gui-sock-*"))
            if socks and socks[0].is_socket():
                self.sock = str(socks[0])
                break
            time.sleep(0.1)
        if not self.sock:
            raise RuntimeError(f"WezTerm never opened its socket: {(self.root / 'wezterm.log').read_text()[-2000:]}")
        panes = json.loads(self.cli("list", "--format", "json"))
        self.pane = str(panes[0]["pane_id"])
        self.wait_text("e2e driver ready", 15)

    def sandbox_env(self):
        e = {k: v for k, v in os.environ.items() if not k.startswith(("THC_", "WEZTERM_", "FICTTY"))}
        e.update(self.env if hasattr(self, "env") else {})
        e.update(getattr(self, "scenario_env", {}))
        e.update(HOME=str(self.home))
        return e

    def fresh_vault(self, sid):
        """Each scenario its own vault and cache, under this terminal's scratch root."""
        tag = re.sub("[^a-zA-Z0-9]", "", sid)
        self.scenario_env = {"THC_VAULT": str(self.root / f"v-{tag}"), "THC_CACHE_DIR": str(self.root / f"c-{tag}")}
        # From a folder of its own: `thc init` writes a project file there (never in HOME).
        w = self.root / f"w-{tag}"
        w.mkdir(exist_ok=True)
        r = subprocess.run([str(thc_bin()), "init", self.scenario_env["THC_VAULT"]], env=self.sandbox_env(), cwd=w, capture_output=True, text=True)
        if r.returncode:
            raise Fail(f"thc init: {r.stderr.strip()}")

    # -- wezterm cli, always against this instance's own socket and pane
    def cli(self, *args, input=None):
        assert self.sock and self.sock.startswith(str(self.root)), "never the user's WezTerm"
        env = {k: v for k, v in os.environ.items() if not k.startswith("WEZTERM_")}
        env["WEZTERM_UNIX_SOCKET"] = self.sock
        r = subprocess.run([WEZ, "cli", "--no-auto-start", *args], env=env, capture_output=True, text=True, input=input)
        if r.returncode:
            raise RuntimeError(f"wezterm cli {args}: {r.stderr}")
        return r.stdout

    def text(self):
        return self.cli("get-text", "--pane-id", self.pane)

    def send(self, data):
        self.cli("send-text", "--pane-id", self.pane, "--no-paste", input=data)

    def key(self, name):
        """A press through WezTerm's bindings and encoder. Returns how WezTerm took it."""
        self.seq += 1
        k, m = wez_key(name)
        tmp = self.root / "keyq.tmp"
        tmp.write_text(f"{self.seq}\t{k}\t{m}\n")
        os.rename(tmp, self.root / "keyq")
        deadline = time.time() + 5
        while time.time() < deadline:
            try:
                seq, how = (self.root / "keyack").read_text().split("\t")
                if int(seq) == self.seq:
                    return how
            except (FileNotFoundError, ValueError):
                pass
            time.sleep(0.01)
        raise Fail(f"WezTerm never took key {name}")

    def run(self, cmdline):
        """Start a program in the pane (the driver runs it in the foreground)."""
        try:
            (self.root / "done").unlink()
        except FileNotFoundError:
            pass
        deadline = time.time() + 10
        while True:
            try:
                fd = os.open(self.root / "cmd.fifo", os.O_WRONLY | os.O_NONBLOCK)
                break
            except OSError:
                if time.time() > deadline:
                    raise Fail("the pane's driver isn't waiting for a command (a program still running?)")
                time.sleep(0.05)
        env = " ".join(f"{k}={v}" for k, v in getattr(self, "scenario_env", {}).items())
        os.write(fd, (f"{env} {cmdline}" if env else cmdline).encode() + b"\n")
        os.close(fd)

    def finished(self):
        return (self.root / "done").exists()

    def wait_done(self, timeout=10):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if self.finished():
                return True
            time.sleep(0.05)
        return False

    def wait_text(self, s, timeout=10):
        deadline = time.time() + timeout
        while time.time() < deadline:
            t = self.text()
            # A long line wraps on screen: look across the wrap too.
            if s in t or s in "".join(l.rstrip() if len(l) < 20 else l for l in t.split("\n")) or s in t.replace("\n", ""):
                return t
            time.sleep(0.1)
        raise Fail(f"never showed {s!r}:\n{compact(self.text())}")

    def locate(self, spec):
        """The cell of `find|ing`'s bar (or the start of `find`) on screen: (x, y), 0-based."""
        needle = spec.replace("|", "")
        at = spec.index("|") if "|" in spec else 0
        for y, line in enumerate(self.text().split("\n")):
            i = line.find(needle)
            if i >= 0:
                return len(line[:i]) + at, y
        raise Fail(f"{needle!r} isn't on screen:\n{compact(self.text())}")

    def close(self):
        # Never block on the driver: it may be busy running a program that won't read.
        try:
            fd = os.open(self.root / "cmd.fifo", os.O_WRONLY | os.O_NONBLOCK)
            os.write(fd, b"exit\n")
            os.close(fd)
        except OSError:
            pass
        try:
            os.killpg(self.proc.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            self.proc.wait(5)
        except subprocess.TimeoutExpired:
            os.killpg(self.proc.pid, signal.SIGKILL)
        shutil.rmtree(self.root, ignore_errors=True)


def sgr(button, x, y, release=False):
    return f"\x1b[<{button};{x + 1};{y + 1}{'m' if release else 'M'}"


def mouse_bytes(step, term):
    """SGR 1006 reports for a mouse step, at cells found on the terminal's screen."""
    if "wheel" in step:
        x, y = term.locate(step["at"]) if "at" in step else (40, 12)
        b = 65 if step["wheel"] == "down" else 64
        return [sgr(b, x, y)] * step.get("n", 1)
    if "drag" in step:
        (x1, y1), (x2, y2) = term.locate(step["drag"][0]), term.locate(step["drag"][1])
        out = []
        if step.get("double"):
            out += [sgr(0, x1, y1), sgr(0, x1, y1, True)]
        out.append(sgr(0, x1, y1))
        cells = [(x, y1) for x in range(x1 + 1, x2 + 1)] if y1 == y2 else [(x2, y) for y in range(y1 + 1, y2 + 1)]
        out += [sgr(32, x, y) for x, y in cells]
        out.append(sgr(0, x2, y2, True))
        return out
    kind = next(k for k in ("click", "dclick", "tclick", "sclick") if k in step)
    x, y = term.locate(step[kind])
    btn = 4 if kind == "sclick" else 0
    n = {"click": 1, "sclick": 1, "dclick": 2, "tclick": 3}[kind]
    out = []
    for _ in range(n):
        out += [sgr(btn, x, y), sgr(btn, x, y, True)]
    return out


MOUSE = ("click", "dclick", "tclick", "sclick", "drag", "wheel")


class Clipboard:
    """The system clipboard's text, saved before a real run and put back after."""

    def __init__(self, on):
        self.on = on and shutil.which("/usr/bin/pbpaste") is not None
        self.saved = subprocess.run(["/usr/bin/pbpaste"], capture_output=True).stdout if self.on else None

    def set(self, text):
        if self.on:
            subprocess.run(["/usr/bin/pbcopy"], input=text.encode())

    def get(self):
        return subprocess.run(["/usr/bin/pbpaste"], capture_output=True, text=True).stdout if self.on else None

    def restore(self):
        if self.on and self.saved is not None:
            subprocess.run(["/usr/bin/pbcopy"], input=self.saved)


# ---- targets ---------------------------------------------------------------------------------------

class Result:
    def __init__(self, sid, target, mode, status, detail="", sent=None, file=""):
        self.sid, self.target, self.mode, self.status, self.detail, self.sent, self.file = sid, target, mode, status, detail, sent or [], file


def run_steps_real(term, steps, sent, between=None, thc=None):
    for st in steps:
        if "thc" in st:
            thc(st["thc"], st.get("actor"))
            sent.append(f"thc {' '.join(st['thc'])}" + (f" (as {st['actor']})" if st.get("actor") else ""))
        elif "key" in st:
            how = term.key(st["key"])
            k, m = wez_key(st["key"])
            sent.append(f"SendKey {{key={k!r}, mods={m!r}}} → {how}")
        elif "bytes" in st:
            term.send(st["bytes"])
            sent.append(f"bytes {show_bytes(st['bytes'])}")
        elif "type" in st:
            term.send(st["type"])
            sent.append(f"text {st['type']!r}")
        elif "paste" in st:
            term.send(f"\x1b[200~{st['paste']}\x1b[201~")
            sent.append(f"bracketed paste {st['paste']!r}")
        elif any(k in st for k in MOUSE):
            for b in mouse_bytes(st, term):
                term.send(b)
                sent.append(f"mouse {show_bytes(b)}")
                time.sleep(0.03)
        elif "wait" in st:
            time.sleep(st["wait"] / 1000)
            continue
        elif "assert" in st:
            if between:
                within(lambda: between(st["assert"]), st.get("within", GRACE))
            continue
        time.sleep(float(os.environ.get("E2E_SETTLE", "0.12")))


def within(f, ms):
    """f() passes now, or within `ms` milliseconds (polled)."""
    deadline = time.time() + ms / 1000
    while True:
        try:
            return f()
        except Fail:
            if time.time() >= deadline:
                raise
            time.sleep(0.1)


def rule_under(screen, label):
    """The accent rule (━) runs under the header label `label`, and only there."""
    rows = screen.split("\n")
    for y, row in enumerate(rows[:-1]):
        i = row.find(f" {label} ")
        if i >= 0 and "━" in rows[y + 1]:
            under = rows[y + 1]
            span = range(i + 1, i + 1 + len(label))
            ok = all(x < len(under) and under[x] == "━" for x in span)
            check(ok, f"the accent rule isn't under {label!r}:\n{row.rstrip()}\n{under.rstrip()}")
            return
    raise Fail(f"no header with {label!r} and a rule under it:\n{compact(screen)}")


def fictty_headless_steps(steps, editor="body", origin=None):
    out = []
    for st in steps:
        if "key" in st:
            out.append({"key": fictty_key(st["key"])})
        elif "type" in st:
            out += [{"key": "space" if c == " " else ("enter" if c == "\n" else c)} for c in st["type"]]
        elif "paste" in st:
            out.append({"msg": {"msg": "paste", "text": st["paste"]}})
        elif any(k in st for k in ("click", "dclick", "tclick", "sclick")):
            kind = next(k for k in ("click", "dclick", "tclick", "sclick") if k in st)
            spec = st[kind]
            p = {"id": editor, "find": spec.replace("|", ""), "at": spec.index("|") if "|" in spec else 0, "origin": origin, "extend": kind == "sclick"}
            out += [{"point": p}] * {"click": 1, "sclick": 1, "dclick": 2, "tclick": 3}[kind]
        elif "drag" in st:
            a, b = st["drag"]
            pa = {"id": editor, "find": a.replace("|", ""), "at": a.index("|") if "|" in a else 0, "origin": origin}
            pb = {"id": editor, "find": b.replace("|", ""), "at": b.index("|") if "|" in b else 0, "origin": origin}
            out += [{"point": pa}] * (2 if st.get("double") else 1) + [{"drag": pb}]
        elif "wheel" in st:
            raise Fail("skip: the wheel needs a terminal (fictty's Headless has no hit map)")
        elif "assert" in st:
            out.append({"snap": json.dumps(st["assert"])})
        elif "bytes" in st:
            raise Fail("skip: raw bytes need a terminal")
    return out


class FicttyNotes:
    """fictty's showcase notes editor."""
    name = "fictty-notes"
    kind = "fictty"

    def __init__(self):
        self.dir, self.fictty, self.probe, self.rev = build_fictty()
        self.show = self.dir / "src/examples/showcase"

    def available(self):
        return True, f"fictty {self.rev}"

    # -- headless
    def headless(self, s):
        e = expect_for(s, self.name)
        if "clip_before" in s:
            raise Fail("skip: ⌘V pastes the system clipboard, which Headless doesn't have")
        tmp = Path(tempfile.mkdtemp(prefix="e2e-fictty-"))
        try:
            setup_steps = []
            text = None
            if "setup" in s:
                text, anchor, head = notation.plain(s["setup"])
                setup_steps = [{"key": "tab"}] * 2 + [{"key": "c-home"}] + [{"key": "right"}] * anchor
                setup_steps += [{"key": "s-right" if head >= anchor else "s-left"}] * abs(head - anchor)
                setup_steps.append({"snap": "setup"})
            ui = json.loads((self.show / "notes.json").read_text())
            ui["store"] = {**ui.get("store", {}), **fictty_store(text, s.get("fictty_store"))}
            ui_file = tmp / "notes.json"
            ui_file.write_text(json.dumps(ui))
            plan = {"ui": str(ui_file), "dir": str(self.show), "size": [100, 30], "themes": "themes.json",
                    "steps": setup_steps + fictty_headless_steps(steps_of(s, self.name), origin=(text or "").split("\n")[0] or None)}
            r = subprocess.run([str(self.probe)], input=json.dumps(plan), capture_output=True, text=True)
            if r.returncode:
                raise Fail(f"probe: {r.stderr.strip()[-1500:]}")
            snaps = json.loads(r.stdout)["snaps"]
            for sn in snaps:
                if sn["label"] == "setup":
                    got = notation.show_plain(*fictty_doc(sn["state"]))
                    check(got == notation.norm_plain(s["setup"]), f"setup: want {notation.norm_plain(s['setup'])}, got {got} (focus {sn['state'].get('focus')})")
                elif sn["label"] != "end":
                    self.assert_fictty(json.loads(sn["label"]), sn["state"], sn["screen"], sn["copied"])
            end = snaps[-1]
            self.assert_fictty(e, end["state"], end["screen"], end["copied"])
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    def doc_of(self, state):
        return fictty_doc(state)

    def assert_fictty(self, e, state, screen, copied, osc52=None):
        if "caret_line_min" in e or "caret_line_max" in e:
            text, _, head = self.doc_of(state)
            check_caret_line(e, text[:head].count("\n"))
        if "caret_at_row_start" in e or "caret_at_row_end" in e:
            text, _, head = self.doc_of(state)
            check_rows(e, screen, text, head)
        if "doc" in e:
            got = notation.show_plain(*self.doc_of(state))
            check(got == notation.norm_plain(e["doc"]), f"doc: want {notation.norm_plain(e['doc'])}\n      got  {got}")
        if "fictty_state" in e:
            probs = subset(e["fictty_state"], state)
            check(not probs, "state: " + "; ".join(probs))
        if "store" in e:
            for k, want in e["store"].items():
                ent = state["store"].get(k, {})
                val = ent.get("value", ent.get("text"))
                if isinstance(want, dict) and "len" in want:
                    check(isinstance(val, list) and len(val) == want["len"], f"store {k}: want {want['len']} items, got {json.dumps(val)[:200]}")
                elif isinstance(want, dict) and "contains" in want:
                    check(want["contains"] in json.dumps(val, ensure_ascii=False), f"store {k}: want {want['contains']!r} in {json.dumps(val, ensure_ascii=False)[:300]}")
        clip = (copied[-1] if copied else None) if copied is not None else None
        if "clip" in e and copied is not None:
            check(clip == e["clip"], f"clipboard (copied): want {e['clip']!r}, got {clip!r}")
        if "toast" in e:
            check(e["toast"] in json.dumps(state, ensure_ascii=False), f"toast: want {e['toast']!r} in the state")
        if copied is not None:
            e = {k: v for k, v in e.items() if k != "clip"}
        check_common(e, screen, osc52=osc52)

    # -- real
    def real(self, term, s, clipboard, sent):
        e = expect_for(s, self.name)
        text = None
        if "setup" in s:
            text, anchor, head = notation.plain(s["setup"])
        ui = json.loads((self.show / "notes.json").read_text())
        ui["store"] = {**ui.get("store", {}), **fictty_store(text, s.get("fictty_store"))}
        run = term.root / f"run-{s['id']}"
        shutil.copytree(self.show, run, dirs_exist_ok=True)
        (run / "notes.json").write_text(json.dumps(ui))
        sock = term.root / "f.sock"
        if "clip_before" in s:
            clipboard.set(s["clip_before"])
        elif "clip" in e:
            clipboard.set(f"e2e-sentinel-{s['id']}")
        term.run(f"cd {run} && FICTTY_SOCKET={sock} {self.fictty} run notes.json --themes themes.json")
        fenv = dict(os.environ, FICTTY_SOCKET=str(sock))

        def state():
            r = subprocess.run([str(self.fictty), "get", "--state"], env=fenv, capture_output=True, text=True)
            if r.returncode:
                raise Fail(f"fictty get --state: {r.stderr or r.stdout}")
            return json.loads(r.stdout)

        deadline = time.time() + 10
        while not sock.exists() and time.time() < deadline:
            time.sleep(0.05)
        time.sleep(0.3)
        try:
            if text is not None:
                for _ in range(4):
                    if state().get("focus") == "body":
                        break
                    term.send(SETUP_BYTES["tab"])
                    time.sleep(0.1)
                seq = [SETUP_BYTES["c-home"]] + [SETUP_BYTES["right"]] * anchor + [SETUP_BYTES["s-right" if head >= anchor else "s-left"]] * abs(head - anchor)
                term.send("".join(seq))
                time.sleep(0.3)
                got = notation.show_plain(*fictty_doc(state()))
                check(got == notation.norm_plain(s["setup"]), f"setup: want {notation.norm_plain(s['setup'])}, got {got}")
            run_steps_real(term, steps_of(s, self.name), sent, between=lambda a: self.assert_fictty(a, state(), term.text(), None, osc52=clipboard.get() if "clip" in a else None))
            time.sleep(0.3)
            self.assert_fictty(e, state(), term.text(), None, osc52=clipboard.get() if "clip" in e else None)
        finally:
            self.stop(term, sock)

    def stop(self, term, sock):
        # Only this run's fictty: its binary lives in thc's target/e2e, never the user's.
        if not term.finished():
            subprocess.run(["pkill", "-TERM", "-f", f"^{self.fictty} run"], capture_output=True)
            if not term.wait_done(5):
                subprocess.run(["pkill", "-KILL", "-f", f"^{self.fictty} run"], capture_output=True)
                term.wait_done(3)
        try:
            sock.unlink()
        except FileNotFoundError:
            pass


class ThcTui:
    """thc's TUI on a scratch vault."""
    name = "thc-tui"
    kind = "thc"

    def __init__(self):
        self.bin = thc_bin()

    def available(self):
        return True, subprocess.run([str(self.bin), "--version"], capture_output=True, text=True).stdout.strip()

    def headless(self, s):
        raise Fail("skip: thc's headless half runs in cargo test (crates/thc-tui/src/flows/e2e.rs)")

    def thc(self, term, *args, check_ok=True, input=None, actor=None):
        env = term.sandbox_env()
        if actor:
            env["THC_ACTOR"] = actor
        r = subprocess.run([str(self.bin), *args], env=env, capture_output=True, text=True, input=input)
        if check_ok and r.returncode:
            raise Fail(f"thc {' '.join(args)}: {r.stderr.strip()}")
        return r.stdout

    def ui(self, term, *words):
        return self.thc(term, "ui", "send", "--raw", *words)

    def ui_state(self, term):
        return json.loads(self.ui(term, "state.get", "no-history"))

    def doc_now(self, term, page, ids):
        """The open document as notes (saved first) and the caret and anchor."""
        self.ui(term, "trace.checkpoint")
        st = self.ui_state(term)
        tree = json.loads(self.thc(term, "show", self.page_id, "--json", "--depth", "9"))
        blocks, idx = [], {}

        def walk(n, depth):
            for c in n.get("children", []):
                idx[c["id"]] = len(blocks)
                kind = "task" if c.get("status") else ("para" if c.get("props", {}).get("style") == "para" else "bullet")
                blocks.append({"depth": depth, "kind": kind, "status": c.get("status"), "text": c.get("text", "")})
                walk(c, depth + 1)
        walk(tree.get("node", tree), 0)
        d = st.get("document") or {}

        def pos(nid, byte):
            if not nid:
                return (len(blocks), 0)
            i = idx.get(nid)
            if i is None:
                raise Fail(f"the caret's note {nid} isn't on the page")
            t = blocks[i]["text"].encode()
            return (i, len(t[:byte].decode(errors="ignore")))
        head = pos(d.get("caret_id"), d.get("caret_byte", 0))
        a = d.get("anchor")
        anchor = pos(a["id"], a["byte"]) if a else None
        if head[0] == len(blocks) or (anchor and anchor[0] == len(blocks)):
            blocks.append({"depth": 0, "kind": "para", "status": None, "text": ""})
        return blocks, anchor, head, st

    def real(self, term, s, clipboard, sent):
        e = expect_for(s, self.name)
        term.fresh_vault(s["id"])
        clip_file = term.root / "clip"
        clip_file.write_text(s.get("clip_before", ""))
        if "clip" in e:
            clipboard.set(f"e2e-sentinel-{s['id']}")
        page = None
        if "setup" in s:
            page = f"g{re.sub('[^a-z0-9]', '', s['id'].lower())}z"
            blocks, lines = thc_apply_lines(s["setup"], page)
            out = json.loads(self.thc(term, "--json", "page", "new", page))
            pid = out["nodes"][0]["id"]
            lines = [{**l, "under": pid if l.get("under") == "$page" else l.get("under")} for l in lines]
            # A golden of one empty line is the page's own fresh line: no notes to make.
            lines = [] if all(b["text"] == "" for b in blocks) else lines
            if lines:
                self.thc(term, "apply", "-", input="\n".join(json.dumps(l) for l in lines))
            self.page_id = pid
            term.run(f"{self.bin} p {page}")
        else:
            seed_vault(s, lambda *a, input=None: self.thc(term, *a, input=input))
            term.run(f"{self.bin} tui")
        try:
            self.wait_tui(term)
            if page:
                _, anchor, head = notation.notes(s["setup"])
                n = self.offset(blocks, anchor)
                m = self.offset(blocks, head)
                term.send(SETUP_BYTES["c-home"] + SETUP_BYTES["right"] * n + SETUP_BYTES["s-right" if m >= n else "s-left"] * abs(m - n))
                time.sleep(0.4)
                b, a, h, _ = self.doc_now(term, page, None)
                got = notation.show_notes(*self.trim(b, a, h))
                check(got == notation.norm_notes(s["setup"]), f"setup: want {notation.norm_notes(s['setup'])}\n       got  {got}")
            run_steps_real(term, steps_of(s, self.name), sent, between=lambda a: self.assert_thc(term, for_target(a, self.name), page, clip_file, clipboard),
                           thc=lambda args, actor: self.thc(term, *args, actor=actor))
            time.sleep(0.3)
            within(lambda: self.assert_thc(term, e, page, clip_file, clipboard), s.get("within", GRACE))
        finally:
            self.stop(term)

    @staticmethod
    def offset(blocks, p):
        """A (note, char) position as arrow presses from the document's start."""
        i, c = p
        return sum(len(b["text"]) + 1 for b in blocks[:i]) + c

    @staticmethod
    def trim(blocks, anchor, head):
        """Drop the fresh empty line a page opens with, unless the caret or selection is on it."""
        while len(blocks) > 1 and blocks[-1]["text"] == "" and head[0] < len(blocks) - 1 and (anchor is None or anchor[0] < len(blocks) - 1):
            blocks = blocks[:-1]
        return blocks, anchor, head

    def assert_thc(self, term, e, page, clip_file, clipboard):
        screen = term.text()
        if "caret_line_min" in e or "caret_line_max" in e:
            _, _, h, _ = self.doc_now(term, page, None)
            check_caret_line(e, h[0])
        if "caret_at_row_start" in e or "caret_at_row_end" in e:
            b, _, h, _ = self.doc_now(term, page, None)
            check_rows(e, screen, b[h[0]]["text"] if h[0] < len(b) else "", h[1])
        if "doc" in e:
            b, a, h, _ = self.doc_now(term, page, None)
            got = notation.show_notes(*self.trim(b, a, h))
            check(got == notation.norm_notes(e["doc"]), f"doc: want {notation.norm_notes(e['doc'])}\n     got  {got}")
        if "thc_state" in e:
            st = self.ui_state(term)
            probs = subset(e["thc_state"], st)
            check(not probs, "state: " + "; ".join(probs))
        check_vault(e, lambda *a: self.thc(term, *a))
        if "rule_under" in e:
            rule_under(screen, e["rule_under"])
        clip = clip_file.read_text() if "clip" in e else None
        check_common(e, screen, clip=clip, osc52=clipboard.get() if "clip" in e and clipboard.on else None)

    def wait_tui(self, term):
        deadline = time.time() + 15
        while time.time() < deadline:
            r = subprocess.run([str(self.bin), "ui", "ls", "--json"], env=term.sandbox_env(), capture_output=True, text=True)
            if r.returncode == 0 and json.loads(r.stdout or "{}").get("sessions"):
                time.sleep(0.3)
                return
            if term.finished():
                raise Fail(f"thc exited at start:\n{term.text()}")
            time.sleep(0.1)
        raise Fail(f"thc's TUI never answered on its socket:\n{term.text()}")

    def stop(self, term):
        r = subprocess.run([str(self.bin), "ui", "ls", "--json"], env=term.sandbox_env(), capture_output=True, text=True)
        try:
            for t in json.loads(r.stdout or "{}").get("sessions", []):
                os.kill(int(t["pid"]), signal.SIGTERM)
        except (ValueError, KeyError, ProcessLookupError):
            pass
        if not term.wait_done(8):
            raise Fail("thc didn't exit on SIGTERM")


def build_screen():
    """thc with `thc screen`: THC_SCREEN_BIN, or built from THC_SCREEN_REV (the screens branch,
    default f8af743, PR #95) against an export of the fictty commit it was written for
    (THC_SCREEN_FICTTY_REV, default 197ea78), all under target/e2e."""
    if os.environ.get("THC_SCREEN_BIN"):
        return Path(os.environ["THC_SCREEN_BIN"]), os.environ.get("THC_SCREEN_FICTTY_REV", "197ea78"), os.environ.get("THC_SCREEN_PROBE")
    rev = os.environ.get("THC_SCREEN_REV", "f8af743")
    frev = os.environ.get("THC_SCREEN_FICTTY_REV", "197ea78")
    fd, _, _, _ = build_fictty(frev, probe_too=False)
    d = BUILD / f"screen-{rev}-on-{frev}"
    src = d / "src"
    if not (src / "Cargo.toml").exists():
        src.mkdir(parents=True, exist_ok=True)
        r = subprocess.run(["git", "-C", str(REPO), "archive", "--format=tar", "-o", str(d / "src.tar"), rev], capture_output=True, text=True)
        if r.returncode:
            return None, f"no commit {rev} here ({r.stderr.strip()})", None
        subprocess.run(["tar", "-xf", str(d / "src.tar"), "-C", str(src)], check=True)
    for toml in src.glob("crates/*/Cargo.toml"):
        t = toml.read_text()
        if re.search(r"^fictty = ", t, re.M):
            toml.write_text(re.sub(r"^fictty = .*$", f'fictty = {{ path = "{fd / "src"}" }}', t, flags=re.M))
    r = subprocess.run(["cargo", "build", "-p", "thc", "--quiet"], cwd=src, env=dict(os.environ, CARGO_TARGET_DIR=str(d / "target")), capture_output=True, text=True)
    if r.returncode:
        return None, f"building {rev} failed: {r.stderr[-1500:]}", None
    # The headless half: fictty-probe on thc screen's own app (app_screen.rs), from the same tree.
    probe = d / "probe"
    (probe / "src").mkdir(parents=True, exist_ok=True)
    shutil.copy(E2E / "fictty-probe" / "main.rs", probe / "src" / "main.rs")
    shutil.copy(E2E / "fictty-probe" / "app_screen.rs", probe / "src" / "app.rs")
    (probe / "Cargo.toml").write_text(
        '[package]\nname = "screen-probe"\nversion = "0.0.0"\nedition = "2024"\npublish = false\n\n[dependencies]\n'
        f'fictty = {{ path = "{fd / "src"}" }}\nthc-screen = {{ path = "{src / "crates/thc-screen"}" }}\n'
        f'thc-core = {{ path = "{src / "crates/thc-core"}" }}\nserde_json = "1"\nanyhow = "1"\n\n[workspace]\n')
    r = subprocess.run(["cargo", "build", "--quiet"], cwd=probe, env=dict(os.environ, CARGO_TARGET_DIR=str(d / "target")), capture_output=True, text=True)
    if r.returncode:
        return None, f"building the screen probe failed: {r.stderr[-1500:]}", None
    return d / "target/debug/thc", frev, d / "target/debug/screen-probe"


class ThcScreen(FicttyNotes):
    """`thc screen`: thc's views on fictty. Seeded like the TUI (a scratch vault through the CLI),
    read back from the terminal, fictty's own state (`fictty get --state` on its socket) and the
    vault."""
    name = "thc-screen"
    kind = "fictty"

    def __init__(self):
        self.bin, self.frev, self.probe = build_screen()
        self.fictty = None
        if self.bin:
            self.fictty = build_fictty(self.frev, probe_too=False)[1]

    def available(self):
        if not self.bin:
            return False, self.frev
        r = subprocess.run([str(self.bin), "screen", "--help"], capture_output=True, text=True)
        return r.returncode == 0, f"{self.bin} (fictty {self.frev})" if r.returncode == 0 else "this thc has no `thc screen`"

    def env(self, root):
        e = {k: v for k, v in os.environ.items() if not k.startswith(("THC_", "WEZTERM_", "FICTTY"))}
        e.update(pane_env(root, root / "h"))
        return e

    # -- editor goldens: a page's Markdown in the `doc` editor (store entry page/<id>)
    def title(self, s):
        return f"g{re.sub('[^a-z0-9]', '', s['id'].lower())}z"

    def header(self, s):
        """What the page's text starts with: up to 1697cb4 its heading and a blank line (the title
        was text in the editor); from 18b2762 on, nothing (the title is a heading row outside)."""
        old = os.environ.get("THC_SCREEN_REV", "f8af743") in ("5ca8d24", "1697cb4")
        return f"# {self.title(s)}\n\n" if old else ""

    def doc_of(self, state, s=None):
        """(text, anchor, head) of the page editor, its heading left out (offsets shift with it)."""
        s = s or self.current
        key = f"page/{self.page_id}"
        entry = state["store"].get(key)
        if entry is None:
            raise Fail(f"no store entry {key} (keys: {sorted(state['store'])[:12]})")
        text = (entry.get("doc") or {}).get("text")
        if text is None:
            raise Fail(f"store entry {key} isn't a text: {json.dumps(entry)[:200]}")
        # The page's editor: `doc` up to 1697cb4, `<tab>_doc` after (pages_doc, journal_doc).
        views = [(nid, l["editors"][key]) for nid, l in state["local"].items() if key in (l.get("editors") or {})]
        if not views:
            raise Fail(f"no editor has a view of {key} (editors: {[n for n, l in state['local'].items() if l.get('editors')]})")
        focus = state.get("focus")
        view = next((v for n, v in views if n == focus), views[0][1])
        sel = view["selection"]
        r = sel["ranges"][sel.get("primary_index", 0)]
        h = self.header(s)
        if not text.startswith(h):
            raise Fail(f"the page's text lost its heading {h!r}: {text[:80]!r}")
        n = len(h)
        if min(r["anchor"], r["head"]) < n:
            # The heading is text in this editor: say so rather than draw a mark outside the golden.
            body = notation.show_plain(text[n:], max(r["anchor"] - n, 0), max(r["head"] - n, 0))
            raise Fail(f"the {'caret' if r['anchor'] == r['head'] else 'selection'} reaches into the page heading "
                       f"(anchor {r['anchor'] - n}, caret {r['head'] - n} from the body's start): {body}")
        return text[n:], r["anchor"] - n, r["head"] - n

    def editor_id(self):
        old = os.environ.get("THC_SCREEN_REV", "f8af743") in ("5ca8d24", "1697cb4")
        return "doc" if old else "pages_doc"

    def origin(self, s):
        """The text the editor's first row shows, for headless pointer cells."""
        if self.header(s):
            return f"# {self.title(s)}"
        text, _, _ = notation.plain(s["setup"])
        return text.split("\n")[0] or None

    def page_text(self, s):
        text, _, _ = notation.plain(s["setup"])
        return self.header(s) + text

    def place_keys(self, s):
        _, anchor, head = notation.plain(s["setup"])
        n = len(self.header(s))
        return ["c-home"] + ["right"] * (n + anchor) + ["s-right" if head >= anchor else "s-left"] * abs(head - anchor)

    def new_page(self, s, thc):
        out = json.loads(thc("--json", "page", "new", self.title(s)))
        self.page_id = out["nodes"][0]["id"]
        self.current = s
        return self.page_id

    def headless_editor(self, s):
        if not self.probe:
            raise Fail("skip: no screen probe built (THC_SCREEN_PROBE)")
        e = expect_for(s, self.name)
        if "clip_before" in s:
            raise Fail("skip: ⌘V pastes the system clipboard, which Headless doesn't have")
        root = Path(tempfile.mkdtemp(prefix="thce2e-h.", dir="/tmp"))
        try:
            (root / "h").mkdir()
            (root / "w").mkdir()
            env = self.env(root)
            subprocess.run([str(self.bin), "init", str(root / "vault")], env=env, capture_output=True, cwd=root / "w")

            def thc(*a, input=None):
                r = subprocess.run([str(self.bin), *a], env=env, capture_output=True, text=True, cwd=root / "w", input=input)
                if r.returncode:
                    raise Fail(f"thc {' '.join(a)}: {r.stderr.strip()}")
                return r.stdout
            pid = self.new_page(s, thc)
            steps = [{"msg": {"msg": "write", "key": f"page/{pid}", "write": "put", "value": self.page_text(s), "by": "agent"}}]
            steps += [{"key": k} for k in self.place_keys(s)] + [{"snap": "setup"}]
            steps += fictty_headless_steps(steps_of(s, self.name), editor=self.editor_id(), origin=self.origin(s))
            plan = {"vault": str(root / "vault"), "view": "page", "id": pid, "size": [120, 32], "steps": steps}
            r = subprocess.run([str(self.probe)], input=json.dumps(plan), capture_output=True, text=True, env=env, cwd=root / "w")
            if r.returncode:
                raise Fail(f"screen probe: {r.stderr.strip()[-1500:]}")
            snaps = json.loads(r.stdout)["snaps"]
            for sn in snaps:
                if sn["label"] == "setup":
                    got = notation.show_plain(*self.doc_of(sn["state"], s))
                    check(got == notation.norm_plain(s["setup"]), f"setup: want {notation.norm_plain(s['setup'])}\n       got  {got}")
                elif sn["label"] != "end":
                    self.assert_fictty(for_target(json.loads(sn["label"]), self.name), sn["state"], sn["screen"], sn["copied"])
            end = snaps[-1]
            self.assert_fictty(e, end["state"], end["screen"], end["copied"])
        finally:
            shutil.rmtree(root, ignore_errors=True)

    def real_editor(self, term, s, clipboard, sent):
        e = expect_for(s, self.name)
        term.fresh_vault(s["id"])

        def thc(*a, input=None):
            r = subprocess.run([str(self.bin), *a], env=term.sandbox_env(), capture_output=True, text=True, cwd=term.root, input=input)
            if r.returncode:
                raise Fail(f"thc {' '.join(a)}: {r.stderr.strip()}")
            return r.stdout
        pid = self.new_page(s, thc)
        clip_file = term.root / "clip"
        clip_file.write_text(s.get("clip_before", ""))
        if "clip_before" in s:
            clipboard.set(s["clip_before"])
        elif "clip" in e:
            clipboard.set(f"e2e-sentinel-{s['id']}")
        sock = term.root / "s.sock"
        term.run(f"{self.bin} screen --view page --id {pid} --socket {sock}")
        fenv = {**term.sandbox_env(), "FICTTY_SOCKET": str(sock)}

        def fictty(*a, input=None):
            r = subprocess.run([str(self.fictty), *a], env=fenv, capture_output=True, text=True, input=input)
            if r.returncode:
                raise Fail(f"fictty {' '.join(a)}: {r.stderr or r.stdout}")
            return r.stdout

        def state():
            return json.loads(fictty("get", "--state"))
        deadline = time.time() + 15
        while not sock.exists() and time.time() < deadline:
            if term.finished():
                raise Fail(f"thc screen exited:\n{compact(term.text())}")
            time.sleep(0.05)
        time.sleep(0.5)
        try:
            # The golden's text, through fictty's hosted store: thc's host saves it to the vault.
            fictty("put", f"page/{pid}", "-", input=json.dumps(self.page_text(s)))
            time.sleep(0.3)
            term.send("".join(SETUP_BYTES[k] for k in self.place_keys(s)))
            within(lambda: check(notation.show_plain(*self.doc_of(state(), s)) == notation.norm_plain(s["setup"]),
                                 f"setup: want {notation.norm_plain(s['setup'])}\n       got  {notation.show_plain(*self.doc_of(state(), s))}"), GRACE)
            run_steps_real(term, steps_of(s, self.name), sent,
                           between=lambda a: self.assert_fictty(for_target(a, self.name), state(), term.text(), None, osc52=clipboard.get() if "clip" in a else None))
            time.sleep(0.3)
            within(lambda: self.assert_fictty(e, state(), term.text(), None, osc52=clipboard.get() if "clip" in e else None), s.get("within", GRACE))
        except Fail as f:
            if os.environ.get("E2E_DUMP"):
                try:
                    Path(os.environ["E2E_DUMP"], f"{s['id']}-{self.name}.json").write_text(json.dumps(state(), indent=1, ensure_ascii=False))
                except Fail:
                    pass
            raise f
        finally:
            if not term.finished():
                subprocess.run(["pkill", "-TERM", "-f", f"^{self.bin} screen --view page --id {pid} --socket {sock}"], capture_output=True)
                if not term.wait_done(5):
                    subprocess.run(["pkill", "-KILL", "-f", f"^{self.bin} screen --view page --id {pid} --socket {sock}"], capture_output=True)
                    term.wait_done(3)

    def headless(self, s):
        if "setup" in s:
            return self.headless_editor(s)
        keys = []
        for st in steps_of(s, self.name):
            if "key" in st:
                keys.append(fictty_key(st["key"]))
            elif "type" in st:
                keys += ["space" if c == " " else c for c in st["type"]]
            elif "wait" in st or "assert" in st:
                continue
            else:
                raise Fail(f"skip: {next(iter(st))} steps need the live screen")
        root = Path(tempfile.mkdtemp(prefix="thce2e-h.", dir="/tmp"))
        try:
            (root / "h").mkdir()
            env = self.env(root)

            def thc(*a, input=None):
                return subprocess.run([str(self.bin), *a], env=env, capture_output=True, text=True, cwd=root / "h", input=input).stdout
            (root / "w").mkdir()
            subprocess.run([str(self.bin), "init", str(root / "vault")], env=env, capture_output=True, cwd=root / "w")
            seed_vault(s, thc)
            r = subprocess.run([str(self.bin), "screen", "--render", "120x32", *(["--keys", ",".join(keys)] if keys else [])], env=env, capture_output=True, text=True, cwd=root)
            if r.returncode:
                raise Fail(f"thc screen --render: {r.stderr.strip()}")
            e = expect_for(s, self.name)
            if {"fictty_state", "selected_row", "focus_not"} & set(e):
                raise Fail("skip: asserts on the live screen's state (--render has none)")
            check_common(e, r.stdout)
            if "rule_under" in e:
                rule_under(r.stdout, e["rule_under"])
            check_vault(e, thc)
        finally:
            shutil.rmtree(root, ignore_errors=True)

    def real(self, term, s, clipboard, sent):
        if "setup" in s:
            return self.real_editor(term, s, clipboard, sent)
        e = expect_for(s, self.name)
        term.fresh_vault(s["id"])

        def thc(*a, actor=None, input=None):
            env = {**term.sandbox_env(), **({"THC_ACTOR": actor} if actor else {})}
            r = subprocess.run([str(self.bin), *a], env=env, capture_output=True, text=True, cwd=term.root, input=input)
            if r.returncode:
                raise Fail(f"thc {' '.join(a)}: {r.stderr.strip()}")
            return r.stdout
        seed_vault(s, thc)
        sock = term.root / "s.sock"
        term.run(f"{self.bin} screen --socket {sock}")
        fenv = {**term.sandbox_env(), "FICTTY_SOCKET": str(sock)}

        def state():
            r = subprocess.run([str(self.fictty), "get", "--state"], env=fenv, capture_output=True, text=True)
            if r.returncode:
                raise Fail(f"fictty get --state: {r.stderr or r.stdout}")
            return json.loads(r.stdout)

        def assert_all(a):
            screen = term.text()
            check_common(a, screen)
            if "rule_under" in a:
                rule_under(screen, a["rule_under"])
            if "fictty_state" in a:
                probs = subset(a["fictty_state"], state())
                check(not probs, "state: " + "; ".join(probs))
            if "focus_not" in a:
                f = state().get("focus")
                check(f != a["focus_not"], f"focus: want anything but {a['focus_not']!r}, it's {f!r}")
            if "selected_row" in a:
                row = selected_row(state(), a["selected_row"].get("list"))
                for k, v in a["selected_row"].items():
                    if k != "list":
                        check(row.get(k) == v, f"selected row: want {k}={v!r}, got {json.dumps(row, ensure_ascii=False)[:300]}")
            check_vault(a, thc)

        deadline = time.time() + 15
        while not sock.exists() and time.time() < deadline:
            if term.finished():
                raise Fail(f"thc screen exited:\n{compact(term.text())}")
            time.sleep(0.05)
        time.sleep(0.5)
        try:
            run_steps_real(term, steps_of(s, self.name), sent, between=lambda a: assert_all(for_target(a, self.name)), thc=lambda args, actor: thc(*args, actor=actor))
            time.sleep(0.3)
            within(lambda: assert_all(e), s.get("within", GRACE))
        except Fail as f:
            try:
                st = state()
                info = f"(fictty focus: {st.get('focus')!r}, status: {st.get('status')!r})"
                if os.environ.get("E2E_DUMP"):
                    Path(os.environ["E2E_DUMP"], f"{s['id']}-{self.name}.json").write_text(json.dumps(st, indent=1, ensure_ascii=False))
            except Fail:
                info = ""
            raise Fail(f"{f}\n{info}") from None
        finally:
            if not term.finished():
                subprocess.run(["pkill", "-TERM", "-f", f"^{self.bin} screen --socket {sock}"], capture_output=True)
                if not term.wait_done(5):
                    subprocess.run(["pkill", "-KILL", "-f", f"^{self.bin} screen --socket {sock}"], capture_output=True)
                    term.wait_done(3)


def selected_row(state, list_id=None):
    """The row a fictty list shows selected: its view state's `selected` index into the rows its
    source gave (the focused list, or `list_id`)."""
    lid = list_id or state.get("focus")
    node = find_node(state["ui"]["root"], lid)
    if node is None:
        raise Fail(f"no list {lid!r} (focus {state.get('focus')!r})")
    sel = state.get("local", {}).get(lid, {}).get("selected", 0)
    bind = node.get("bind", "")
    src, _, path = bind.partition(".")
    data = state.get("data", {}).get(src, {}).get("value")
    if data is None:
        data = (state.get("store", {}).get(src) or {}).get("value")
    for part in [p for p in path.split(".") if p]:
        data = (data or {}).get(part)
    if node.get("type") == "tree" and isinstance(data, list):
        # A tree's selection counts its shown rows: each row, then its children (none folded here).
        flat = []

        def walk(rows):
            for r in rows:
                flat.append(r)
                walk(r.get("children") or [])
        walk(data)
        data = flat
    if not isinstance(data, list) or sel >= len(data):
        raise Fail(f"list {lid!r}: no row {sel} in {json.dumps(data, ensure_ascii=False)[:200]}")
    return data[sel]


def find_node(n, nid):
    if not isinstance(n, dict):
        return None
    if n.get("id") == nid:
        return n
    for c in n.get("children", []) or []:
        f = find_node(c, nid)
        if f:
            return f
    return None


def make_target(name):
    return {"thc-tui": ThcTui, "fictty-notes": FicttyNotes, "thc-screen": ThcScreen}[name]()


def pane_env(root, home):
    """What every program in the real terminal gets: scratch everything, and a pbcopy/pbpaste
    that keep thc's own clipboard writes in a file (OSC 52 still reaches WezTerm)."""
    shims = root / "bin"
    shims.mkdir(exist_ok=True)
    clip = root / "clip"
    for tool, body in (("pbcopy", f'cat > "{clip}"'), ("pbpaste", f'cat "{clip}" 2>/dev/null')):
        p = shims / tool
        p.write_text(f"#!/bin/sh\n{body}\n")
        p.chmod(0o755)
    (root / "t").mkdir(exist_ok=True)
    return {
        "E2E_ROOT": str(root), "HOME": str(home), "TMPDIR": str(root / "t") + "/", "THC_VAULT": str(root / "vault"),
        "THC_CONFIG_DIR": str(root / "cfg"), "THC_CACHE_DIR": str(root / "cache"), "THC_SOCKET": str(root / "d.sock"),
        "XDG_CACHE_HOME": str(root / "xdg"), "XDG_CONFIG_HOME": str(home / ".config"), "THC_NO_UPDATE_CHECK": "1",
        "THC_TEST": "1", "THC_TEST_ROOT": str(root), "THC_SETUP_APP": "none", "THC_SETUP_LOGIN": "skip",
        "THC_NOTIFY": "log", "PATH": f"{shims}:/usr/bin:/bin:/usr/sbin:/sbin", "TERM_PROGRAM": "WezTerm",
        "THC_ACTOR": "human", "FICTTY_SOCKET": str(root / "f.sock"),
    }


# ---- running ---------------------------------------------------------------------------------------

def classify(r, s):
    k = known_for(s, r.target, r.mode)
    if r.status == "fail" and k:
        r.status = "xfail"
        r.detail = f"[{k}] " + r.detail
    elif r.status == "pass" and k:
        r.status = "xpass"
        r.detail = f"known failure no longer fails: {k}"
    return r


def run_headless(target, scenarios):
    out = []
    for s in scenarios:
        if target.name not in targets_of(s):
            continue
        t0 = time.time()
        try:
            if needs_terminal(s):
                raise Fail("skip: sends raw bytes, needs a terminal")
            target.headless(s)
            r = Result(s["id"], target.name, "headless", "pass")
        except Fail as f:
            msg = str(f)
            r = Result(s["id"], target.name, "headless", "skip" if msg.startswith("skip:") else "fail", msg)
        r.file = s["_file"]
        r.secs = time.time() - t0
        out.append(classify(r, s) if r.status != "skip" else r)
        report_one(out[-1])
    return out


def run_real(target, scenarios, keep, clip_on):
    out = []
    chosen = [s for s in scenarios if target.name in targets_of(s)]
    if not chosen:
        return out
    if not Path(WEZ).exists():
        sys.exit(f"no WezTerm at {WEZ} (set WEZTERM)")
    # fictty's forwards for fictty's targets (E2E_FICTTY_KEYS=1 adds them to any, =0 leaves them out).
    want = os.environ.get("E2E_FICTTY_KEYS", "1" if target.kind == "fictty" else "0") == "1"
    term = Terminal(target.name, keep, fictty=fictty_cli() if want else None, setup=setup_thc())
    print(f"   WezTerm key modules: {', '.join(term.modules)}")
    root = term.root
    clipboard = Clipboard(clip_on)
    failed = False
    try:
        for s in chosen:
            t0 = time.time()
            sent = []
            try:
                target.real(term, s, clipboard, sent)
                r = Result(s["id"], target.name, "real", "pass", sent=sent)
            except Fail as f:
                msg = str(f)
                r = Result(s["id"], target.name, "real", "skip" if msg.startswith("skip:") else "fail", msg, sent=sent)
            except Exception as ex:  # noqa: BLE001
                import traceback
                r = Result(s["id"], target.name, "real", "fail", f"harness error: {ex!r}\n{traceback.format_exc(limit=-3)}", sent=sent)
            r.file = s["_file"]
            r.secs = time.time() - t0
            if r.status != "skip":
                classify(r, s)
            failed |= r.status == "fail"
            out.append(r)
            report_one(r)
            if r.status == "fail" and keep:
                print(f"--keep: leaving the window open (socket {term.sock}); close it with: kill {term.proc.pid}")
                return out
    finally:
        clipboard.restore()
        if not (keep and failed):
            term.close()
    return out


PROBE_KEYS = ["d-c", "d-x", "d-v", "d-a", "d-z", "d-s-z", "d-o", "d-[", "d-]", "d-left", "d-right", "d-up", "d-down",
              "d-s-left", "d-s-right", "d-s-up", "d-s-down", "a-left", "a-right", "a-s-left", "a-s-right", "d-bs",
              "a-bs", "d-del", "a-del", "del", "bs", "c-k", "c-t", "c-w", "c-z", "s-left", "s-enter", "s-tab", "esc", "A"]


def build_keydump():
    """keydump (scripts/e2e/keydump), built twice: as `keydump` and as `thc`, so WezTerm's
    foreground-process check (thc_keys.lua's is_thc) sees one as thc."""
    d = BUILD / "keydump"
    (d / "src").mkdir(parents=True, exist_ok=True)
    shutil.copy(E2E / "keydump" / "main.rs", d / "src" / "main.rs")
    (d / "Cargo.toml").write_text('[package]\nname = "keydump"\nversion = "0.0.0"\nedition = "2021"\npublish = false\n\n'
                                  '[[bin]]\nname = "keydump"\npath = "src/main.rs"\n\n[[bin]]\nname = "thc"\npath = "src/main.rs"\n\n[workspace]\n')
    r = subprocess.run(["cargo", "build", "--release", "--quiet"], cwd=d, env=dict(os.environ, CARGO_TARGET_DIR=str(d / "target")), capture_output=True, text=True)
    if r.returncode:
        sys.exit(r.stderr)
    return d / "target/release"


def run_keys(keys, keep):
    """What WezTerm sends for each key, to a program named thc and to any other, for the kitty
    flags thc (1) and fictty (1|4) push. The truth table the forwards and decoders must agree on."""
    bins = build_keydump()
    term = Terminal("keys", keep, fictty=fictty_cli() if os.environ.get("E2E_FICTTY_KEYS", "1") == "1" else None, setup=setup_thc())
    print(f"WezTerm key modules: {', '.join(term.modules)}")
    table = {}
    try:
        # A program named thc (thc's forwards), any other program, and one that says it's a fictty
        # screen (the `fictty` user var: fictty's forwards), with thc's and fictty's kitty flags.
        for prog, flags, var in (("thc", 1, ""), ("thc", 5, ""), ("keydump", 1, ""), ("keydump", 5, ""), ("keydump", 5, "fictty")):
            col = f"{prog}/{flags}" + (f"+{var}" if var else "")
            log = term.root / f"keys-{prog}-{flags}{var}.log"
            log.write_text("")
            term.run(f"{bins / prog} {flags} {log} {var}".rstrip())
            term.wait_text(f"keydump ready (flags {flags})", 10)
            time.sleep(0.2)
            table.setdefault("(flags query)", {})[col] = (log.read_text().strip().replace("\n", " ") or "(no answer)", "-")
            if True:
                for k in keys:
                    before = log.read_text()
                    how = term.key(k)
                    time.sleep(0.15)
                    got = log.read_text()[len(before):].strip().replace("\n", " ")
                    table.setdefault(k, {})[col] = (got or "(nothing)", how)
                subprocess.run(["pkill", "-TERM", "-f", f"^{bins / prog} {flags}"], capture_output=True)
                term.wait_done(5)
    finally:
        if not keep:
            term.close()
    cols = ["thc/1", "thc/5", "keydump/1", "keydump/5", "keydump/5+fictty"]
    print(f"{'key':10} {'SendKey':28} " + " ".join(f"{c:22}" for c in cols) + "  via")
    for k, row in table.items():
        wk, wm = wez_key(k)
        print(f"{k:10} {wk + ' ' + wm:28} " + " ".join(f"{row.get(c, ('', ''))[0]:22}" for c in cols) + "  " + ",".join(sorted({v[1] for v in row.values()})))
    return table


def report_one(r):
    mark = {"pass": "PASS ", "fail": "FAIL ", "skip": "skip ", "xfail": "XFAIL", "xpass": "XPASS"}[r.status]
    print(f"{mark} {r.target:13} {r.mode:8} {r.sid:10} {getattr(r, 'secs', 0):5.1f}s  {r.file}")
    if r.status in ("fail", "xfail", "xpass") and r.detail:
        for line in r.detail.split("\n"):
            print(f"        {line}")
    if r.status in ("fail", "xfail") and r.sent:
        print("        sent: " + " · ".join(r.sent))
    sys.stdout.flush()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("target", nargs="?", default="all", help="thc-tui, fictty-notes, thc-screen or all")
    ap.add_argument("scenarios", nargs="*", help="scenario ids or file stems (default: every one)")
    ap.add_argument("--headless", action="store_true", help="no terminal: fictty's Headless (thc's runs in cargo test)")
    ap.add_argument("--keep", action="store_true", help="leave the window open on the first failure")
    ap.add_argument("--no-system-clipboard", action="store_true", help="never read or set the system clipboard")
    ap.add_argument("--json", help="write the results here as JSON")
    ap.add_argument("--keys", action="store_true", help="print what WezTerm sends for each key (to thc and to other programs)")
    a = ap.parse_args()
    if a.keys:
        t = run_keys(a.scenarios or PROBE_KEYS, a.keep)
        if a.json:
            Path(a.json).write_text(json.dumps(t, indent=1, ensure_ascii=False))
        return
    if a.target not in TARGETS + ["all"]:
        a.scenarios.insert(0, a.target)
        a.target = "all"
    scen = load_scenarios(a.scenarios)
    results = []
    times = {}
    for name in (TARGETS if a.target == "all" else [a.target]):
        t = make_target(name)
        ok, what = t.available()
        print(f"== {name} ({what}){'' if ok else ': unavailable, skipped'}")
        if not ok:
            continue
        t0 = time.time()
        results += run_headless(t, scen) if a.headless else run_real(t, scen, a.keep, not a.no_system_clipboard)
        times[name] = time.time() - t0
    counts = {}
    for r in results:
        counts[r.status] = counts.get(r.status, 0) + 1
    print("== " + " · ".join(f"{v} {k}" for k, v in sorted(counts.items())) + " · " + " · ".join(f"{k} {v:.0f}s" for k, v in times.items()))
    if a.json:
        Path(a.json).write_text(json.dumps([{**r.__dict__} for r in results], indent=1, ensure_ascii=False))
    sys.exit(1 if counts.get("fail") or counts.get("xpass") else 0)


if __name__ == "__main__":
    main()
