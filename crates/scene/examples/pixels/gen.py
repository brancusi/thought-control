# Writes pixels.json: thc's Today on pixel cards (rounded, shadowed, gradient), with the brand
# mark as an image, larger text where the terminal has it, and the same data as an anti-aliased
# plot beside its braille version. Without terminal graphics it falls back to cells.
import copy, importlib.util, json, os
D = os.path.dirname(os.path.abspath(__file__))
REL = "crates/scene/examples/pixels"
spec = importlib.util.spec_from_file_location("thcgen", os.path.join(D, "..", "thc", "gen.py"))
thc = importlib.util.module_from_spec(spec); spec.loader.exec_module(thc)

def text(t, size=None, **kw):
    n = {"type": "text", "text": t, **kw}
    if size is not None: n["size"] = size
    return n
def card(child, title=None, size=None, glow=None, **kw):
    n = {**child, "backdrop": {"fill": "raised", "fill2": "surface", "border": "rim", "radius": 0.55, "shadow": 1.1,
                               **({"glow": glow} if glow else {})}}
    if title: n["title"] = title
    if size is not None: n["size"] = size
    n.update(kw); return n
col = lambda *c, **kw: {"type": "col", "children": list(c), **kw}
row = lambda *c, **kw: {"type": "row", "children": list(c), **kw}

today = copy.deepcopy(thc.today); today.pop("size", None)
detail = copy.deepcopy(thc.detail)
DONE = [2, 3, 1, 4, 6, 3, 5, 7, 4, 6, 8, 5, 9, 7]
ADDED = [4, 2, 5, 3, 4, 6, 3, 5, 6, 4, 5, 7, 5, 6]
series = [{"name": "done", "bind": "week.done", "color": "accent"}, {"name": "added", "bind": "week.added", "color": "agent"}]

def kpi(n, label, tone):
    return card(col(text(f"<{tone}+b>{{{n}}}</>", 3, bind="kpis", scale=3), text(f"<muted>{label}</>", 1)), size="1*")

ui = {
    "theme": {**thc.EMBER_DARK, "bg": "#1b1916", "rim": "#4a433b", "surface": "#221f1b", "raised": "#2e2a25"},
    "status_bar": False,
    "root": col(
        row({"type": "image", "src": f"{REL}/mark.png", "size": 14},
            col(text("<text+b>Thought Control</>", 2, scale=2), text("<muted>Today · {date} · vault </><accent>{vault}</>", 1, bind="feed"), size="1*"),
            text("\t<muted>pixels: kitty graphics · text: </><text>{text}</> ", bind="caps"),
            size=4),
        text("", size=1),
        row(card(today, "Today", size="58%"),
            text("", size=2),
            col(card(detail, "Detail", size="55%", glow="#3a281e"),
                row(card({"type": "plot", "series": series, "width": 0.14}, "Done vs added · pixels", size="1*"),
                    text("", size=2),
                    card({"type": "chart", "series": series, "labels": ["-14d", "today"]}, "Same data · cells", size="1*")),
                size="1*"),
            size="1*"),
        text("", size=1),
        row(kpi("open", "open tasks", "text"), text("", size=2), kpi("today", "due or scheduled today", "today"),
            text("", size=2), kpi("review", "agent changes to review", "agent"), text("", size=2), kpi("done", "done today", "done"),
            size=6),
    ),
    "data": {
        "feed": {"cmd": ["python3", "crates/scene/examples/thc/thcfeed.py"], "every": 3},
        "week": {"value": {"done": DONE, "added": ADDED}},
        "kpis": {"shell": "python3 -c \"import json,subprocess as s; q=lambda x: json.loads(s.run(['thc','q',x,'--json'],capture_output=True,text=True).stdout)['count']; "
                          "r=json.loads(s.run(['thc','review','--json'],capture_output=True,text=True).stdout)['pending']; "
                          "print(json.dumps({'open': q('status:open'), 'today': q('(sched<=today or due<=today) status:open'), 'review': r, 'done': q('done>=today')}))\"",
                 "every": 5},
        "caps": {"value": {"text": "2× and 3× (OSC 66)"}},
    },
}
json.dump(ui, open(os.path.join(D, "pixels.json"), "w"), indent=1)
print("wrote pixels.json")
