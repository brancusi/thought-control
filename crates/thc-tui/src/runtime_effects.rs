//! Executes value effects and delivers explicit results. IO belongs here, never
//! in update. Additional legacy effects migrate in later P3 steps.
use crate::{
    app::App,
    update::{self, DocumentFields, DocumentIdentity, Effect, Fields, Msg},
};

pub(crate) fn document_identity(app: &App) -> Option<DocumentIdentity> {
    app.doc.as_ref().map(|doc| DocumentIdentity {
        vault: app.vault.paths.vault.clone(),
        target: doc.target.clone(),
        revision: doc.revision(),
        caret: doc.caret(),
    })
}
pub(crate) fn dispatch(app: &mut App, msg: Msg) {
    let identity = document_identity(app);
    let document = identity.zip(app.doc.as_mut()).map(|(identity, doc)| DocumentFields { identity, scroll: &mut doc.scroll });
    let effects = update::update(
        Fields { page_ids: Some(&mut app.ui.page_ids), cursor: app.ui.cursor, scroll: &mut app.ui.scroll, document, toast: &mut app.ui.toast },
        msg,
    );
    for effect in effects {
        match effect {
            Effect::EditKeys => {
                app.save_doc(true);
                app.drain_saves(true);
                app.overlay = None;
                app.editor_request = Some("@keys".into());
            }
            Effect::WritePageIds { visible } => {
                let result =
                    std::fs::write(app.vault.paths.cache.join("tui.toml"), format!("page_ids = {visible}\n")).map_err(|e| e.to_string());
                dispatch(app, Msg::PageIdsPersisted { result });
            }
            Effect::WriteClipboard { text, notice } => {
                let result = set_clipboard(&text);
                dispatch(app, Msg::ClipboardResult { result, notice, at: app.ui.now_ms });
            }
        }
    }
}

/// The wall clock, as the runtime reads it: epoch milliseconds and the local offset from UTC in
/// minutes. `THC_NOW` pins it (fixtures, replay), so a pinned run is the same every time.
pub(crate) fn wall_clock() -> (u64, i32) {
    use chrono::{Local, TimeZone};
    let local = thc_core::dates::now_local();
    let offset = Local.offset_from_local_datetime(&local).earliest().map_or(0, |o| o.local_minus_utc() / 60);
    let utc = local - chrono::Duration::minutes(offset as i64);
    (utc.and_utc().timestamp_millis().max(0) as u64, offset)
}

/// Advance the UI's logical clock to the wall clock (the runtime's tick, before input).
pub(crate) fn tick(app: &mut App) {
    let (now_ms, offset) = wall_clock();
    if now_ms != app.ui.now_ms || offset != app.ui.utc_offset_min {
        app.ui.tick(now_ms, offset);
    }
}

pub(crate) fn toggle_page_ids(app: &mut App) {
    dispatch(app, Msg::TogglePageIds { at: app.ui.now_ms });
}

/// OSC 52 always (it reaches the local clipboard over SSH where the terminal allows); on a local
/// session also the platform tool, which works where OSC 52 is off (Terminal.app, tmux).
fn set_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    if crate::SNAPSHOT.with(|s| s.get()) {
        return Ok(());
    }
    let osc = format!("\x1b]52;c;{}\x07", crate::doc_keys::base64(text.as_bytes()));
    let _ = std::io::stdout().write_all(osc.as_bytes());
    let _ = std::io::stdout().flush();
    if std::env::var_os("SSH_TTY").is_some() || std::env::var_os("SSH_CONNECTION").is_some() {
        return Ok(());
    }
    for (tool, args) in [("pbcopy", &[][..]), ("wl-copy", &[][..]), ("xclip", &["-selection", "clipboard"][..])] {
        if let Ok(mut child) = std::process::Command::new(tool)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
            return Ok(());
        }
    }
    Ok(())
}
