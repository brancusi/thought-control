# Writes today.json: thc's Today screen as a scene UI, styled with the ember-dark tokens.
import json, os
D = os.path.dirname(os.path.abspath(__file__))
REL = "crates/scene/examples/thc"
EMBER_DARK = {"text": "#ece5d8", "muted": "#a0968a", "accent": "#e8834f", "line": "#3a352f", "selection": "#352e28",
              "today": "#e2b54a", "overdue": "#f2766e", "done": "#8fc487", "doing": "#6cc5ce", "agent": "#88b1ec",
              "tag": "#cca884", "conflict": "#d49aD0", "surface": "#24211d", "raised": "#2c2824", "overdue-tint": "#3a2220",
              "border": "#3a352f", "hover": "#2c2824", "dim": "#a0968a"}
def text(t, size=None, **kw):
    n = {"type": "text", "text": t, **kw}
    if size is not None: n["size"] = size
    return n
GLYPH = "{status|todo=<text>[ ]</>;doing=<doing>[/]</>;waiting=<muted>[w]</>;done=<done>[x]</>;cancelled=<muted>[-]</>;note=<muted> · </>;*=<text>[ ]</>}"
header = text(" <muted>[</><accent+b>•</><muted>]</> <accent>{vault}</>     <text+b>Today</>   <muted>Inbox</><text>{inbox|? $}</>   "
              "<muted>Tasks</>   <muted>Pages</>   <muted>Journal</>   <muted>Search</>   <muted>Log</><agent>{log|? $}</>\t<muted>{date}</> ",
              1, bind="feed")
rule = {"type": "row", "size": 1, "children": [
    text("<line>──────────</><accent>━━━━━━━</><line>{*─}</>", 90),
    text("<line>┬</>", 1),
    text("<line>{*─}</>")]}
today = {"type": "list", "id": "today", "bind": "feed.rows", "size": 90, "variant": "kind", "skip": ["section", "blank"],
         "variants": {"section": " <text+b>{title}</>  <muted>{count}</>", "blank": "",
                      "done": "<muted>{short}</>  <done>[x]</> <muted>{text}</>\t{meta_m|markup} "},
         "item": "<muted>{short}</>  " + GLYPH + " {text_m|markup}\t{meta_m|markup} ",
         "mark": "<accent+b>▌</> ", "lead": "  ", "selected": "on-selection", "style": {"fg": "text"}}
KEYS = "<text>e</> <muted>edit</>  <text>x</> <muted>done</>  <text>d</> <muted>date</>  <text>p</> <muted>priority</>  <text>m</> <muted>move</>"
def hist(i): return f"{{history.{i}.t|?<muted>$</>  }}{{history.{i}.who|?<agent>◆ $</>}}{{history.{i}.what|?<muted>$</>}}"
def kid(i): return f"{{kids.{i}.status|todo= <text>[ ]</> ;done= <done>[x]</> }}{{kids.{i}.text}}\t{{kids.{i}.meta_m|markup}} "
detail = text("\n".join([
    GLYPH + " <text+b>{text_m|markup}</>",
    "<muted>{crumbs}</>",
    "",
    "{status|note=;*=<muted>status     </>}{status|done=<done>$</>;note=;*=<text>$</>}",
    "{sched_l|?<muted>scheduled  </><text>$</>}{sched_rel|?<muted> · $</>}",
    "{due_l|?<muted>due        </><text>$</>}{due_rel|?<muted> · $</>}",
    "{prio_m|?<muted>priority   </>}{prio_m|markup}",
    "{repeat_l|?<muted>repeat     </><text>$</>}",
    "{alert_l|?<muted>alert      </><text>$</>}",
    "{tags_m|?<muted>tags       </>}{tags_m|markup}",
    "<muted>created    </><text>{created_l} · </><agent>◆ {by}</>",
    "{why|?<muted>why here   </><text>$</>}",
    "",
    "{subtasks|?<text+b>Subtasks  $</>}", kid(0), kid(1), kid(2), "{subtasks|? }",
    "{history.0.t|?<text+b>History</>}", hist(0), hist(1), hist(2),
    "{more|true=<text>L</> <muted>full history</>  <text>R</> <muted>rewind…</>}",
    "",
    KEYS,
]), bind="@today", style={"fg": "text"})
ui = {
    "theme": EMBER_DARK,
    "status_bar": False,
    "root": {"type": "col", "children": [
        header, rule,
        {"type": "row", "children": [today, {"type": "rule", "size": 1}, text("", 1), detail]},
        text(" <overdue>{bar}</>", 1, bind="feed", style={"bg": "overdue-tint"}),
    ]},
    "data": {"feed": {"cmd": ["python3", f"{REL}/thcfeed.py"], "every": 3}},
}
json.dump(ui, open(f"{D}/today.json", "w"), indent=1)
print("wrote today.json")
