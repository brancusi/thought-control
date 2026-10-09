//! Document buttons use the same update in every pane; the host supplies the target view.
use crate::{app::{App, PromptKind}, input::LineInput, ui::Click};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

pub(crate) fn target(app: &mut App, target: Option<&Click>, m: MouseEvent) -> bool {
    if m.kind != MouseEventKind::Down(MouseButton::Left) { return false; }
    match target {
        Some(Click::Meta { line, field }) => {
            let Some(l) = app.doc.as_ref().and_then(|d| d.blocks().get(*line)) else { return true };
            let id = l.id.clone();
            match *field {
                "open" => app.doc_open(),
                "conflict" => { app.selected = Some(id); app.open_compare(); }
                _ => {
                    app.save_doc(true);
                    if let Some(n) = app.vault.store.node(&id).ok().flatten() {
                        let (kind, cur) = if *field == "due" { (PromptKind::Due(id.clone()), n.due.clone()) } else { (PromptKind::Sched(id.clone()), n.scheduled.clone()) };
                        let human = cur.as_deref().map(|c| crate::input::human_date(c, app.today)).unwrap_or_default();
                        app.prompt = Some((kind, LineInput::with(&human)));
                    }
                }
            }
            true
        }
        Some(Click::Scroll(i)) => {
            if let (Some((_, h, _, ty, th)), Some(d)) = (app.render.doc_scrollbar, app.doc.as_mut()) {
                if *i >= ty && *i < ty + th { app.scroll_drag = true; }
                else if *i < ty { d.set_scroll(d.scroll().saturating_sub(h as usize), true); }
                else { d.set_scroll(d.scroll() + h as usize, true); }
            }
            true
        }
        Some(Click::LinkRow(i)) => {
            if app.main.link_open {
                app.main.link_sel = Some(*i);
                super::update::handle(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            }
            true
        }
        Some(Click::Text) => true,
        _ => false,
    }
}

pub(crate) fn scroll_drag(app: &mut App, m: MouseEvent) {
    if let (Some((top, h, total, _, _)), Some(d)) = (app.render.doc_scrollbar, app.doc.as_mut()) {
        d.set_scroll((m.row.saturating_sub(top) as usize * total / h.max(1) as usize).min(total.saturating_sub(1)), true);
    }
}
