//! caretline-next: a standalone text-editing engine.
//!
//! The editing model is Helix's (vendored under [`helix`]): a rope, multi-range
//! [`Selection`](helix::Selection)s of anchor and head, every edit a
//! [`Transaction`](helix::Transaction) that maps selections through its changes, an undo
//! [`History`](helix::history::History) tree, grapheme-correct motion, and the
//! `DocumentFormatter` for soft wrap.
//!
//! Around it is a strict Elm architecture:
//!
//! - [`State`] holds everything: text, selection (with the goal column), scroll, viewport,
//!   clipboard register, history, config. It serializes to JSON and back without loss.
//! - [`Msg`] is every input, [`Effect`] every output. Both are plain values.
//! - [`update`] is pure and deterministic: no clock, randomness or I/O. Time arrives in
//!   [`Msg::Tick`].
//! - [`view`] is pure: state in, a cell grid ([`Frame`]) out.
//! - [`keymap`](keymap::keymap) is a pure function from a key to an optional message.
//!
//! So a session is its initial state plus its messages, and replaying them reproduces it
//! exactly ([`trace`]). [`Session`] keeps one (state, revision, trace) and [`protocol`]
//! answers JSON requests against it.

pub mod helix;
pub mod keymap;
pub mod layout;
pub mod marks;
pub mod msg;
pub mod outline;
pub mod protocol;
pub mod session;
pub mod state;
pub mod trace;
pub mod update;
pub mod view;
pub mod views;
pub mod external;

pub use marks::{BlockAttrs, Mark, MarkId, Marks};
pub use outline::{BlockInfo, Kind, NewBlock, Outline, OutlineConfig};
pub use keymap::{keymap, keymap_for, outline_keymap, parse_keys, script_to_msgs, script_to_msgs_for, Key, KeyCode, Mods};
pub use msg::{By, Dir, Effect, Msg};
pub use session::Session;
pub use state::{Config, Document, ExternalUndo, Follow, Scroll, State, View, ViewConfig, Viewport};
pub use update::{replay, update};
pub use views::update_doc;
pub use layout::OutlineLayout;
pub use view::{view, Frame};
