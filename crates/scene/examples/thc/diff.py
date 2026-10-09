# Cell-by-cell comparison of two ANSI frames: character, background, and (on non-blank cells)
# foreground and attributes. Prints the score and the rows that differ.
import sys; sys.path.insert(0, sys.argv[1]); from ansigrid import parse
w, h = int(sys.argv[4]), int(sys.argv[5])
a = parse(open(sys.argv[2]).read(), w, h); b = parse(open(sys.argv[3]).read(), w, h)
def norm(c):
    ch, fg, bg, at = c
    return (ch, bg) if ch == " " else (ch, fg, bg, at)
diff = [(y, x) for y in range(h) for x in range(w) if norm(a[y][x]) != norm(b[y][x])]
chars = sum(1 for y, x in diff if a[y][x][0] != b[y][x][0])
print(f"cells {w*h}  identical {w*h-len(diff)} ({100*(w*h-len(diff))/(w*h):.2f}%)  differ {len(diff)} (text {chars}, style only {len(diff)-chars})")
rows = sorted({y for y, _ in diff})
for y in rows[:12]:
    xs = [x for yy, x in diff if yy == y]
    x = xs[0]
    print(f"  row {y:2} cols {xs[0]}..{xs[-1]} ({len(xs)}): thc {a[y][x]}  scene {b[y][x]}")
    print("     thc  |" + "".join(c[0] for c in a[y][max(0,x-8):x+40]) + "|")
    print("     scene|" + "".join(c[0] for c in b[y][max(0,x-8):x+40]) + "|")
