# Copy dossiers into the site, normalising verdicts ("use (as …)" → "use", the rest into verdict_why)
# and applying corrections that the researchers couldn't know.
import json, glob, os, re, sys
SRC, DST = sys.argv[1], sys.argv[2]
V = ["use", "adopt", "borrow", "complement", "ignore"]
FIX = {
    "ratatui": {"relation_to_scene.scene_reinvents_note": None},
}
for f in sorted(glob.glob(SRC + "/*.json")):
    d = json.load(open(f))
    rel = d.setdefault("relation_to_scene", {})
    v = (rel.get("verdict") or "").strip()
    first = re.split(r"[\s(/,;:]", v.lower(), 1)[0] if v else ""
    if first in V and v.lower() != first:
        extra = v[len(first):].strip(" ()-–—:;,")
        rel["verdict"] = first
        if extra and extra.lower() not in (rel.get("verdict_why") or "").lower():
            rel["verdict_why"] = f"{extra[0].upper()}{extra[1:]}. {rel.get('verdict_why') or ''}".strip()
    elif first in V:
        rel["verdict"] = first
    if d["id"] == "ratatui":
        # scene already renders through ratatui (crates/scene depends on ratatui 0.30).
        rel["overlap"] = (rel.get("overlap") or "") + " thc-scene renders through ratatui today: its view draws into a ratatui buffer."
    json.dump(d, open(os.path.join(DST, os.path.basename(f)), "w"), indent=1, ensure_ascii=False)
    print(d["id"], rel.get("verdict"))
