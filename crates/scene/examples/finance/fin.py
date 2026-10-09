# A live (made-up) finance feed: one JSON line per tick with everything the dashboard shows.
#   fin.py [ticks per second]
import json, math, random, sys, time
random.seed(7)
hz = float(sys.argv[1]) if len(sys.argv) > 1 else 10
def money(x, d=0): return f"{x:,.{d}f}"
def signed(x, d=0, suffix=""): return f"{x:+,.{d}f}{suffix}"
def short(x):
    a = abs(x)
    return f"{'-' if x < 0 else ''}${a/1e6:.2f}M" if a >= 1e6 else f"{'-' if x < 0 else ''}${a/1e3:.0f}K"

MONTHS = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"]
months = []
for i, m in enumerate(MONTHS):
    rev = 3.2e6 + i * 1.4e5 + random.gauss(0, 1.2e5)
    cogs = rev * random.uniform(0.33, 0.41)
    opex = 1.5e6 + random.gauss(0, 1e5) + (1.9e6 if m == "Mar" else 0) + (0.8e6 if m == "Jul" else 0)
    months.append({"m": m, "rev": rev, "cogs": cogs, "opex": opex})
BOOK = [("NVDA","NVIDIA",1200,121.4),("AAPL","Apple",2600,228.1),("MSFT","Microsoft",900,415.2),("AMZN","Amazon",1500,186.3),
        ("GOOGL","Alphabet",1700,163.9),("META","Meta",500,571.8),("TSLA","Tesla",800,244.2),("AVGO","Broadcom",300,1650.0),
        ("JPM","JPMorgan",1100,212.5),("V","Visa",700,289.0),("XOM","Exxon",1900,118.4),("LLY","Eli Lilly",250,902.6),
        ("COST","Costco",200,884.1),("ASML","ASML",180,702.3),("TSM","TSMC",1300,186.7),("BRK.B","Berkshire",400,452.0)]
pos = [{"sym": s, "name": n, "qty": q, "cost": p * random.uniform(0.7, 1.15), "px": p, "open": p} for s, n, q, p in BOOK]
SECTORS = ["Semis", "Software", "Internet", "Consumer", "Financials", "Energy", "Health", "Industrials", "Materials", "Utilities"]
heat = {s: [random.gauss(0, 1.5) for _ in range(5)] for s in SECTORS}
REGIONS = [("NYC", 40.7, -74.0), ("SF", 37.8, -122.4), ("London", 51.5, -0.1), ("Berlin", 52.5, 13.4), ("Tokyo", 35.7, 139.7),
           ("Singapore", 1.35, 103.8), ("Sydney", -33.9, 151.2), ("São Paulo", -23.5, -46.6), ("Toronto", 43.7, -79.4), ("Mumbai", 19.1, 72.9)]
region_rev = {r[0]: random.uniform(0.2e6, 1.4e6) for r in REGIONS}
equity, bench = [1_000_000.0], [1_000_000.0]
for _ in range(299):
    equity.append(equity[-1] * (1 + random.gauss(0.0006, 0.009)))
    bench.append(bench[-1] * (1 + random.gauss(0.0003, 0.007)))
NEWS = ["Fed holds rates; signals two cuts in 2027", "NVDA beats on data-center revenue", "Oil slides 3% on supply build",
        "Treasury 10y at 4.12%", "TSMC guides capex higher", "EU fines a big tech firm €1.2B", "Payrolls +212k vs 180k est.",
        "Copper hits 6-month high", "Yen weakens past 152", "Apple unveils on-device model", "Credit spreads tighten 4bp"]
