//! Runtime configuration/registry inputs captured before rendering.
use crate::{
    app::{App, Overlay, View},
    theme::Accent,
};
use std::{collections::HashMap, path::PathBuf};

#[derive(Default)]
pub(crate) struct PresentationSnapshot {
    pub short_name: Option<String>,
    pub scope_current: String,
    pub scope_default: String,
    pub scope_current_text: String,
    pub scope_default_text: String,
    pub scope_choices: Vec<String>,
    pub spans_vaults: bool,
    pub vault_width: usize,
    pub vault_labels: HashMap<Option<usize>, (String, Accent)>,
    pub vault_styles: HashMap<String, (Option<String>, Accent)>,
    pub history_styles: HashMap<PathBuf, (String, Accent)>,
}

pub(crate) fn capture(app: &App) -> PresentationSnapshot {
    let short_name = thc_core::settings::current().str("vault.name_short").map(str::to_owned).filter(|s| !s.is_empty());
    let mut out = PresentationSnapshot { short_name: short_name.clone(), ..Default::default() };
    out.vault_labels.insert(None, (short_name.unwrap_or_else(|| app.vault_name.clone()), app.theme.accent));
    for (i, other) in app.others.iter().enumerate() {
        out.vault_labels.insert(Some(i), (other.name.clone(), other.accent));
    }
    out.vault_width =
        out.vault_labels.values().map(|(label, _)| unicode_width::UnicodeWidthStr::width(label.as_str())).max().unwrap_or(0).min(12);
    if app.view == View::Today || matches!(app.overlay, Some(Overlay::Scope { .. } | Overlay::Recipe { .. })) {
        out.scope_default = app.scope_default();
        out.scope_current = app.scope_override.get("today").cloned().unwrap_or_else(|| out.scope_default.clone());
        out.scope_current_text = app.scope_text(&out.scope_current);
        out.scope_default_text = app.scope_text(&out.scope_default);
        out.spans_vaults = app.view == View::Today && !app.others.is_empty() && app.scope_vaults().len() > 1;
    }
    if matches!(app.overlay, Some(Overlay::Scope { .. })) {
        out.scope_choices = app.scope_choices();
        let reg = thc_core::registry::Registry::load();
        for name in &out.scope_choices {
            let settings = reg.find(name).map(|e| thc_core::settings::load(Some(&e.path)));
            let accent = Accent::parse(settings.as_ref().map_or("ember", |s| s.accent()));
            let short = settings.as_ref().and_then(|s| s.str("vault.name_short")).map(str::to_owned).filter(|s| !s.is_empty() && s != name);
            out.vault_styles.insert(name.clone(), (short, accent));
        }
    }
    if matches!(app.overlay, Some(Overlay::History { .. })) {
        let reg = thc_core::registry::Registry::load();
        for place in &app.history.entries {
            out.history_styles.entry(place.vault.clone()).or_insert_with(|| {
                let name = reg.by_path(&place.vault).map(|e| e.name.clone()).unwrap_or_else(|| thc_core::vault::tilde(&place.vault));
                let accent = Accent::parse(thc_core::settings::load(Some(&place.vault)).accent());
                (name, accent)
            });
        }
    }
    out
}
