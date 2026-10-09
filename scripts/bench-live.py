#!/usr/bin/env python3
"""Per-key latency of a real `thc tui` on a pty, end to end: from a key's bytes written to the
terminal to the last byte of the frame that answers it. Driven by scripts/bench-live.sh (which
makes the scratch vault and, for the live case, starts its daemon).

    bench-live.py THC --page TITLE [--keys N] [--pace MS] [--budget MS] [--label L]

A frame ends at the last byte before a quiet gap (GAP_MS). Each key's latency is the end of the
first frame after it; frames nobody asked for (a save's result, a daemon push) are counted on
their own, with the time they kept the terminal busy. Exits 1 when a key's p99 is over budget.
"""
import argparse, os, pty, select, signal, struct, sys, termios, fcntl, time

GAP_MS = 3.0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("thc")
    ap.add_argument("--page", required=True)
    ap.add_argument("--keys", type=int, default=400)
    ap.add_argument("--pace", type=float, default=70.0, help="ms between keys (a fast typist: 60-90)")
    ap.add_argument("--budget", type=float, default=4.0)
    ap.add_argument("--label", default="")
    ap.add_argument("--cols", type=int, default=140)
    ap.add_argument("--rows", type=int, default=40)
    a = ap.parse_args()

    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.execv(a.thc, [a.thc, "p", a.page])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", a.rows, a.cols, 0, 0))

    def read_for(sec):
        """Read whatever comes for `sec` seconds; [(t, nbytes)]."""
        out, end = [], time.perf_counter() + sec
        while True:
            left = end - time.perf_counter()
            if left <= 0:
                return out
            r, _, _ = select.select([fd], [], [], left)
            if r:
                try:
                    b = os.read(fd, 65536)
                except OSError:
                    return out
                if not b:
                    return out
                out.append((time.perf_counter(), len(b)))

    # Settle: the first frame, the terminal's replies (thc's queries go unanswered: plain xterm).
    read_for(2.5)
    os.write(fd, b"\x1b[1;5H")  # ⌃Home: type at the top (pages open at their end)
    read_for(0.5)

    text = ("the quick brown fox jumps over the lazy dog while the words keep flowing " * 40)[: a.keys]
    lat, unasked, busy = [], 0, []
    for ch in text:
        t0 = time.perf_counter()
        os.write(fd, ch.encode())
        events = read_for(a.pace / 1000.0)
        # Frames: runs of reads with gaps under GAP_MS.
        frames, cur = [], None
        for t, n in events:
            if cur and (t - cur[1]) * 1000 <= GAP_MS:
                cur[1] = t
            else:
                cur = [t, t]
                frames.append(cur)
        if frames:
            lat.append((frames[0][1] - t0) * 1000)
            for f in frames[1:]:
                unasked += 1
                busy.append((f[1] - f[0]) * 1000)
        else:
            lat.append(float("inf"))

    os.write(fd, b"\x11")  # ⌃Q
    read_for(1.5)
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    os.waitpid(pid, 0)

    s = sorted(lat)
    p = lambda q: s[min(len(s) - 1, int(round((len(s) - 1) * q)))]
    slow = sum(1 for x in lat if x > a.budget)
    print(f"{a.label:<34} {len(lat)} keys · p50 {p(0.5):.2f} ms · p99 {p(0.99):.2f} ms · max {s[-1]:.2f} ms · over {a.budget:g} ms: {slow}"
          + (f" · other frames {unasked} (longest {max(busy):.2f} ms)" if busy else ""))
    worst = sorted(range(len(lat)), key=lambda i: -lat[i])[:5]
    if p(0.99) > a.budget:
        print("   slowest keys (index: ms): " + ", ".join(f"{i}: {lat[i]:.1f}" for i in worst))
        sys.exit(1)


if __name__ == "__main__":
    main()