news, tick, t0 = [], 0, time.time()
while True:
    tick += 1
    for p in pos:
        p["px"] *= 1 + random.gauss(0, 0.0012)
    for s in SECTORS:
        heat[s][0] += random.gauss(0, 0.05)
    equity.append(equity[-1] * (1 + random.gauss(0.0004, 0.002))); equity.pop(0)
    bench.append(bench[-1] * (1 + random.gauss(0.0002, 0.0016))); bench.pop(0)
    for r in region_rev: region_rev[r] *= 1 + random.gauss(0, 0.002)
    if tick % 25 == 1:
        news.insert(0, {"t": time.strftime("%H:%M:%S"), "h": random.choice(NEWS), "imp": random.choice(["▲", "▼", "•"])}); del news[14:]
    rows, total_mv, total_pnl, day_pnl = [], 0, 0, 0
    for p in pos:
        mv, pnl, dpnl = p["qty"] * p["px"], p["qty"] * (p["px"] - p["cost"]), p["qty"] * (p["px"] - p["open"])
        total_mv += mv; total_pnl += pnl; day_pnl += dpnl
        rows.append({**p, "mv": mv, "pnl": pnl, "dpnl": dpnl, "day": (p["px"] / p["open"] - 1) * 100, "ret": (p["px"] / p["cost"] - 1) * 100})
    for r in rows:
        r.update(qty_s=money(r["qty"]), px_s=money(r["px"], 2), mv_s=money(r["mv"]), pnl_s=signed(r["pnl"]), ret_s=signed(r["ret"], 1, "%"),
                 day_s=signed(r["day"], 2, "%"), dpnl_s=signed(r["dpnl"]), w=f"{r['mv'] / total_mv * 100:.1f}%",
                 stop=money(r["px"] * 0.92, 2), cost_s=money(r["cost"], 2))
    rows.sort(key=lambda r: -r["mv"])
    worst = min(rows, key=lambda r: r["day"]); best = max(rows, key=lambda r: r["day"])
    rev = sum(m["rev"] for m in months[-3:]); cogs = sum(m["cogs"] for m in months[-3:]); opex = sum(m["opex"] for m in months[-3:])
    gp = rev - cogs; ebitda = gp - opex; da = 2.1e5; interest = 0.9e5; tax = max(0, (ebitda - da - interest) * 0.21); net = ebitda - da - interest - tax
    budget = {"Revenue": rev * 0.94, "COGS": cogs * 0.97, "Gross profit": gp * 0.92, "R&D": opex * 0.38, "Sales & marketing": opex * 0.36,
              "G&A": opex * 0.21, "EBITDA": ebitda * 1.12, "D&A": da, "Interest": interest * 0.9, "Tax": tax * 1.1, "Net income": net * 1.15}
    actual = {"Revenue": rev, "COGS": cogs, "Gross profit": gp, "R&D": opex * 0.41, "Sales & marketing": opex * 0.37, "G&A": opex * 0.22,
              "EBITDA": ebitda, "D&A": da, "Interest": interest, "Tax": tax, "Net income": net}
    cost_lines = {"COGS", "R&D", "Sales & marketing", "G&A", "D&A", "Interest", "Tax"}
    stmt = []
    for line, a in actual.items():
        b = budget[line]; v = (b - a) if line in cost_lines else (a - b)
        stmt.append({"line": ("  " if line in cost_lines else "") + line, "a": money(a / 1e3), "b": money(b / 1e3), "v": signed(v / 1e3),
                     "vp": signed(v / b * 100 if b else 0, 1, "%"), "vn": v / b * 100 if b else 0, "bold": line in {"Gross profit", "EBITDA", "Net income"}})
    nav = equity[-1]; peak = max(equity); dd = (nav / peak - 1) * 100
    rets = [equity[i] / equity[i - 1] - 1 for i in range(1, len(equity))]
    mu = sum(rets) / len(rets); sd = (sum((x - mu) ** 2 for x in rets) / len(rets)) ** 0.5
    out = {
        "tick": tick, "time": time.strftime("%H:%M:%S"),
        "kpi": {"rev": short(rev), "rev_chg": "+12.4%", "gm": f"{gp / rev * 100:.1f}%", "gm_chg": "-4.1", "cash": short(18.2e6 - tick * 40),
                "runway": "22 mo", "nav": short(nav), "day": signed(day_pnl), "day_short": short(day_pnl), "ebitda": short(ebitda),
                "net": short(net), "mv": short(total_mv), "upnl": short(total_pnl)},
        "months": [{"m": m["m"], "rev": round(m["rev"] / 1e3), "net": round((m["rev"] - m["cogs"] - m["opex"]) / 1e3),
                    "rev_s": short(m["rev"]), "net_s": short(m["rev"] - m["cogs"] - m["opex"]), "gm": f"{(1 - m['cogs'] / m['rev']) * 100:.1f}%"} for m in months],
        "stmt": stmt, "equity": equity, "bench": bench,
        "dd": [(e / max(equity[:i + 1]) - 1) * 100 for i, e in enumerate(equity)],
        "positions": rows, "worst": worst, "best": best,
        "sectors": [{"s": s, "d": signed(v[0], 2), "w": signed(v[1], 2), "m": signed(v[2], 2), "q": signed(v[3], 2), "y": signed(v[4] * 3, 1)} for s, v in heat.items()],
        "regions": [{"city": c, "lat": la, "lon": lo, "rev": short(region_rev[c]), "share": f"{region_rev[c] / sum(region_rev.values()) * 100:.1f}%"} for c, la, lo in REGIONS],
        "aging": [{"b": b, "v": v} for b, v in [("0-30", 1840), ("31-60", 920), ("61-90", 410), ("91-120", 220), ("120+", 160)]],
        "cash_bridge": [{"k": k, "v": v} for k, v in [("Open", 900), ("Ops", 640), ("Capex", -380), ("Debt", -210), ("FX", -45), ("Equity", 320), ("Tax", -160)]],
        "budget": {"rd": 0.72 + 0.02 * math.sin(tick / 30), "sm": 0.93, "ga": 0.58, "hire": 0.81},
        "risk": {"var": short(nav * 1.65 * sd * 10), "beta": f"{1.08 + 0.02 * math.sin(tick / 40):.2f}", "sharpe": f"{mu / sd * 15.87:.2f}",
                 "dd": f"{dd:.1f}%", "vol": f"{sd * 15.87 * 100:.1f}%", "lev": "1.3x"},
        "news": news,
    }
    sys.stdout.write(json.dumps(out) + "\n"); sys.stdout.flush()
    d = t0 + tick / hz - time.time()
    if d > 0: time.sleep(d)
