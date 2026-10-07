//! Store-backed overlay choices captured outside the view.
use crate::app::{App, Go, MoveItem, Overlay, PaletteEntry, Recipe};

#[derive(Default)]
pub(crate) struct OverlaySnapshot {
    pub releases: Vec<crate::about::Release>,
    pub recipe: Option<Recipe>,
    pub finder: Vec<(String, Go)>,
    pub moves: Vec<Option<MoveItem>>,
    pub palette_matches: Vec<PaletteEntry>,
    pub palette_entries: Vec<PaletteEntry>,
    /// The `[[` popup's rows for the query typed: (query, matches, the creation row).
    pub link: Option<(String, Vec<(String, String)>, Option<String>)>,
}

pub(crate) fn capture(app: &App) -> OverlaySnapshot {
    let mut out = OverlaySnapshot::default();
    if let Some((_, q)) = app.link_query() {
        let (m, create) = app.link_matches(&q);
        out.link = Some((q, m, create));
    }
    match &app.overlay {
        Some(Overlay::About(_)) => out.releases = crate::about::releases().to_vec(),
        Some(Overlay::Recipe { name }) => out.recipe = app.recipe(name),
        Some(Overlay::Finder { input, .. }) => out.finder = app.finder_matches(&input.buf),
        Some(Overlay::Move { input, .. }) => out.moves = app.move_items(&input.buf),
        Some(Overlay::Palette { input, .. }) => {
            out.palette_matches = crate::input::palette_matches(app, &input.buf);
            out.palette_entries = app.palette_entries();
        }
        _ => {}
    }
    out
}
