#!/usr/bin/env python3
"""Run the thc demo one-liner the way a reader does, `… | sh`, from an empty home, and check it.

    scripts/try-test.py                                  # web/public/try.sh on this checkout's HEAD, no network
    scripts/try-test.py web/public/try/slug.sh           # a frozen copy, at the commit it pins
    scripts/try-test.py https://thoughtcontrol.app/try.sh --github --ref main
    scripts/try-test.py --pty --expect Today --second "thc add 'From a second terminal' >/dev/null"

By default it fetches thc from this checkout (a file:// remote, so nothing goes over the network)
at --ref (this checkout's HEAD; it must be committed), sets up with THC_TRY_NO_RUN=1, then, as a
second terminal would through ~/.thc-try/bin/thc: adds a task, completes another, and renders a
frame of the screen (`thc screen --render WxH` when the binary has it, else THC_TUI_SNAPSHOT=WxH
thc tui) that must show the seeded notes and the new one.

--pty also runs the screen for real, piped into `sh` inside a fresh pseudo-terminal: it waits for
each --expect text to be drawn, sends each --key, then runs each --second command with only
~/.thc-try/bin added to PATH while the screen is up: `thc ui ls` must find it, and a task added
there must show on it (read back with `thc ui render`).

Either way it checks the reader's real thc is untouched: ~/thought, ~/.config/thought and the
daemon's socket folder (/tmp/thc-<uid>) have the same files, sizes and times afterwards, and no
UI socket of the sandbox is advertised outside it. Exits 0 when everything held, 1 when not.

The home is empty but Rust isn't reinstalled: CARGO_HOME and RUSTUP_HOME keep pointing at yours,
and CARGO_TARGET_DIR (default: this checkout's target/) lets the build reuse your cache.
"""
import argparse
import fcntl
import json
import os
import pty
import pwd
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KEYS = {"enter": b"\r", "esc": b"\x1b", "tab": b"\t", "down": b"\x1b[B", "up": b"\x1b[A", "ctrl-q": b"\x11"}
# Tasks the sample seed (scripts/seed-sample.sh) puts on Today: a frame must show some.
SEEDED = ["Draft Q4 OKRs", "Water plants", "Call dentist", "Dentist appointment", "Summarize unread newsletters"]
NOTE = "Water the ferns before the weekend"
LIVE = "Repot the basil on the sill"


def visible(raw: bytes) -> str:
    text = raw.decode("utf-8", "replace")
    return re.sub(r"\x1b\[[0-9;?<>=]*[A-Za-z~]|\x1b[\]_P][^\x07\x1b]*(\x07|\x1b\\)?|\x1b[()][A-Z0-9]|\x1b[=>78]", "", text)


def drew(want: str, raw: bytes) -> bool:
    """Whether the text was drawn. Spaces don't count: the screen only redraws cells that
    changed, so a space already there is skipped and words run together in the output."""
    return want.replace(" ", "") in re.sub(r"\s", "", visible(raw))


def tree(path: str) -> dict:
    """Every file and folder under path, with its size and modification time (stat only)."""
    out = {}
    if not os.path.lexists(path):
        return out
    for d, dirs, files in os.walk(path):
        for n in dirs + files:
            p = os.path.join(d, n)
            try:
                st = os.lstat(p)
            except FileNotFoundError:
                continue
            out[os.path.relpath(p, path)] = (st.st_size, st.st_mtime_ns)
    st = os.lstat(path)
    out["."] = (0, st.st_mtime_ns)
    return out


def real_places():
    home = pwd.getpwuid(os.getuid()).pw_dir
    return {
        "~/thought": os.path.join(home, "thought"),
        "~/.config/thought": os.path.join(home, ".config/thought"),
        "~/.thc-try": os.path.join(home, ".thc-try"),
        f"/tmp/thc-{os.getuid()}": f"/tmp/thc-{os.getuid()}",
    }


