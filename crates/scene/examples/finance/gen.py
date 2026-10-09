# Writes dashboard.json: a live finance dashboard on one stream (`fin`).
import json, os
D = os.path.dirname(os.path.abspath(__file__))
REL = "crates/scene/examples/finance"

def box(title, child, size=None, edge=None, **kw):
    n = {**child, "title": title}
    if size is not None: n["size"] = size
    if edge: n["edge"] = edge
    n.update(kw); return n
def col(*c, size=None, **kw): return {"type": "col", "children": list(c), **({"size": size} if size else {}), **kw}
def row(*c, size=None, **kw): return {"type": "row", "children": list(c), **({"size": size} if size else {}), **kw}
def text(t, bind=None, size=None, **kw):
    n = {"type": "text", "text": t}
    if bind: n["bind"] = bind
    if size is not None: n["size"] = size
    n.update(kw); return n

def kpi(id, title, big, sub, color=None):
    return box(title, col(
        {"type": "big", "bind": "fin.kpi", "text": big, "align": "center", "size": 3, **({"style": {"fg": color}} if color else {})},
        text(sub, "fin.kpi", 1, align="center", style={"fg": "dim"})), id=id, size="1*", style={"bg": "panel"})

kpis = row(
    kpi("k-rev", "Revenue · Q3", "{rev}", "{±rev_chg} YoY · EBITDA {ebitda}", "accent"),
    kpi("k-gm", "Gross margin", "{gm}", "{±gm_chg} pts vs Q2 · net {net}", "gold"),
    kpi("k-cash", "Cash", "{cash}", "runway {runway} · burn $1.1M/mo", "fg"),
    kpi("k-day", "Book · today", "{±day}", "NAV {nav} · MV {mv} · uP&L {upnl}"),
    size=6)

positions_cols = [
    {"title": "sym", "value": "{sym}", "size": 6},
    {"title": "qty", "value": "{qty_s}", "size": 7, "align": "right"},
    {"title": "last", "value": "{px_s}", "size": 9, "align": "right"},
    {"title": "mkt value", "value": "{mv_s}", "size": 10, "align": "right"},
    {"title": "day", "value": "{day_s}", "size": 8, "align": "right", "heat": {"value": "{day}", "min": -1.5, "max": 1.5}},
    {"title": "day P&L", "value": "{±dpnl_s}", "size": 9, "align": "right"},
    {"title": "total", "value": "{±ret_s}", "size": 8, "align": "right"},
    {"title": "wt", "value": "{w}", "size": 6, "align": "right"},
]
pos_tip = {"title": "{sym} · {name}", "text": "{qty_s} @ {px_s} (cost {cost_s})\nMV {mv_s} · weight {w}\nToday {±day_s} = {±dpnl_s}\nTotal {±ret_s} · stop {stop}"}
def positions(id, size=None):
    n = {"type": "table", "id": id, "bind": "fin.positions", "columns": positions_cols, "tip": pos_tip}
    return box("Positions · click a row · hover for detail", n, size)

equity = box("Equity vs benchmark · 300 ticks", {"type": "chart", "labels": ["-300", "-150", "now"], "series": [
    {"name": "book", "bind": "fin.equity", "color": "accent"},
    {"name": "S&P", "bind": "fin.bench", "color": "dim"}]}, "60%", id="equity")
monthly = box("Net income by month ($K)", {"type": "bars", "id": "months", "bind": "fin.months", "label": "{m}", "value": "net",
                                           "tip": {"title": "{m}", "text": "revenue {rev_s}\nnet {±net_s}\ngross margin {gm}"}}, id="months")

detail = box("Selected", col(
    {"type": "big", "bind": "@pos.sym", "text": "{.}", "size": 3, "style": {"fg": "accent"}} if False else
    text("{sym}  {name}", "@pos", 1, style={"bold": True, "fg": "accent"}),
    text("last {px_s}   qty {qty_s}\nmkt value {mv_s}   weight {w}\nday {±day_s}   P&L {±dpnl_s}\ntotal {±ret_s}   stop {stop}", "@pos", 5),
    {"type": "gauge", "bind": "@pos", "value": "{w}", "max": 25, "label": "weight {w} of 25% cap", "size": 1, "style": {"fg": "gold"}},
), size=11, id="detail")

budget = box("Budget used · Q3", col(
    {"type": "gauge", "bind": "fin.budget", "value": "{rd}", "label": "R&D", "size": 1, "style": {"fg": "accent"}},
    text("", size=1),
    {"type": "gauge", "bind": "fin.budget", "value": "{sm}", "label": "Sales & marketing", "size": 1, "style": {"fg": "neg"}},
    text("", size=1),
    {"type": "gauge", "bind": "fin.budget", "value": "{ga}", "label": "G&A", "size": 1, "style": {"fg": "pos"}},
    text("", size=1),
    {"type": "gauge", "bind": "fin.budget", "value": "{hire}", "label": "Hiring plan", "size": 1, "style": {"fg": "gold"}},
), size=10, id="budget")

