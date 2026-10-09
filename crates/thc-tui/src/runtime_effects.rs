//! Executes value effects and delivers explicit results. IO belongs here, never
//! in update. Additional legacy effects migrate in later P3 steps.
use crate::{
    app::App,
    update::{self, Effect, Fields, Msg},
};

/// Perform one effect: the IO update asked for. Results come back as messages.
fn perform(app: &mut App, effect: Effect) {
    match effect {
        Effect::EditKeys => {
            app.save_doc(true);
            app.overlay = None;
            app.editor_request = Some("@keys".into());
        }
        Effect::WritePageIds { visible } => {
            let result =
                std::fs::write(app.vault.paths.cache.join("tui.toml"), format!("page_ids = {visible}\n")).map_err(|e| e.to_string());
            dispatch(app, Msg::PageIdsPersisted { result });
        }
        Effect::WriteDocumentMode { on } => {
            // A snapshot or test session never writes this device's choice.
            let result = if crate::SNAPSHOT.with(|s| s.get()) {
                Ok(())
            } else {
                std::fs::write(crate::app::document_mode_file(&app.vault.paths.cache), format!("document_mode = {on}\n")).map_err(|e| e.to_string())
            };
            dispatch(app, Msg::DocumentModePersisted { result });
        }
        Effect::Reload => {
            let _ = app.reload();
        }
        Effect::SaveDoc => app.save_doc(true),
        // Ids are the runtime's to mint (the model reads no randomness).
        Effect::MintIds { n } => dispatch(app, Msg::IdsMinted { ids: (0..n).map(|_| thc_core::id::new_id()).collect() }),
        Effect::Reopen => app.patch_doc(),
        Effect::Quit => app.quit = true,
        Effect::SetMouse { on } => app.mouse_request = Some(on),
        Effect::SpawnEditor { target } => app.editor_request = Some(target),
        e @ (Effect::SidebarLoad { .. } | Effect::SidebarDrop { .. } | Effect::SidebarPersist | Effect::SidebarEvicted { .. }) => crate::sidebar_app::perform(app, e),
        Effect::WriteClipboard { text, notice } => {
            let result = set_clipboard(&text);
            dispatch(app, Msg::ClipboardResult { result, notice, at: app.ui.now_ms });
        }
    }
}

/// A keymap action through the pure update (update::action), its effects run here. False:
/// the action isn't one it handles.
pub(crate) fn action(app: &mut App, action: &str) -> bool {
    let facts = update::Facts {
        rows: &app.rows,
        doc_open: app.doc.is_some(),
        mouse: app.tui_prefs.mouse,
        half_page: (app.render.list_height / 2).max(1) as isize,
    };
    let Some(effects) = update::action(&mut app.ui, &facts, action) else { return false };
    run(app, effects);
    true
}

/// Perform effects in order.
pub(crate) fn run(app: &mut App, effects: Vec<Effect>) {
    for effect in effects {
        perform(app, effect);
    }
}

/// A change to the sidebar through its pure update (update::sidebar), its effects run here.
pub(crate) fn sidebar_op(app: &mut App, op: update::SidebarOp) {
    let effects = update::sidebar(&mut app.ui, op);
    run(app, effects);
}

pub(crate) fn dispatch(app: &mut App, msg: Msg) {
    let effects = update::update(
        Fields { page_ids: Some(&mut app.ui.page_ids), document_mode: Some(&mut app.ui.document_mode), cursor: app.ui.cursor, scroll: &mut app.ui.scroll, toast: &mut app.ui.toast, doc: app.doc.as_mut() },
        msg,
    );
    run(app, effects);
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

pub(crate) fn toggle_page_ids(app: &mut App) {
    dispatch(app, Msg::TogglePageIds { at: app.ui.now_ms });
}

thread_local! {
    /// What a snapshot or test session copied (`set_clipboard` never reaches the real one there).
    pub(crate) static SNAPSHOT_CLIPBOARD: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// OSC 52 always (it reaches the local clipboard over SSH where the terminal allows); on a local
/// session also the platform tool, which works where OSC 52 is off (Terminal.app, tmux).
fn set_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    if crate::SNAPSHOT.with(|s| s.get()) {
        // Never the real clipboard; a test reads what would have gone there.
        SNAPSHOT_CLIPBOARD.with(|c| *c.borrow_mut() = Some(text.to_string()));
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
