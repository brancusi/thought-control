# thc's Today as a view model for a scene UI: header counts, section and row records (with meta
# and inline tokens as <style> markup) and each row's detail. The UI decides layout and look.
#   thcfeed.py          (uses THC_VAULT / THC_NOW like thc itself)
import datetime as dt, json, os, re, subprocess, sys

def thc(*args):
    out = subprocess.run(["thc", *args, "--json"], capture_output=True, text=True)
    return json.loads(out.stdout) if out.stdout.strip() else {}

now = dt.datetime.fromisoformat(os.environ["THC_NOW"]) if os.environ.get("THC_NOW") else dt.datetime.now()
today = now.date()
nodes = {}
def node(i):
    if i not in nodes: nodes[i] = thc("show", i, "--depth", "0")
    return nodes[i]

def day(d):
    k = (d - today).days
    if k == 0: return "today"
    if k == 1: return "tomorrow"
    if 1 < k < 7: return d.strftime("%a").lower()
    return f"{d.strftime('%b')} {d.day} (in {k}d)"

def when(s):  # "2026-10-12T09:00" → (date, "09:00" or None)
    if not s: return None, None
    d = dt.datetime.fromisoformat(s) if "T" in s else dt.datetime.fromisoformat(s + "T00:00")
    return d.date(), (s.split("T")[1] if "T" in s else None)

UNITS = {"DAILY": ("d", "daily"), "WEEKLY": ("w", "weekly"), "MONTHLY": ("mo", "monthly"), "YEARLY": ("y", "yearly")}
def repeat_short(r):
    if not r: return None
    rule = dict(p.split("=") for p in r["rule"].split(";"))
    n = int(rule.get("INTERVAL", 1)); u = UNITS.get(rule.get("FREQ"), ("", ""))
    s = u[1] if n == 1 else f"{n}{u[0]}"
    return s + ("!" if r.get("mode") == "from_done" else "")

def page_of(n):
    p = n.get("parent")
    while p:
        x = node(p)
        if x.get("kind") == "page": return x.get("title") or x.get("text")
        p = x.get("parent")

def inline(text):
    text = text.replace("<", "‹")
    text = re.sub(r"\[\[([^\]]+)\]\]", r"<=muted>[[</><text+u>\1</><=muted>]]</>", text)
    return re.sub(r"(?<![\w>])#([\w-]+)", r"<tag>#\1</>", text)

def meta(n, section):
    parts = []
    sd, st = when(n.get("scheduled")); dd, _ = when(n.get("due"))
    alerts = {a["node"]: a for a in thc("alert", "ls").get("alerts", [])} if "alerts" not in globals() else globals()["alerts"]
    if sd:
        word = day(sd)
        tone = "today" if section == "today" and sd == today else "muted"
        s = word + (f" {st}" if st else "") + (" ◎" if n["id"] in alerts else "")
        parts.append(f"<{tone}>{s}</>")
    if dd:
        tone = "today" if dd == today else "muted"
        parts.append(f"<{tone}>due {day(dd)}</>")
    if n.get("priority") == "high": parts.append("<text+b>!high</>")
    elif n.get("priority") in ("med", "low"): parts.append(f"<muted>!{n['priority']}</>")
    r = repeat_short(n.get("repeat"))
    if r: parts.append(f"<muted>↻ {r}</>")
    by = n.get("created_by", "")
    if by.startswith("agent:"): parts.append(f"<agent>◆ {by[6:]}</>")
    p = page_of(n)
    if p: parts.append(f"<muted>¶ {p}</>")
    return "<muted> · </>".join(parts)

def rel(d):  # the detail pane's "· mon", "· in 7d"
    k = (d - today).days
    return day(d) if k < 7 else f"in {k}d"

def crumbs(n):
    out, p = [], n.get("parent")
    while p:
        x = node(p)
        if x.get("kind") == "journal" or x.get("journal"):
            jd = dt.date.fromisoformat(x.get("journal") or x.get("title"))
            k = (today - jd).days
            out.append("§ " + {0: "today", 1: "yesterday"}.get(k, f"{jd.strftime('%a %b')} {jd.day}"))
        elif x.get("kind") == "page":
            out.append(f"¶ {x.get('title') or x.get('text')}")
        else:
            out.append(x.get("text", ""))
        p = x.get("parent")
    return " › ".join(list(reversed(out)) + [n["short"]])

def label(i):
    x = node(i)
    return x.get("title") or x.get("text") or i

