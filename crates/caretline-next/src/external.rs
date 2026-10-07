//! Changes from outside the editor (another device, a daemon, an agent): placeholder.

use crate::msg::{Effect, Msg};
use crate::state::{Document, View};

pub(crate) fn apply(_doc: &mut Document, _views: &mut [View], _msg: Msg) -> Vec<Effect> {
    Vec::new()
}
