//! fictty-probe: a fictty screen without a terminal (fictty's `App::headless`), driven by a plan
//! on stdin and read back as JSON on stdout. The e2e harness (scripts/e2e/e2e.py) builds it
//! against a fictty source tree, writing its Cargo.toml beside a copy of this file, so thc's
//! workspace never depends on fictty.
//!
//! Plan: `{"ui": PATH, "store": PATH?, "themes": PATH?, "size": [W, H], "dir": PATH?,
//!          "steps": [STEP]}`, a STEP one of
//! - `{"key": NAME}`: a key by fictty's name (`d-s-z`, `a-left`, `c-home`)
//! - `{"msg": MSG}`: any message, as the socket takes it (`{"msg": "paste", "text": "…"}`)
//! - `{"point": {"id", "find", "at", "origin", "extend"?}}`: a click in editor `id`'s text at the
//!   `at`-th character of the first screen row holding `find`, relative to where `origin` (the
//!   editor's first line) is drawn. Headless has no hit map, so this stands in for the runtime's
//! - `{"drag": {…same, no extend}}`: the pointer dragged there with the button down
//! - `{"snap": LABEL}`: record the state and screen now
//!
//! Output: `{"snaps": [{"label", "state", "screen", "copied"}]}`, the last labelled "end".

use serde_json::{Value, json};
use std::io::Read;

/// Where `needle` first shows on screen: its column (in cells, ASCII) and row.
fn locate(screen: &str, needle: &str) -> Option<(u16, u16)> {
    screen.lines().enumerate().find_map(|(y, l)| {
        let i = l.find(needle)?;
        Some((l[..i].chars().count() as u16, y as u16))
    })
}

/// A pointer step's cell, relative to the editor's text.
fn cell(screen: &str, p: &Value) -> anyhow::Result<(String, u16, u16)> {
    let id = p["id"].as_str().unwrap_or("body").to_string();
    let find = p["find"].as_str().unwrap_or_default();
    let origin = p["origin"].as_str().unwrap_or(find);
    let at = p["at"].as_u64().unwrap_or(0) as u16;
    let (fx, fy) = locate(screen, find).ok_or_else(|| anyhow::anyhow!("{find:?} isn't on screen:\n{screen}"))?;
    let (ox, oy) = locate(screen, origin).ok_or_else(|| anyhow::anyhow!("{origin:?} isn't on screen"))?;
    Ok((id, fx + at - ox, fy - oy))
}

fn main() -> anyhow::Result<()> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    let plan: Value = serde_json::from_str(&raw)?;
    if let Some(dir) = plan["dir"].as_str() {
        std::env::set_current_dir(dir)?;
    }
    let mut app = fictty::App::file(plan["ui"].as_str().expect("plan.ui"));
    if let Some(p) = plan["store"].as_str() {
        app = app.store(p);
    }
    if let Some(p) = plan["themes"].as_str() {
        app = app.themes_file(std::path::Path::new(p))?;
    }
    let w = plan["size"][0].as_u64().unwrap_or(100) as u16;
    let h = plan["size"][1].as_u64().unwrap_or(30) as u16;
    let mut screen = app.headless(w, h)?;
    let mut snaps = Vec::new();
    let snap = |screen: &mut fictty::Headless, label: &str, snaps: &mut Vec<Value>| {
        let text = screen.screen();
        let copied = screen.copied().to_vec();
        let state = serde_json::to_value(screen.state()).unwrap_or(Value::Null);
        snaps.push(json!({"label": label, "state": state, "screen": text, "copied": copied}));
    };
    for step in plan["steps"].as_array().cloned().unwrap_or_default() {
        if let Some(k) = step["key"].as_str() {
            screen.keys(&[k]);
        } else if let Some(m) = step.get("msg") {
            let msg: fictty::state::Msg = serde_json::from_value(m.clone())?;
            screen.msg(msg);
        } else if let Some(p) = step.get("point") {
            let (id, col, row) = cell(&screen.screen(), p)?;
            let extend = p["extend"].as_bool().unwrap_or(false);
            screen.msg(serde_json::from_value(json!({"msg": "point", "id": id, "col": col, "row": row, "extend": extend}))?);
        } else if let Some(p) = step.get("drag") {
            let (id, col, row) = cell(&screen.screen(), p)?;
            screen.msg(serde_json::from_value(json!({"msg": "drag", "id": id, "col": col, "row": row}))?);
        } else if let Some(l) = step["snap"].as_str() {
            snap(&mut screen, l, &mut snaps);
        }
    }
    snap(&mut screen, "end", &mut snaps);
    println!("{}", json!({"snaps": snaps}));
    Ok(())
}
