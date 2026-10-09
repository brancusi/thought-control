# Parse an ANSI frame (SGR truecolor) into a grid of cells: (char, fg, bg, attrs).
import re, sys, json
SGR = re.compile(r'\x1b\[([0-9;]*)m')
def parse(text, w, h):
    grid = [[(" ", None, None, "") for _ in range(w)] for _ in range(h)]
    fg = bg = None; attrs = set(); y = 0
    for line in text.split("\n")[:h]:
        x = 0; i = 0
        while i < len(line):
            m = SGR.match(line, i)
            if m:
                ps = [int(p) if p else 0 for p in m.group(1).split(";")] if m.group(1) else [0]
                k = 0
                while k < len(ps):
                    p = ps[k]
                    if p == 0: fg = bg = None; attrs = set()
                    elif p in (1, 2, 3, 4, 7, 9): attrs.add("bdiu.r.s"[p - 1] if p < 8 else {7: "r", 9: "s"}[p])
                    elif p == 22: attrs -= {"b", "d"}
                    elif p == 39: fg = None
                    elif p == 49: bg = None
                    elif p in (38, 48) and k + 4 < len(ps) and ps[k + 1] == 2:
                        c = "#%02x%02x%02x" % tuple(ps[k + 2:k + 5])
                        if p == 38: fg = c
                        else: bg = c
                        k += 4
                    elif 30 <= p <= 37 or 90 <= p <= 97: fg = f"ansi{p}"
                    k += 1
                i = m.end(); continue
            ch = line[i]
            if x < w: grid[y][x] = (ch, fg, bg, "".join(sorted(attrs)))
            x += 1; i += 1
        y += 1
    return grid
if __name__ == "__main__":
    a = parse(open(sys.argv[1]).read(), int(sys.argv[2]), int(sys.argv[3]))
    for y in [0, 1, 2, 3]:
        runs, last = [], None
        for x, c in enumerate(a[y]):
            key = (c[1], c[2], c[3])
            if key != last: runs.append((x, key)); last = key
        print(y, runs[:14])
