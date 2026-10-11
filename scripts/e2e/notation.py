"""The golden notation (editing.md §0), shared by every target.

`Hello ⟦wor▮⟧ld`: `▮` is the caret, `⟦…⟧` the selection (the caret at one end, the anchor at the
other), ` ‖ ` a boundary between notes (` ¦ ` one without a blank line), `⏎` a soft break inside a
note. thc documents are notes: leading pairs of spaces nest a note, `[ ] ` / `[x] ` make it a task
and `- ` a list item; anything else is a paragraph. A plain-text editor (fictty's) reads a note
boundary and a soft break both as a newline.
"""

import re

NOTE = re.compile(r" ?[‖¦] ?")


def _marks(golden):
    """The text with the marks taken out, and the caret and anchor as offsets into it."""
    text, head, anchor, opened = [], None, None, None
    for c in golden:
        at = len(text)
        if c == "⟦":
            opened = at
        elif c == "⟧":
            anchor = at if head == opened else opened
        elif c == "▮":
            head = at
        else:
            text.append(c)
    if head is None:
        raise ValueError(f"no caret in {golden!r}")
    return "".join(text), head, (head if anchor is None else anchor)


# ---- plain text (fictty editors) -----------------------------------------------------------------

def plain(golden):
    """(text, anchor, head) in characters, notes and soft breaks as newlines."""
    g = NOTE.sub("\n", golden).replace("⏎", "\n")
    text, head, anchor = _marks(g)
    return text, anchor, head


def show_plain(text, anchor, head):
    """A plain text and selection back as a golden, newlines shown as ⏎."""
    lo, hi = min(anchor, head), max(anchor, head)
    out = []
    for i, c in enumerate(list(text) + [None]):
        if lo != hi and i == lo:
            out.append("⟦")
        if i == head:
            out.append("▮")
        if lo != hi and i == hi:
            out.append("⟧")
        if c is not None:
            out.append("⏎" if c == "\n" else c)
    return "".join(out)


def norm_plain(golden):
    """A golden as a plain-text editor shows it (for comparing)."""
    return show_plain(*plain(golden))


# ---- notes (thc documents) -----------------------------------------------------------------------

MARK = re.compile(r"^(?P<ind>(?:  )*)(?P<mark>\[ \] |\[x\] |- )?")


def notes(golden):
    """[{depth, kind, status, text}], and the anchor and caret as (note, char offset in its text)."""
    g = golden.replace(" ¦ ", " ‖ ")
    parts = g.split(" ‖ ") if g.strip() not in ("▮",) else [g]
    out, head, anchor, open_at = [], None, None, None
    for i, part in enumerate(parts):
        # Marks may sit inside the decoration (`⟦- one`): read the decoration without them.
        bare = part.replace("⟦", "").replace("⟧", "").replace("▮", "")
        m = MARK.match(bare)
        depth = len(m.group("ind")) // 2
        mark = m.group("mark") or ""
        kind = {"[ ] ": "task", "[x] ": "task", "- ": "bullet"}.get(mark, "para")
        status = {"[ ] ": "todo", "[x] ": "done"}.get(mark)
        skip = len(m.group("ind")) + len(mark)
        text, n = [], 0
        for c in part:
            if c in "⟦⟧▮":
                at = (i, max(0, len(text)))
                if c == "⟦":
                    open_at = at
                elif c == "⟧":
                    anchor = at if head == open_at else open_at
                else:
                    head = at
                continue
            if n < skip:
                n += 1
                continue
            text.append("\n" if c == "⏎" else c)
        out.append({"depth": depth, "kind": kind, "status": status, "text": "".join(text)})
    if head is None:
        raise ValueError(f"no caret in {golden!r}")
    return out, (head if anchor is None else anchor), head


def show_notes(blocks, anchor, head):
    """Notes and a selection (each end (note, char offset), anchor None for none) as a golden."""
    lo, hi = (min(anchor, head), max(anchor, head)) if anchor is not None and anchor != head else (None, None)
    parts = []
    for i, b in enumerate(blocks):
        mark = {"todo": "[ ] ", "done": "[x] "}.get(b.get("status") or "", "[ ] ") if b["kind"] == "task" else ("- " if b["kind"] == "bullet" else "")
        # An empty line's kind isn't text yet: a page's fresh line reads `▮`, whatever it will be.
        if b["kind"] == "bullet" and b["text"] == "":
            mark = ""
        s = ["  " * b["depth"]]
        # A selection that starts at the note's start takes in its decoration, as the golden does.
        dec_open = lo is not None and lo == (i, 0)
        if dec_open:
            s.append("⟦")
        s.append(mark)
        t = b["text"]
        for j, c in enumerate(list(t) + [None]):
            if lo is not None and (i, j) == lo and not dec_open:
                s.append("⟦")
            if (i, j) == head:
                s.append("▮")
            if hi is not None and (i, j) == hi:
                s.append("⟧")
            if c is not None:
                s.append("⏎" if c == "\n" else c)
        parts.append("".join(s))
    return " ‖ ".join(parts)


def norm_notes(golden):
    return show_notes(*notes(golden))
