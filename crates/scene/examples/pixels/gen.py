# The pixel edition of thc's Today: build() returns the UI (the tour varies it: theme, layout,
# card style, the mark, the feed's vault); run alone it writes pixels.json for your own vault.
import copy, importlib.util, json, os
D = os.path.dirname(os.path.abspath(__file__))
REL = "crates/scene/examples/pixels"
spec = importlib.util.spec_from_file_location("thcgen", os.path.join(D, "..", "thc", "gen.py"))
thc = importlib.util.module_from_spec(spec); spec.loader.exec_module(thc)

DARK = {**thc.EMBER_DARK, "bg": "#1b1916", "rim": "#4a433b", "surface": "#221f1b", "raised": "#2e2a25", "shadow": "#000000"}
LIGHT = {"bg": "#f7f3ec", "surface": "#efe9de", "raised": "#fffdf8", "selection": "#ede1cf", "line": "#ddd4c6", "border": "#ddd4c6",
         "text": "#29241f", "muted": "#655c51", "dim": "#655c51", "accent": "#a8461a", "overdue": "#b3262f", "today": "#7f5b00",
         "done": "#3a7036", "doing": "#1c6c77", "agent": "#2d5fa3", "tag": "#77583a", "rim": "#d8cfc0", "hover": "#efe9de",
         "overdue-tint": "#f6e1dc", "shadow": "#6b5a44"}

def text(t, size=None, **kw):
    n = {"type": "text", "text": t, **kw}
    if size is not None: n["size"] = size
    return n
col = lambda *c, **kw: {"type": "col", "children": list(c), **kw}
row = lambda *c, **kw: {"type": "row", "children": list(c), **kw}
gap = lambda n=2: text("", size=n)

# A progress ring as SVG: the arc's length comes from the data (`dash`, out of a circumference of
# 251.3), the label is real font text. Colours are filled in per theme.
RING = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 260 100">
  <circle cx="50" cy="50" r="40" fill="none" stroke="TRACK" stroke-width="12"/>
  <circle cx="50" cy="50" r="40" fill="none" stroke="ARC" stroke-width="12" stroke-linecap="round"
          stroke-dasharray="{dash} 251.3" transform="rotate(-90 50 50)"/>
  <text x="50" y="58" font-family="Helvetica, Arial" font-size="24" font-weight="700" fill="INK" text-anchor="middle">{pct}%</text>
  <text x="108" y="44" font-family="Helvetica, Arial" font-size="20" font-weight="700" fill="INK">today</text>
  <text x="108" y="70" font-family="Helvetica, Arial" font-size="16" fill="SUB">{done} of {total} done</text>