def changed(before: dict, after: dict) -> list:
    keys = set(before) | set(after)
    return sorted(k for k in keys if before.get(k) != after.get(k))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("script", nargs="?", default=os.path.join(ROOT, "web/public/try.sh"), help="a URL or a local file")
    ap.add_argument("--ref", help="the version to fetch (default: this checkout's HEAD; a frozen copy's own pin with --pinned)")
    ap.add_argument("--pinned", action="store_true", help="pass no version: run the script's own default (a frozen copy's pin)")
    ap.add_argument("--github", action="store_true", help="fetch from GitHub, as a reader does, instead of this checkout")
    ap.add_argument("--render", default="120x32", help="the frame size to render")
    ap.add_argument("--pty", action="store_true", help="also run the screen for real in a pseudo-terminal")
    ap.add_argument("--expect", action="append", default=None, help="--pty: text that must appear (in order)")
    ap.add_argument("--key", action="append", default=[], help="--pty: keys to send after the matching --expect, comma-separated")
    ap.add_argument("--second", action="append", default=[], help="--pty: a command for a second terminal; it must succeed")
    ap.add_argument("--timeout", type=float, default=900, help="seconds per step (a cold build takes minutes)")
    ap.add_argument("--keep", action="store_true", help="keep the temporary home")
    a = ap.parse_args()

    fetch = f"curl -fsSL {a.script}" if re.match(r"https?://", a.script) else f"cat '{a.script}'"
    ref = None if a.pinned else (a.ref or subprocess.check_output(["git", "-C", ROOT, "rev-parse", "HEAD"], text=True).strip())
    tmp = tempfile.mkdtemp(prefix="thc-try-")
    home = os.path.join(tmp, "home")
    os.makedirs(home)
    try_home = os.path.join(home, ".thc-try")
    real = pwd.getpwuid(os.getuid()).pw_dir

    # A reader's environment: an empty home, none of your thc settings, Rust still installed.
    env = {k: v for k, v in os.environ.items() if not k.startswith("THC_")}
    env.update(HOME=home, TERM="xterm-256color")
    env.setdefault("CARGO_HOME", os.path.join(real, ".cargo"))
    env.setdefault("RUSTUP_HOME", os.path.join(real, ".rustup"))
    env.setdefault("CARGO_TARGET_DIR", os.path.join(ROOT, "target"))
    if not a.github:
        env["THC_TRY_REPO"] = "file://" + ROOT
    places = real_places()
    before = {name: tree(p) for name, p in places.items()}
    ui_dir = os.path.join(tempfile.gettempdir(), "thc-ui")
    ui_before = set(os.listdir(ui_dir)) if os.path.isdir(ui_dir) else set()

    ok = True

    def check(cond, what, detail=""):
        nonlocal ok
        ok = ok and bool(cond)
        print(f"{'✓' if cond else '✗'} {what}{(': ' + detail) if detail else ''}", flush=True)
        return cond

    line = f"{fetch} | sh" + (f" -s -- {ref}" if ref else "")
    print(f"$ {line}   (home: {home}{', from this checkout' if not a.github else ''})", flush=True)
    r = subprocess.run(["sh", "-c", line], env={**env, "THC_TRY_NO_RUN": "1"}, timeout=a.timeout, stdin=subprocess.DEVNULL)
    if not check(r.returncode == 0, "the one-liner set up a sandbox", f"exit {r.returncode}"):
        return finish(False, tmp, a.keep)
    wrapper = os.path.join(try_home, "bin/thc")
    check(os.access(wrapper, os.X_OK), "~/.thc-try/bin/thc is there")

    # The second terminal: a person's shell (its own HOME and thc settings) with the wrapper on PATH.
    second = {k: v for k, v in os.environ.items() if not k.startswith("THC_")}
    second.update(PATH=os.path.join(try_home, "bin") + ":" + second.get("PATH", ""), THC_ACTOR="claude")

    def thc(*args, **kw):
        return subprocess.run([wrapper, *args], env={**second, **kw.pop("env", {})}, capture_output=True, text=True, timeout=120, **kw)

    sandbox = thc("vault", "info", "--json")
    path = (json.loads(sandbox.stdout or "{}").get("path") or "") if sandbox.returncode == 0 else ""
    check(path.startswith(try_home), "the wrapper's vault is the sandbox's", path or sandbox.stderr.strip()[:200])
    added = thc("--json", "todo", NOTE, "--due", "today", "--key", "try-test-note")
    check(added.returncode == 0, "a second terminal added a task", added.stderr.strip()[:200])
    task = thc("--json", "todo", "Sort the seed packets", "--key", "try-test-task")
    tid = json.loads(task.stdout)["nodes"][0]["id"] if task.returncode == 0 else ""
    done = thc("--json", "done", tid) if tid else task
    check(done.returncode == 0, "and completed a task", tid[:5] or done.stderr.strip()[:200])
    refused = thc("daemon", "start")
    check(refused.returncode != 0 and "switched off" in refused.stderr, "the wrapper refuses `thc daemon`")

    has_screen = thc("screen", "--help")
    if has_screen.returncode == 0 and "--render" in has_screen.stdout:
        how, frame = "thc screen --render", thc("screen", "--render", a.render, env={"THC_ACTOR": ""})
    else:
        how, frame = "THC_TUI_SNAPSHOT thc tui", thc("tui", env={"THC_TUI_SNAPSHOT": a.render, "THC_ACTOR": ""})
    shown = frame.stdout
    check(frame.returncode == 0 and any(s in shown for s in SEEDED), f"a {a.render} frame ({how}) shows the made-up notes", frame.stderr.strip()[:200])
    check(NOTE[:20] in shown, "and the second terminal's task")
    print(shown, flush=True)

    if a.pty:
        ok = run_pty(a, line, env, try_home, second) and ok

    # Nothing outside the sandbox changed.
    for name, p in places.items():
        diff = changed(before[name], tree(p))
        check(not diff, f"{name} untouched", ", ".join(diff[:5]))
    ui_after = set(os.listdir(ui_dir)) if os.path.isdir(ui_dir) else set()
    leaked = []
    for n in ui_after - ui_before:
        try:
            if tmp in open(os.path.join(ui_dir, n), errors="replace").read():
                leaked.append(n)
        except OSError:
            pass
    check(not leaked, f"no sandbox screen advertised in your {ui_dir}", ", ".join(leaked))
    envsh = open(next(os.path.join(try_home, d, "env.sh") for d in os.listdir(try_home) if d.endswith(".sandbox"))).read()
    sock = re.search(r"THC_SOCKET='([^']*)'", envsh)
    check(sock and not sock.group(1).startswith(f"/tmp/thc-{os.getuid()}/"), "the sandbox's daemon socket is its own", sock.group(1) if sock else "missing")
    return finish(ok, tmp, a.keep)


