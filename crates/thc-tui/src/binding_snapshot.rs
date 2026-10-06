//! Binding hints captured by the coordinator. Rendering never consults the global
//! binding table or evaluates store-backed binding predicates.
use crate::{
    app::{App, Overlay},
    keymap::Hint,
};

#[derive(Default)]
pub(crate) struct BindingSnapshot {
    pub footer: Vec<Hint>,
    pub prefix: Option<(String, Vec<Hint>)>,
    pub continuations: Vec<Hint>,
    pub help: Vec<(&'static str, Vec<(String, String)>)>,
}

pub(crate) fn capture(app: &App) -> BindingSnapshot {
    let footer = crate::keymap::footer(app, &crate::keymap::footer_ctxs(app));
    let prefix = crate::keymap::prefix_footer(app);
    let continuations = prefix.as_ref().map(|(_, hints)| hints.clone()).unwrap_or_default();
    let help = match app.overlay {
        Some(Overlay::Help { all, .. }) => crate::keymap::help(app, &crate::keymap::help_ctxs(app, all)),
        _ => vec![],
    };
    BindingSnapshot { footer, prefix, continuations, help }
}
