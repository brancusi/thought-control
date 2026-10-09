# An "analyst" agent walking someone through the dashboard: it switches tabs, points at things
# with callouts, rings and spotlights (bound to live numbers), and moves the mouse to show tips.
#   tour.py [seconds per step]
import json, os, socket, sys, tempfile, time
STEP = float(sys.argv[1]) if len(sys.argv) > 1 else 5
path = os.environ.get("THC_SCENE_SOCKET") or os.path.join(tempfile.gettempdir(), "thc-scene.sock")
s = socket.socket(socket.AF_UNIX); s.connect(path); f = s.makefile("rw")
def send(req):
    f.write(json.dumps(req) + "\n"); f.flush(); return json.loads(f.readline())
def layers(*ls): send({"op": "layers", "layers": list(ls)})
def tab(i): send({"op": "click", "id": "main", "tab": i})
def screen(): return send({"op": "screen"})["screen"].split("\n")
def find(word):
    for y, line in enumerate(screen()):
        x = line.find(word)
        if x >= 0: return x, y
def say(m): print(time.strftime("%H:%M:%S"), m, flush=True)
GOLD = {"accent": "gold", "fill": "#2a2110", "fg": "#f0d9a8", "ring": "#5a4410", "edge": "double"}
RED = {"accent": "neg", "fill": "#2b1214", "fg": "#ffd7d5", "ring": "#5c1a1d", "edge": "thick"}
BLUE = {"accent": "accent", "fill": "#0f1d33", "fg": "#cfe3ff", "ring": "#1b3a63", "edge": "rounded"}
GREEN = {"accent": "pos", "fill": "#0f2416", "fg": "#c8f5d3", "ring": "#174d27", "edge": "plain"}
A = "analyst"

say("1 · overview: revenue, spotlit"); tab(0); layers()
time.sleep(1)
layers({"on": "k-rev", "title": "Revenue {rev} · +12.4% YoY", "text": "Best quarter on record. EBITDA {ebitda}, net {net}.",
        "bind": "fin.kpi", "arrow": True, "pulse": 900, "spotlight": True, "place": ["below"], "width": 46, "look": GOLD, "by": A})
time.sleep(STEP)

say("2 · margin and the March loss, two callouts at once")
layers({"on": "k-gm", "title": "Margin down 4.1 pts", "text": "Cloud costs +31% QoQ. The new contract lands in November.",
        "arrow": True, "ring": True, "place": ["below"], "width": 40, "look": RED, "by": A},
       {"on": "months", "row": 2, "title": "March: −$1.9M one-off", "text": "Restructuring. Excluding it, March net is in line.",
        "arrow": True, "pulse": 700, "place": ["left", "below"], "width": 38, "look": RED, "by": A})
time.sleep(STEP)

say("3 · the book: worst mover, live")
st = send({"op": "state"})
rows = st["data"]["fin"]["value"]["positions"]; worst = st["data"]["fin"]["value"]["worst"]["sym"]
i = next(k for k, r in enumerate(rows) if r["sym"] == worst)
layers({"on": "pos", "row": i, "title": "{sym} {±day_s} today", "text": "Day P&L {±dpnl_s} · weight {w}\nStop at {stop}: 8% under last.",
        "bind": "fin.worst", "arrow": True, "pulse": 600, "place": ["right", "below"], "width": 34, "look": RED, "by": A})
send({"op": "click", "id": "pos", "row": i})
time.sleep(STEP)

say("4 · hovering rows: tips follow the mouse"); layers()
at = find("sym ")
if at:
    x, y = at
    for k in range(1, 9):
        send({"op": "mouse", "kind": "move", "x": x + 2, "y": y + k}); time.sleep(0.45)
    for k in range(8, 0, -1):
        send({"op": "mouse", "kind": "move", "x": x + 2, "y": y + k}); time.sleep(0.2)
send({"op": "mouse", "kind": "move", "x": 0, "y": 0})

say("5 · four looks at once, one per KPI")
layers({"on": "k-rev", "text": "double · gold", "arrow": True, "ring": True, "place": ["below"], "look": GOLD, "by": A},
       {"on": "k-gm", "text": "thick · red", "arrow": True, "ring": True, "place": ["below"], "look": RED, "by": A},
       {"on": "k-cash", "text": "rounded · blue", "arrow": True, "pulse": 500, "place": ["below"], "look": BLUE, "by": A},
       {"on": "k-day", "text": "plain · green", "arrow": True, "pulse": 1300, "place": ["below"], "look": GREEN, "by": A})
time.sleep(STEP)

say("6 · P&L tab: EBITDA miss, spotlit"); layers(); tab(1); time.sleep(0.8)
layers({"on": "stmt", "row": 6, "title": "EBITDA 10.7% under budget", "text": "R&D over by 7.9%: two senior hires landed early. Net income follows.",
        "arrow": True, "ring": True, "spotlight": True, "place": ["below", "right"], "width": 44, "look": BLUE, "by": A},
       {"on": "stmt", "row": 10, "ring": True, "pulse": 800, "look": RED})
time.sleep(STEP)

say("7 · cash bridge: walking the bars with the mouse"); layers()
at = find("Open      Ops")
if at:
    x, y = at
    for k in range(7):
        send({"op": "mouse", "kind": "move", "x": x + 3 + k * 10, "y": y - 2}); time.sleep(0.6)
send({"op": "mouse", "kind": "move", "x": 0, "y": 0})

say("8 · risk & world: Tokyo"); tab(3); time.sleep(0.8)
at = find("●Tokyo")
if at:
    send({"op": "mouse", "kind": "move", "x": at[0], "y": at[1]}); time.sleep(STEP / 2)
    at2 = find("●London") or find("●Lon")
    if at2: send({"op": "mouse", "kind": "move", "x": at2[0], "y": at2[1]}); time.sleep(STEP / 2)
send({"op": "mouse", "kind": "move", "x": 0, "y": 0})
layers({"on": "world", "title": "Tokyo is 14% of revenue", "text": "Up from 9% a year ago; FX is a 2-pt headwind.", "arrow": True,
        "ring": True, "place": ["right", "above"], "width": 36, "look": GOLD, "by": A})
time.sleep(STEP)

say("9 · done"); layers(); tab(0)
