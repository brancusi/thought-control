//! thc-screen: thc's views on fictty, the terminal UI runtime where a UI is one serializable
//! value.
//!
//! - `feeds`: the store (and an explicit now) to the JSON each view binds to. Pure.
//! - `screens`: each view as a fictty `Ui`, built from Rust types; its sources and actions are
//!   the host's.
//! - `host`: `ThcHost`, the vault as a fictty `Host` (feeds in, writes out, change notices).
//! - `theme`: thc's palettes (thc_core::palette) as fictty themes.
//! - `markup`: thc's words as fictty template markup.

pub mod feeds;
pub mod host;
pub mod markup;
pub mod screens;
pub mod theme;

pub use host::ThcHost;
pub use screens::{Route, View};

/// A frame of `route` without a terminal: `width`×`height` cells after `keys`, as text, or as
/// ANSI escapes with `ansi`.
pub fn render(host: &ThcHost, route: &Route, theme: &str, width: u16, height: u16, keys: &[&str], ansi: bool) -> anyhow::Result<String> {
    let mut screen = fictty::App::new(host.screen(route)).themes(theme::themes()).theme(theme).host(host.clone()).headless(width, height)?;
    screen.keys(keys);
    Ok(if ansi { screen.ansi() } else { screen.screen() })
}
