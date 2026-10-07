//! Helix's editing core, vendored.
//!
//! The files in this directory (except this one and `README.md`) come from
//! [Helix](https://github.com/helix-editor/helix) and stay under the Mozilla Public License 2.0
//! (`LICENSE-MPL-2.0`). Each file names its upstream path, the upstream commit and what was
//! changed. This module file plays the part of `helix-core/src/lib.rs` and `helix-stdx`: it
//! wires the vendored files together and re-exports what they import from the crate root.
#![allow(dead_code, clippy::all)]

pub mod chars;
pub mod doc_formatter;
pub mod graphemes;
pub mod history;
pub mod line_ending;
pub mod movement;
pub mod position;
pub mod selection;
pub mod text_annotations;
pub mod transaction;

#[cfg(test)]
pub mod test;

/// The pieces of `helix-stdx` the vendored files use.
pub mod stdx {
    pub mod range;
    pub mod rope;
    pub use range::Range;
}

/// Stand-in for Helix's syntax module: the formatter and annotations carry an optional
/// highlight, which this crate never sets.
pub mod syntax {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Highlight(pub u32);
}

pub use ropey::{self, str_utils, Rope, RopeBuilder, RopeSlice};
pub use smallvec::{smallvec, SmallVec};
pub use smartstring::SmartString;

pub type Tendril = SmartString<smartstring::LazyCompact>;

pub use position::{
    char_idx_at_visual_offset, coords_at_pos, pos_at_coords, softwrapped_dimensions,
    visual_offset_from_anchor, visual_offset_from_block, Position, VisualOffsetError,
};
#[allow(deprecated)]
pub use position::{pos_at_visual_coords, visual_coords_at_pos};

pub use line_ending::{LineEnding, NATIVE_LINE_ENDING};
pub use selection::{Range, Selection};
pub use transaction::{Assoc, Change, ChangeSet, Deletion, Operation, Transaction};