def what(e):
    b = e["body"]
    if e["op"] == "node.create": return "created"
    if e["op"] == "node.complete": return "done"
    if e["op"] == "alert.add": return "◎ alert"
    if e["op"] == "edge.add" and b.get("rel") == "tag": return f"tagged #{label(b['dst'])}"
    if e["op"] == "edge.add" and b.get("rel") == "mention": return f"links ¶ {label(b['dst'])}"
    return e["op"].split(".")[-1]

def why(n, sd, st, dd):
    if n.get("reasons"):
        words = {"scheduled": "scheduled today", "repeating": "repeating", "due-today": "due today", "overdue": "overdue",
                 "done-today": "done today", "doing": "in progress"}
        return " · ".join(words.get(r, r) for r in n["reasons"] if r in words)
    parts = []
    if sd: parts.append(f"scheduled {day(sd)}")
    elif dd: parts.append(f"due {day(dd)}")
    if n["id"] in alerts: parts.append(f"alert {alerts[n['id']]['fire_at'].split('T')[1]}")
    if n.get("repeat"): parts.append("repeating")
    return " · ".join(parts)

def detail(n):
    sd, st = when(n.get("scheduled")); dd, _ = when(n.get("due"))
    fmt = lambda d: f"{d.strftime('%a %b')} {d.day}"
    created = dt.datetime.fromisoformat(n["created"])
    hist = thc("history", n["id"]).get("events", [])
    a = alerts.get(n["id"])
    kids = [c for c in thc("q", f"parent:{n['id']} is:task").get("items", [])]
    return {
        "crumbs": crumbs(n),
        "sched_l": (fmt(sd) + (f" {st}" if st else "")) if sd else "", "sched_rel": rel(sd) if sd else "",
        "due_l": fmt(dd) if dd else "", "due_rel": rel(dd) if dd else "",
        "prio_m": {"high": "<text+b>!high</>", "med": "<muted>!med</>", "low": "<muted>!low</>"}.get(n.get("priority"), ""),
        "repeat_l": f"↻ {n['repeat']['text']}" if n.get("repeat") else "",
        "alert_l": f"◎ {a['fire_at'].replace('T', ' ')} · {a['state']}" if a else "",
        "tags_m": " ".join(f"<tag>#{t}</>" for t in n.get("tags", [])),
        "created_l": f"{created.strftime('%b')} {created.day} {created.strftime('%H:%M')}",
        "by": n.get("created_by", "").replace("agent:", ""),
        "why": why(n, sd, st, dd),
        "subtasks": f"{sum(1 for c in kids if c.get('status') == 'done')}/{len(kids)}" if kids else "",
        "kids": [{"status": c.get("status", "todo"), "text": c["text"], "meta_m": "<muted>" + (f"due {day(when(c['due'])[0])}" if c.get("due") else "") + "</>"} for c in kids],
        "history": [{"t": dt.datetime.fromtimestamp(e["ms"] / 1000).strftime("%H:%M"),
                     "who": e["actor"].replace("agent:", ""), "what": what(e)} for e in hist[:3]],
        "more": len(hist) > 1,
    }

def row(n, section):
    status = n.get("status") or "note"
    r = {"kind": "done" if status == "done" else "node", "id": n["id"], "short": n["short"], "status": status,
         "text": n["text"], "text_m": inline(n["text"]), "meta_m": meta(n, section), **detail(n)}
    if status == "done":
        r["meta_m"] = f"<muted>{n['done_at'].split('T')[1]}</>"
    return r

alerts = {a["node"]: a for a in thc("alert", "ls").get("alerts", [])}
t = thc("today")
nxt = thc("q", "happens<=+7d happens>today (status:open or is:event) sort:date").get("items", [])
key = lambda n: (min(x for x in [when(n.get("scheduled"))[0], when(n.get("due"))[0]] if x), when(n.get("scheduled"))[1] or "", 0 if n.get("scheduled") else 1)
nxt.sort(key=key)
rows = []
for title, items, sec in [("Overdue", t.get("overdue", []), "overdue"), ("Today", t.get("today", []), "today"),
                          ("Next 7 days", nxt, "next"), ("Done today", t.get("done", []), "done")]:
    if not items: continue
    if rows: rows.append({"kind": "blank"})
    rows.append({"kind": "section", "title": title, "count": len(items)})
    rows += [row(n, sec) for n in items]
out = {
    "vault": thc("vault").get("name", ""),
    "date": f"{now.strftime('%a %b')} {now.day} · {now.strftime('%H:%M')}",
    "inbox": thc("q", "is:inbox").get("count", 0),
    "log": thc("review").get("pending", 0),
    "rows": rows,
    "bar": "THC_FIXTURE_IDS is set: log ids are derived, not random · unset it (and THC_NOW) unless you're testing" if os.environ.get("THC_FIXTURE_IDS") else "",
}
print(json.dumps(out))
