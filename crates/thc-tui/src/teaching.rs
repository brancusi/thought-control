//! THC pedagogy on the reusable layers/tour reducer. All scenes use synthetic notes;
//! stepping changes presentation only, never restores or overwrites the person's edits.
use crate::app::App;
use crate::session::{Msg, Session};
use serde_json::{Value, json};
use thc_core::builder::TxBuilder;
use thc_core::vault::Vault;

pub const PAGE: &str = "Overlay Workshop";
pub const TRAIL: &str = "Field Notes";

/// Seed only a new sandbox. Refuse existing data rather than overwrite it.
pub fn seed(vault: &mut Vault) -> anyhow::Result<()> {
    anyhow::ensure!(
        vault.store.nodes_where("1", &[])?.is_empty(),
        "demo requires an empty scratch vault"
    );
    let today = thc_core::dates::today();
    vault.transact(|st| {
        let mut b = TxBuilder::new(st, today);
        let workshop = b.create_page(PAGE, &[])?;
        let trail = b.create_page(TRAIL, &[])?;
        for text in [
            "An idea starts here. Try adding your own words.",
            "Follow [[Field Notes]] to explore a connected page.",
            "[ ] Sketch a small experiment #workshop",
            "[ ] Share what you learned #workshop",
            "Unicode belongs here too: café, 日本語, 🌱.",
        ] {
            b.create_from_capture(
                Some(workshop.clone()),
                &thc_core::capture::parse(text, today)?,
                None,
            )?;
        }
        for text in [
            "A page is a home for related thoughts, not a folder you must choose first.",
            "Link back to [[Overlay Workshop]] whenever an idea connects.",
            "Try editing this page beside the workshop; both panes are real editors.",
        ] {
            b.create_from_capture(
                Some(trail.clone()),
                &thc_core::capture::parse(text, today)?,
                None,
            )?;
        }
        Ok((b.finish(), ()))
    })?;
    Ok(())
}

pub(crate) fn request(app: &App) -> anyhow::Result<Value> {
    let nodes = app.vault.store.nodes_where("1", &[])?;
    let page = nodes
        .iter()
        .find(|n| n.title.as_deref() == Some(PAGE))
        .ok_or_else(|| anyhow::anyhow!("missing workshop"))?
        .id
        .clone();
    let trail = nodes
        .iter()
        .find(|n| n.title.as_deref() == Some(TRAIL))
        .ok_or_else(|| anyhow::anyhow!("missing field notes"))?
        .id
        .clone();
    let find = |prefix: &str| -> anyhow::Result<String> {
        Ok(nodes
            .iter()
            .find(|n| n.text.starts_with(prefix))
            .ok_or_else(|| anyhow::anyhow!("missing demo note"))?
            .id
            .clone())
    };
    let idea = find("An idea")?;
    let link = find("Follow")?;
    let task = find("Sketch")?;
    let trail_intro = find("A page")?;
    let panel = json!({"kind":"page", "vault":app.ui.vault_name, "id":trail});
    let scene = |page: &str, line: &str, write: bool| {
        json!({
            "view":"pages", "page_open":page, "focus":"list", "doc_write":write,
            "document":{"caret_id":line,"caret_byte":if line == link { 9 } else { 0 },"scroll":0},
            "link_open":false,"link_sel":null,"sidebar":{"shown":false}
        })
    };
    Ok(
        json!({"op":"tour.start", "id":"thc.teaching", "title":"Learn THC by doing", "spotlight":true, "steps":[
            {"id":"welcome", "anchor":format!("row:{idea}"), "title":"1 · A safe place to explore",
             "text":"These are scratch notes. F2: next; Shift-F2: back; F3: stop the guide. Ctrl-Q exits. Nothing touches your own vault.",
             "host":scene(&page,&idea,false)},
            {"id":"write", "anchor":"caret@main", "title":"2 · Write, then reshape",
             "text":"You are writing now. Add a few words; use arrows and Backspace to edit. Enter is the existing editor's line break. Esc returns to the page list. F2 continues when ready.",
             "host":scene(&page,&idea,true)},
            {"id":"link", "anchor":format!("row:{link}"), "title":"3 · Thoughts connect",
             "text":"Ctrl-O follows the link on this line (or click its underlined name). Ctrl-Alt-Left goes back; Ctrl-Alt-Right goes forward. Explore, then F2.",
             "host":scene(&page,&link,false)},
            {"id":"navigate", "anchor":format!("row:{trail_intro}"), "title":"4 · Pages, not silos",
             "text":"This is Field Notes. Click its workshop link to return. Esc returns to the page list, where keys 1–5 select Today, Inbox, Tasks, Pages and Journal. F2 opens a companion panel.",
             "host":scene(&trail,&trail_intro,false)},
            {"id":"panel", "anchor":"panel:0", "title":"5 · Keep context beside you",
             "text":"This is a live page panel. Alt-S focuses this panel; click the main text to return. Type directly to edit. Alt-W closes the active panel. Below 120 columns it becomes a drawer or replaces the main pane. F2 continues.",
             "host":{"view":"pages","page_open":page,"doc_write":false,"focus":"sidebar","sidebar":{"shown":true,"open":[panel],"focused":panel}}},
            {"id":"tasks", "anchor":format!("row:{task}"), "title":"6 · Turn a thought into action",
             "text":"Press x to complete this scratch task. Press Shift-X to reopen it. A task keeps its page context; Enter opens the note. F2 reaches the finale.",
             "host":{"view":"tasks","page_open":null,"doc_write":false,"focus":"list","selected":task,"tasks_filter":"status:any #workshop","sidebar":{"shown":false}}},
            {"id":"finish", "anchor":format!("row:{idea}"), "title":"7 · Your turn",
             "text":"Your edits are still here. F2 finishes; F3 dismisses. Keep exploring by typing, following links, opening panels and completing tasks; Ctrl-Q exits. Launch with --keep to preserve this scratch session.",
             "host":scene(&page,&idea,false)}
        ]}),
    )
}

pub(crate) fn start(session: &mut Session) -> anyhow::Result<()> {
    let req = request(&session.app)?;
    session.app.ui.teaching_demo = true;
    session
        .apply(Msg::Layer { req, actor: None })
        .map_err(anyhow::Error::msg)?;
    Ok(())
}
