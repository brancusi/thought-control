# Emits JSON lines forever: `feed.py ticker 500` (lines/s). Each line is a whole value.
import json, math, random, sys, time
kind, rate = sys.argv[1], float(sys.argv[2])
gap, t0, n = 1.0 / rate, time.time(), 0
out = sys.stdout
syms = ["ACME","GLOBX","INITECH","UMBRL","HOOLI","PIEDP","STARK","WAYNE","TYRL","CYBD","MSSY","OSCP",
        "ZORG","NAKA","SOYL","VANDL","GRING","WONKA","OCPC","ABST","SLRS","BRAWN","DUFF","KRUST",
        "MOMC","NUKA","VAULT","APRT","BLKM","CHZ","DNKY","EVIL","FROB","GLRP","HRKN","IRON","JNKY","KAZM"]
px = {s: random.uniform(10, 500) for s in syms}
hist = [100.0] * 240
W, H = 160, 50
ramp = " .:-=+*#%@"
while True:
    n += 1; t = time.time() - t0
    if kind == "ticker":
        for s in random.sample(syms, 8):
            px[s] *= 1 + random.gauss(0, 0.004)
        hist.append(hist[-1] * (1 + random.gauss(0, 0.003))); hist.pop(0)
        rows = [{"sym": s, "px": f"{p:9.2f}", "chg": f"{(p/ (p/(1+random.gauss(0,0.01))) - 1)*100:+.2f}%",
                 "bar": "█" * int(min(30, p / 17))} for s, p in px.items()]
        line = {"seq": n, "rows": rows, "hist": hist, "last": f"{hist[-1]:.3f}"}
    elif kind == "plasma":
        lines = []
        for y in range(H):
            row = []
            for x in range(W):
                v = math.sin(x / 9 + t * 2.1) + math.sin(y / 5 + t * 1.3) + math.sin((x + y) / 13 + t) + math.sin(math.hypot(x - W/2, y - H/2) / 6 - t * 3)
                row.append(ramp[int((v + 4) / 8 * (len(ramp) - 1))])
            lines.append("".join(row))
        line = {"seq": n, "art": "\n".join(lines)}
    elif kind == "biglist":
        line = {"seq": n, "items": [{"i": i, "v": f"{math.sin(i / 50 + t) * 1000:9.2f}"} for i in range(50000)]}
    out.write(json.dumps(line) + "\n"); out.flush()
    nxt = t0 + n * gap
    d = nxt - time.time()
    if d > 0: time.sleep(d)