def run_pty(a, line, env, try_home, second) -> bool:
    expects = a.expect or ["Today"]
    pid, fd = pty.fork()
    if pid == 0:
        os.environ.clear()
        os.environ.update(env)
        os.execvp("sh", ["sh", "-c", line])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 130, 0, 0))
    seen, ok = b"", True
    try:
        for i, want in enumerate(expects):
            end = time.time() + a.timeout
            while not drew(want, seen) and time.time() < end:
                r, _, _ = select.select([fd], [], [], 0.2)
                if r:
                    try:
                        chunk = os.read(fd, 65536)
                    except OSError:
                        break
                    if not chunk:
                        break
                    seen += chunk
                    if b"\x1b[6n" in chunk:
                        os.write(fd, b"\x1b[1;1R")
            if not drew(want, seen):
                print(f"✗ the screen never drew {want!r}. Last output:\n{visible(seen)[-1500:]}")
                return False
            print(f"✓ the screen drew {want!r}", flush=True)
            seen = b""
            if i < len(a.key):
                time.sleep(1.2)  # let the screen settle: keys sent during its startup probe are lost
                for k in a.key[i].split(","):
                    os.write(fd, KEYS.get(k, k.encode()))
                    time.sleep(0.3)
        # keep reading the screen's output, as a terminal would, while the second terminal works
        stop = threading.Event()

        def drain():
            while not stop.is_set():
                if select.select([fd], [], [], 0.2)[0]:
                    try:
                        chunk = os.read(fd, 65536)
                    except OSError:
                        return
                    if b"\x1b[6n" in chunk:
                        os.write(fd, b"\x1b[1;1R")

        threading.Thread(target=drain, daemon=True).start()
        live = f'thc todo "{LIVE}" --due today --key try-test-live'
        for cmd in ["thc ui ls --json | grep -q pid", live, *a.second]:
            r = subprocess.run(["sh", "-c", cmd], env=second, capture_output=True, text=True, timeout=60)
            ok = ok and r.returncode == 0
            print(f"{'✓' if r.returncode == 0 else '✗'} second terminal: {cmd} -> {(r.stdout or r.stderr).strip()[:160]!r}", flush=True)
        # The running screen shows the second terminal's task: read it back through `thc ui render`.
        frame, end = "", time.time() + 20
        while LIVE not in frame and time.time() < end:
            r = subprocess.run(["sh", "-c", f"thc ui render {a.render}"], env=second, capture_output=True, text=True, timeout=60)
            frame = r.stdout
            time.sleep(0.5)
        ok = ok and LIVE in frame
        print(f"{'✓' if LIVE in frame else '✗'} the running screen shows the second terminal's task (thc ui render)", flush=True)
        print(frame, flush=True)
        stop.set()
        os.write(fd, KEYS["ctrl-q"])
        time.sleep(1)
    finally:
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        os.system(f"pkill -f '{try_home}/' >/dev/null 2>&1")
    return ok


def finish(ok, tmp, keep):
    if keep or not ok:
        print(f"(home: {tmp}; delete it when done)")
    else:
        shutil.rmtree(tmp, ignore_errors=True)
    print("try-test: " + ("passed" if ok else "FAILED"))
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
