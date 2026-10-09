# Drives a running screen over its socket: `storm.py patch 1000` or `storm.py push 60` (per second, for N seconds).
import json, os, random, socket, sys, tempfile, time
mode, rate, secs = sys.argv[1], float(sys.argv[2]), float(sys.argv[3]) if len(sys.argv) > 3 else 10
path = os.environ.get("THC_SCENE_SOCKET") or os.path.join(tempfile.gettempdir(), "thc-scene.sock")
s = socket.socket(socket.AF_UNIX); s.connect(path); f = s.makefile("rw")
def send(req):
    f.write(json.dumps(req) + "\n"); f.flush(); return json.loads(f.readline())
colors = ["red", "green", "yellow", "blue", "magenta", "cyan", "white"]
def cell(i, n):
    return {"type": "text", "id": f"c{i}", "border": True, "title": f"cell {i}",
            "style": {"fg": colors[(n + i) % len(colors)], "bold": n % 2 == 0},
            "text": f"{n:>8}\n{'▮' * (n % 12)}"}
def grid(n):
    return {"type": "col", "children": [{"type": "row", "children": [cell(r * 6 + c, n) for c in range(6)]} for r in range(4)]}
def rnd(depth):
    if depth == 0 or random.random() < 0.25:
        return {"type": "text", "border": True, "title": random.choice(["α","β","γ","δ","ε"]),
                "style": {"fg": random.choice(colors)}, "text": "█" * random.randint(1, 40), "size": random.choice(["*", "2*", 3, "20%"])}
    return {"type": random.choice(["row", "col"]), "size": random.choice(["*", "2*"]), "children": [rnd(depth - 1) for _ in range(random.randint(2, 4))]}
if mode == "patch":
    send({"op": "push", "ui": {"root": {"type": "col", "children": [
        {"type": "text", "size": 1, "style": {"bold": True, "fg": "yellow"}, "text": f" patch storm · {rate:.0f} patches/s over one socket · 24 cells"},
        {**grid(0), "id": "grid"}]}}})
t0, n, ok = time.time(), 0, 0
while time.time() - t0 < secs:
    n += 1
    if mode == "patch":
        i = n % 24
        r = send({"op": "patch", "id": f"c{i}", "node": cell(i, n)})
    else:
        r = send({"op": "push", "ui": {"root": {"type": "col", "children": [
            {"type": "text", "size": 1, "style": {"bold": True, "fg": "yellow"}, "text": f" push storm · whole random UI #{n} at {rate:.0f}/s"},
            rnd(4)]}}})
    ok += bool(r.get("ok"))
    d = t0 + n / rate - time.time()
    if d > 0: time.sleep(d)
el = time.time() - t0
print(json.dumps({"mode": mode, "sent": n, "ok": ok, "per_sec": round(n / el, 1)}))
