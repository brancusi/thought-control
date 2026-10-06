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
        caret: doc.view.caret,
    })
}
pub(crate) fn dispatch(app: &mut App, msg: Msg) {
    let identity = document_identity(app);
    let document = identity.zip(app.doc.as_mut()).map(|(identity, doc)| DocumentFields { identity, doc });
    let effects = update::update(
        Fields { page_ids: Some(&mut app.page_ids), cursor: app.cursor, scroll: &mut app.scroll, document, toast: &mut app.toast },
        msg,
    );
    for effect in effects {
        match effect {
            Effect::WritePageIds { visible } => {
                let result =
                    std::fs::write(app.vault.paths.cache.join("tui.toml"), format!("page_ids = {visible}\n")).map_err(|e| e.to_string());
                dispatch(app, Msg::PageIdsPersisted { result });
            }
            Effect::WriteClipboard { text, notice } => {
                let result = set_clipboard(&text);
                dispatch(app, Msg::ClipboardResult { result, notice, at: std::time::Instant::now() });
            }
            Effect::Save { all } => app.save_doc(all),
            Effect::Patch => app.patch_doc(),
        }
    }
}

/// The text column per depth at the current layout, for an edit message.
pub(crate) fn widths(app: &App) -> update::Widths {
    let ctx = crate::doc_ui::DocContext::from_app(app);
    update::Widths((0..=32).map(|depth| crate::doc_ui::text_width(ctx, app.screen_width, app.show_detail, depth)).collect())
}

/// An editing or motion command at the caret, as a message: its time and the ids of the lines
/// it creates are taken here, once, so the update replays.
pub(crate) fn edit(app: &mut App, cmd: caretline::Command) {
    let widths = widths(app);
    dispatch(app, Msg::Edit { cmd, at: std::time::Instant::now(), seed: thc_core::id::new_id(), widths });
}

/// Text typed at the caret, as a message.
pub(crate) fn type_text(app: &mut App, text: &str) {
    dispatch(app, Msg::Type { text: text.to_string(), at: std::time::Instant::now(), seed: thc_core::id::new_id() });
}

pub(crate) fn toggle_page_ids(app: &mut App) {
    dispatch(app, Msg::TogglePageIds { at: std::time::Instant::now() });
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
