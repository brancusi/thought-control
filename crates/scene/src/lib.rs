//! thc-scene: a terminal UI that is one serializable value.
//!
//! - `model`: the UI as data (a node tree, data sources, keys).
//! - `state`: the screen's state, messages and a pure `update`.
//! - `view`: state in, frame out.
//! - `runtime`: the terminal, the control socket, child processes.
//! - `gfx`: pictures (cards, plots, images) placed with the kitty graphics protocol.

pub mod gfx;
pub mod model;
pub mod runtime;
pub mod state;
pub mod view;