world = box("Revenue by region · hover a city", {"type": "map", "id": "map", "bind": "fin.regions", "lat": "lat", "lon": "lon", "label": "{city}",
                                                 "tip": {"title": "{city}", "text": "revenue {rev} · {share} of total"}, "style": {"fg": "gold"}}, id="world")

news = box("Wire", {"type": "list", "id": "news", "bind": "fin.news", "item": "{t}  {imp} {h}", "style": {"fg": "dim"}}, id="wire")

overview = col(
    kpis,
    row(equity, monthly, size="45%"),
    row(positions("pos"), col(detail, budget), size="1*"),
)

stmt = box("Income statement · Q3 ($K) · var vs budget", {"type": "table", "id": "stmt", "bind": "fin.stmt", "columns": [
    {"title": "line", "value": "{line}"},
    {"title": "actual", "value": "{a}", "size": 9, "align": "right"},
    {"title": "budget", "value": "{b}", "size": 9, "align": "right"},
    {"title": "var", "value": "{±v}", "size": 8, "align": "right"},
    {"title": "var %", "value": "{vp}", "size": 8, "align": "right", "heat": {"value": "{vn}", "min": -15, "max": 15}},
]}, "55%", id="stmt-box")
pnl = col(
    row(stmt, col(
        box("Cash bridge ($K)", {"type": "bars", "id": "bridge", "bind": "fin.cash_bridge", "label": "{k}", "value": "v",
                                 "tip": {"text": "{k}: {±v}K"}}),
        box("AR aging ($K)", {"type": "bars", "id": "aging", "bind": "fin.aging", "label": "{b}", "value": "v", "style": {"fg": "gold"},
                              "tip": {"text": "{b} days: ${v}K outstanding"}}),
    )),
    row(box("Revenue by month ($K)", {"type": "bars", "id": "rev", "bind": "fin.months", "label": "{m}", "value": "rev", "style": {"fg": "accent"},
                                     "tip": {"title": "{m}", "text": "revenue {rev_s} · margin {gm}"}}),
        monthly, size="40%"),
)

sectors = box("Sector returns % · heat", {"type": "table", "id": "sectors", "bind": "fin.sectors", "columns": [
    {"title": "sector", "value": "{s}"},
    *[{"title": t, "value": "{" + k + "}", "size": 7, "align": "right", "heat": {"value": "{" + k + "}", "min": -3, "max": 3}}
      for t, k in [("1D", "d"), ("1W", "w"), ("1M", "m"), ("3M", "q"), ("YTD", "y")]],
]})
risk = col(
    row(*[box(t, col({"type": "big", "bind": "fin.risk", "text": v, "align": "center", "size": 3, "style": {"fg": c}}), size="1*", style={"bg": "panel"})
          for t, v, c in [("VaR 99% · 1d", "{var}", "neg"), ("Beta", "{beta}", "accent"), ("Sharpe", "{sharpe}", "pos"),
                          ("Drawdown", "{dd}", "gold"), ("Vol (ann.)", "{vol}", "fg")]], size=5),
    row(box("Drawdown % from peak", {"type": "chart", "series": [{"name": "drawdown", "bind": "fin.dd", "color": "neg"}]}, "55%"), sectors, size="50%"),
    row(world, news),
)

book = row(positions("pos2", "65%"), col(sectors, news))

ui = {
    "theme": {"bg": "#0d1117", "fg": "#c9d1d9", "panel": "#161b22", "border": "#30363d", "accent": "#58a6ff", "pos": "#3fb950",
              "neg": "#f85149", "hover": "#1f2a3a", "dim": "#6e7681", "gold": "#d29922", "ring": "#2d4a6e"},
    "root": col(
        row(text(" ◆ MERIDIAN CAPITAL", size=20, style={"bold": True, "fg": "accent"}),
            text("live · {time} · tick {tick}", "fin", align="right", style={"fg": "dim"}), size=1),
        {"type": "tabs", "id": "main", "tabs": ["Overview", "P&L", "Book", "Risk & world"], "children": [overview, pnl, book, risk]},
    ),
    "data": {"fin": {"stream": ["python3", "-u", f"{REL}/fin.py", "10"]}},
    "keys": {"1": {"load": f"{REL}/dashboard.json"}},
}
json.dump(ui, open(f"{D}/dashboard.json", "w"), indent=1)
print("wrote dashboard.json")
