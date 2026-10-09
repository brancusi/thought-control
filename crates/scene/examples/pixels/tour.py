# A scripted walk through the pixel layer, driven over the screen's socket like any agent would:
# a live plot streaming at 15 Hz, cards restyled while you watch (radius, shadow, glow), the theme
# flipped from ember-dark to ember-light, the brand mark swapped, the layout rearranged, agent
# callouts and a pixel spotlight, and the mouse walking the list for tips. It runs on a seeded
# sample vault with its own config (so none of your vaults show) and the clock pinned.
#   tour.py [seconds per step]
import importlib.util, json, os, socket, subprocess, sys, tempfile, time
STEP = float(sys.argv[1]) if len(sys.argv) > 1 else 5
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..", ".."))
os.chdir(ROOT)
spec = importlib.util.spec_from_file_location("pixgen", "crates/scene/examples/pixels/gen.py")
gen = importlib.util.module_from_spec(spec); spec.loader.exec_module(gen)

# The sample vault and the marks.
vault = os.path.join(tempfile.gettempdir(), "thc-pixel-demo", "v")
NOW = "2026-10-09T09:30"
if not os.path.exists(os.path.join(vault, "thc-vault.toml")):
    env = {**os.environ, "THC_VAULT": vault, "THC_NOW": NOW, "THC_DEVICE": "demo", "THC_FIXTURE_IDS": "1", "THC_ACTOR": "claude",
           "THC_CONFIG_DIR": os.path.join(os.path.dirname(vault), "config")}
    for k in ("THC_TEST", "THC_TEST_ALLOW"): env.pop(k, None)
    subprocess.run(["scripts/seed-sample.sh", vault], env=env, check=True, capture_output=True)
for name, rgb in (("mark.png", None), ("mark-agent.png", "136,177,236")):
    if not os.path.exists(f"crates/scene/examples/pixels/{name}"):
        subprocess.run(["python3", "crates/scene/examples/pixels/mark.py", f"crates/scene/examples/pixels/{name}"] + ([rgb] if rgb else []), check=True)

path = os.environ.get("THC_SCENE_SOCKET") or os.path.join(tempfile.gettempdir(), "thc-scene.sock")
s = socket.socket(socket.AF_UNIX); s.connect(path); f = s.makefile("rw")
def send(req):
    f.write(json.dumps(req) + "\n"); f.flush(); return json.loads(f.readline())
def ui(**kw): return gen.build(vault=vault, now=NOW, **kw)
def push(**kw): send({"op": "push", "ui": ui(**kw)})
def layers(*ls): send({"op": "layers", "layers": list(ls)})
def stats():
    st = send({"op": "stats"}); return f"{st['fps']:5.1f} fps · draw {st['draw_ms']:.2f} ms (max {st['max_ms']:.1f}) · {st['msgs']:4.0f} msg/s"
def say(m): print(time.strftime("%H:%M:%S"), m, flush=True)
def find(word):
    for y, line in enumerate(send({"op": "screen"})["screen"].split("\n")):
        x = line.find(word)
        if x >= 0: return x, y
A = "designer"

say("1 · the pixel edition: cards, the mark, a plot streaming at 15 Hz"); push(); time.sleep(STEP); say("   " + stats())

say("2 · restyling cards live: corner radius, then shadow depth")
for r in [0.55, 0.3, 0.1, 0.0, 0.4, 0.9, 1.4, 1.0, 0.55]:
    push(radius=r); time.sleep(0.35)
for sh in [1.1, 0.5, 0.0, 0.8, 1.6, 2.2, 1.1]:
    push(shadow=sh); time.sleep(0.35)
say("   " + stats())

say("3 · the detail card glows: an agent is working on it")
for k in range(16):
    level = abs((k % 8) - 4) / 4
    g = "#%02x%02x%02x" % (int(0x3a + (0xe8 - 0x3a) * (1 - level) * 0.6), int(0x28 + (0x83 - 0x28) * (1 - level) * 0.6), int(0x1e + (0x4f - 0x1e) * (1 - level) * 0.6))
    push(glow=g); time.sleep(0.18)
say("   " + stats())

say("4 · a designer's callouts, with a pixel spotlight")
layers({"on": "c-plot", "title": "This one is pixels", "text": "Anti-aliased lines and a gradient fill, redrawn 15 times a second.",
        "arrow": True, "spotlight": True, "place": ["above", "left"], "width": 40, "by": A,
        "look": {"accent": "accent", "fill": "#2a1d15", "fg": "#f3d6c4", "edge": "double"}},
       {"on": "c-cells", "title": "…and this one is cells", "text": "The same numbers in braille: 2×4 dots a cell.", "arrow": True,
        "place": ["above", "right"], "width": 34, "by": A, "look": {"accent": "agent", "fill": "#16202e", "fg": "#d5e3f7"}})
time.sleep(STEP); layers()

say("5 · the list: hovering rows shows tips; the selection paints over the card")
at = find("Today  ")
if at:
    x, y = at
    for k in [1, 2, 3, 6, 7, 8, 9, 10]:
        send({"op": "mouse", "kind": "move", "x": x + 6, "y": y + k}); time.sleep(0.5)
    send({"op": "mouse", "kind": "move", "x": 0, "y": 0})
send({"op": "click", "id": "today", "row": 7}); time.sleep(STEP / 2)

say("6 · ember-light: the whole theme, cards included, flipped in one push"); push(theme="light"); time.sleep(STEP); say("   " + stats())

say("7 · the agent's mark, and the layout rearranged"); push(theme="light", mark="mark-agent.png", swap=True); time.sleep(STEP)
push(theme="light", mark="mark-agent.png", swap=True, kpis_top=True); time.sleep(STEP)

say("8 · back to dark"); push(); time.sleep(STEP / 2)

say("9 · an agent draws: SVG pushed into the running screen, rendered at the real pixel size")
ART = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 260 100">
  <defs><linearGradient id="g" x1="0" x2="1"><stop offset="0" stop-color="#e8834f"/><stop offset="1" stop-color="#88b1ec"/></linearGradient></defs>
  <rect x="4" y="8" width="252" height="84" rx="18" fill="none" stroke="url(#g)" stroke-width="3"/>
  <path d="M 24 70 C 60 {a}, 90 {b}, 130 50 S 200 {c}, 236 30" fill="none" stroke="url(#g)" stroke-width="5" stroke-linecap="round"/>
  <text x="130" y="88" font-family="Helvetica, Arial" font-size="13" fill="#a0968a" text-anchor="middle">drawn by an agent · frame {k}</text>
</svg>"""
import math
def node(n, id):
    if n.get("id") == id: return n
    for c in n.get("children", []):
        hit = node(c, id)
        if hit: return hit
ring = node(ui()["root"], "ring")  # keep its card: a patch replaces the whole node
for k in range(24):
    a, b, c = 20 + 30 * math.sin(k / 3), 80 - 30 * math.sin(k / 4), 90 - 40 * math.cos(k / 3)
    art = ART.replace("{a}", f"{a:.0f}").replace("{b}", f"{b:.0f}").replace("{c}", f"{c:.0f}").replace("{k}", str(k))
    send({"op": "patch", "id": "ring", "node": {**ring, "svg": art, "bind": None}})
    time.sleep(0.15)
time.sleep(STEP / 2)
say("   " + stats())
push(); time.sleep(1); say("10 · done")