</svg>"""

def build(vault=None, now=None, theme="dark", radius=0.55, shadow=1.1, glow=None, mark="mark.png", swap=False, kpis_top=False, plot="live"):
    # A demo vault gets its own config too: thc's Today spans every registered vault (vault:*),
    # and the real registry would bring the person's own vaults in.
    cfg = os.path.join(os.path.dirname(vault), "config") if vault else None
    env = (["env", f"THC_VAULT={vault}", f"THC_CONFIG_DIR={cfg}"] + ([f"THC_NOW={now}"] if now else [])) if vault else []
    prefix = (f"THC_VAULT={vault} THC_CONFIG_DIR={cfg} " + (f"THC_NOW={now} " if now else "")) if vault else ""
    def card(child, title=None, size=None, id=None, glow_=None):
        n = {**child, "backdrop": {"fill": "raised", "fill2": "surface", "border": "rim", "radius": radius, "shadow": shadow,
                                   **({"glow": glow_} if glow_ else {"glow": "shadow"})}}
        if title: n["title"] = title
        if size is not None: n["size"] = size
        if id and "id" not in n: n["id"] = id
        return n
    today = copy.deepcopy(thc.today); today.pop("size", None)
    today["tip"] = {"title": "{short} · {status}", "text": "{text}\n{crumbs}"}
    detail = copy.deepcopy(thc.detail)
    series = [{"name": "done", "bind": f"{plot}.done", "color": "accent"}, {"name": "added", "bind": f"{plot}.added", "color": "agent"}]
    def kpi(n, label, tone, id):
        return card(col(text(f"<{tone}+b>{{{n}}}</>", 3, bind="kpis", scale=3), text(f"<muted>{label}</>", 1)), size="1*", id=id)
    left = card(today, "Today", size="58%", id="c-today")
    right = col(card(detail, "Detail", size="55%", id="c-detail", glow_=glow),
                row(card({"type": "plot", "series": series, "width": 0.12}, "Done vs added · pixels, live", size="1*", id="c-plot"),
                    gap(),
                    card({"type": "chart", "series": series}, "Same data · cells", size="1*", id="c-cells")),
                size="1*")
    middle = row(*( [right, gap(), left] if swap else [left, gap(), right] ), size="1*")
    pal = DARK if theme == "dark" else LIGHT
    ring = card({"type": "svg", "id": "ring", "bind": "kpis", "alt": "{pct}% of today done",
                 "svg": RING.replace("TRACK", pal["line"]).replace("ARC", pal["done"]).replace("INK", pal["text"]).replace("SUB", pal["muted"])},
                size=26, id="k-ring")
    kpirow = row(kpi("open", "open tasks", "text", "k-open"), gap(), kpi("today", "due or scheduled today", "today", "k-today"),
                 gap(), kpi("review", "agent changes to review", "agent", "k-review"), gap(), kpi("done", "done today", "done", "k-done"),
                 gap(), ring, size=6)
    header = row({"type": "image", "src": f"{REL}/{mark}", "size": 14, "id": "mark"},
                 col(text("<text+b>Thought Control</>", 2, scale=2), text("<muted>Today · {date} · vault </><accent>{vault}</>", 1, bind="feed"), size="1*"),
                 col(text("\t<muted>pixels </><text>{graphics|true=kitty graphics;*=none, cells only}</>"
                          "<muted> · cell </><text>{cell_w}</><muted>×</><text>{cell_h}</><muted>px · larger text </>"
                          "<text>{text_sizing|true=OSC 66;*=no (block digits)}</> ", 1, bind="$caps"),
                     text("\t<muted>live plot tick </><text>{tick}</> ", 1, bind=f"{plot}"),
                     text("\t<muted>{fps|?$} fps · draw {draw_ms|?$} ms</> ", 1, bind="$stats"), size="1*"),
                 size=4)
    body = [header, gap(1), kpirow, gap(1), middle] if kpis_top else [header, gap(1), middle, gap(1), kpirow]
    kq = ("python3 -c \"import json,subprocess as s; q=lambda x: json.loads(s.run(['thc','q',x,'--json'],capture_output=True,text=True).stdout)['count']; "
          "r=json.loads(s.run(['thc','review','--json'],capture_output=True,text=True).stdout)['pending']; "
          "o=q('status:open'); t=q('(sched<=today or due<=today) status:open'); d=q('done>=today'); n=max(1, t + d); "
          "print(json.dumps({'open': o, 'today': t, 'review': r, 'done': d, 'total': n, 'pct': round(100 * d / n), 'dash': round(251.3 * d / n, 1)}))\"")
    return {
        "theme": DARK if theme == "dark" else LIGHT,
        "status_bar": False,
        "root": col(*body),
        "data": {
            "feed": {"cmd": env + ["python3", "crates/scene/examples/thc/thcfeed.py"], "every": 3},
            "live": {"stream": ["python3", "-u", f"{REL}/wave.py", "15"]},
            "kpis": {"shell": prefix + kq, "every": 5},
        },
    }

if __name__ == "__main__":
    json.dump(build(), open(os.path.join(D, "pixels.json"), "w"), indent=1)
    print("wrote pixels.json")
