//! The editing vocabulary: every generic editing command with a stable id, a name, a short
//! description and a category, each one message ([`command_msg`]), and the default keymap as
//! data ([`default_keymap`]): key chord → command id.
//!
//! caretline owns the vocabulary and a default; an app owns its final bindings. A host takes the
//! table, overrides entries or ignores it, adds its own commands (registered with
//! [`crate::Host::command`]) under its own ids, and builds its help from [`commands`], so help
//! never drifts from what the keys do. [`crate::keymap()`] is a lookup in this table.
//!
//! Ids are `category.verb` and never change within a major version.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Serialize;

use crate::keymap::{parse_keys, Key, KeyCode, Mods, ScriptItem};
use crate::marks::MarkId;
use crate::msg::{By, Dir, Msg};

/// What a command is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Move,
    Select,
    Edit,
    Clipboard,
    History,
    Structure,
    View,
    File,
}

impl Category {
    pub fn name(self) -> &'static str {
        match self {
            Category::Move => "Move",
            Category::Select => "Select",
            Category::Edit => "Edit",
            Category::Clipboard => "Clipboard",
            Category::History => "History",
            Category::Structure => "Structure",
            Category::View => "View",
            Category::File => "File",
        }
    }
}

/// One editing command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CommandInfo {
    /// Stable: `move.word_right`, `select.all`, `history.undo`.
    pub id: &'static str,
    /// For menus and help: `Word right`.
    pub name: &'static str,
    pub description: &'static str,
    pub category: Category,
    /// Meaningful only in an outline document (elsewhere it says so, or acts on lines).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub outline_only: bool,
    /// Acts on one block, which the host names ([`command_msg`]'s `block`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub takes_block: bool,
}

/// Which keyboards a binding is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    /// Every keyboard and terminal.
    Any,
    /// A Command (⌘) chord: macOS, in a terminal that forwards ⌘ (the kitty keyboard
    /// protocol's "super"). Each has a Ctrl or Alt twin for terminals that don't.
    Mac,
}

/// One default binding: a key chord in key-script notation (`<c-s-z>`, `<a-left>`, `<d-a>`,
/// see [`parse_keys`]) and the command it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Binding {
    pub keys: &'static str,
    pub command: &'static str,
    pub platform: Platform,
    /// Only in an outline document's keymap (on top of the plain one).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub outline: bool,
}

macro_rules! cmd {
    ($id:literal, $name:literal, $cat:ident, $desc:literal) => {
        CommandInfo { id: $id, name: $name, description: $desc, category: Category::$cat, outline_only: false, takes_block: false }
    };
    ($id:literal, $name:literal, $cat:ident, $desc:literal, outline) => {
        CommandInfo { id: $id, name: $name, description: $desc, category: Category::$cat, outline_only: true, takes_block: false }
    };
    ($id:literal, $name:literal, $cat:ident, $desc:literal, block) => {
        CommandInfo { id: $id, name: $name, description: $desc, category: Category::$cat, outline_only: true, takes_block: true }
    };
}

const COMMANDS: &[CommandInfo] = &[
    cmd!("move.left", "Left", Move, "Move left one character"),
    cmd!("move.right", "Right", Move, "Move right one character"),
    cmd!("move.up", "Up", Move, "Move up one row"),
    cmd!("move.down", "Down", Move, "Move down one row"),
    cmd!("move.word_left", "Word left", Move, "Move to the start of the previous word"),
    cmd!("move.word_right", "Word right", Move, "Move to the end of the next word"),
    cmd!("move.line_start", "Line start", Move, "Move to the start of the row"),
    cmd!("move.line_end", "Line end", Move, "Move to the end of the row"),
    cmd!("move.doc_start", "Document start", Move, "Move to the start of the document"),
    cmd!("move.doc_end", "Document end", Move, "Move to the end of the document"),
    cmd!("move.page_up", "Page up", Move, "Move up a screenful"),
    cmd!("move.page_down", "Page down", Move, "Move down a screenful"),
    cmd!("move.block_up", "Block up", Move, "Move to the start of this block, then the one before"),
    cmd!("move.block_down", "Block down", Move, "Move to the start of the next block"),
    cmd!("select.left", "Select left", Select, "Extend the selection left one character"),
    cmd!("select.right", "Select right", Select, "Extend the selection right one character"),
    cmd!("select.up", "Select up", Select, "Extend the selection up one row"),
    cmd!("select.down", "Select down", Select, "Extend the selection down one row"),
    cmd!("select.word_left", "Select word left", Select, "Extend the selection to the previous word start"),
    cmd!("select.word_right", "Select word right", Select, "Extend the selection to the next word end"),
    cmd!("select.line_start", "Select to line start", Select, "Extend the selection to the start of the row"),
    cmd!("select.line_end", "Select to line end", Select, "Extend the selection to the end of the row"),
    cmd!("select.doc_start", "Select to document start", Select, "Extend the selection to the start of the document"),
    cmd!("select.doc_end", "Select to document end", Select, "Extend the selection to the end of the document"),
    cmd!("select.page_up", "Select page up", Select, "Extend the selection up a screenful"),
    cmd!("select.page_down", "Select page down", Select, "Extend the selection down a screenful"),
    cmd!("select.block_up", "Select block up", Select, "Extend the selection to the previous block start"),
    cmd!("select.block_down", "Select block down", Select, "Extend the selection to the next block start"),
    cmd!("select.all", "Select all", Select, "Select the whole document"),
    cmd!("select.collapse", "Collapse selection", Select, "Collapse every selection to its caret"),
    cmd!("select.block", "Select block", Select, "Select a block's whole content", block),
    cmd!("edit.newline", "New line", Edit, "Break the line (in a list, start the next item)"),
    cmd!("edit.soft_break", "Line break", Edit, "Break the line inside the block", outline),
    cmd!("edit.insert_tab", "Tab", Edit, "Insert a tab character"),
    cmd!("edit.backspace", "Backspace", Edit, "Delete the character before the caret, or the selection"),
    cmd!("edit.delete_forward", "Delete", Edit, "Delete the character after the caret, or the selection"),
    cmd!("edit.delete_word", "Delete word", Edit, "Delete back to the previous word start"),
    cmd!("edit.delete_word_forward", "Delete word forward", Edit, "Delete forward to the next word end"),
    cmd!("edit.delete_to_line_start", "Delete to line start", Edit, "Delete back to the start of the row"),
    cmd!("edit.delete_to_line_end", "Delete to line end", Edit, "Delete forward to the end of the row"),
    cmd!("edit.kill_line", "Kill line", Edit, "Delete to the end of the line, or the line break when there"),
    cmd!("structure.indent", "Indent", Structure, "Nest the block (or every selected block) one level deeper", outline),
    cmd!("structure.outdent", "Outdent", Structure, "Un-nest the block (or every selected block) one level", outline),
    cmd!("structure.move_up", "Move block up", Structure, "Swap the block and its children with the sibling above", outline),
    cmd!("structure.move_down", "Move block down", Structure, "Swap the block and its children with the sibling below", outline),
    cmd!("clip.copy", "Copy", Clipboard, "Copy the selection"),
    cmd!("clip.cut", "Cut", Clipboard, "Cut the selection"),
    cmd!("clip.paste", "Paste", Clipboard, "Paste (Markdown becomes blocks in an outline)"),
    cmd!("clip.paste_plain", "Paste as plain text", Clipboard, "Paste as plain paragraphs, never list items"),
    cmd!("history.undo", "Undo", History, "Undo the last step"),
    cmd!("history.redo", "Redo", History, "Redo the step undone"),
    cmd!("view.fold", "Fold", View, "Hide a block's children in this view", block),
    cmd!("view.unfold", "Unfold", View, "Show a folded block's children", block),
    cmd!("view.fold_toggle", "Toggle fold", View, "Fold or unfold a block", block),
    cmd!("file.save", "Save", File, "Write the document to its file"),
    cmd!("file.quit", "Quit", File, "Quit (a second time to discard unsaved changes)"),
];

/// Every editing command, in display order.
pub fn commands() -> &'static [CommandInfo] {
    COMMANDS
}

/// The command with this id.
pub fn command(id: &str) -> Option<&'static CommandInfo> {
    COMMANDS.iter().find(|c| c.id == id)
}

/// The message a command sends. `block` names the block for the commands that take one (none
/// without it).
pub fn command_msg(id: &str, block: Option<MarkId>) -> Option<Msg> {
    use Dir::{Backward as B, Forward as F};
    let mv = |dir, by, extend| Some(Msg::Move { dir, by, extend });
    let (verb, extend) = match id.split_once('.') {
        Some(("move", v)) => (v, false),
        Some(("select", v)) if !matches!(v, "all" | "collapse" | "block") => (v, true),
        _ => ("", false),
    };
    let motion = match verb {
        "left" => mv(B, By::Grapheme, extend),
        "right" => mv(F, By::Grapheme, extend),
        "up" => mv(B, By::VisualLine, extend),
        "down" => mv(F, By::VisualLine, extend),
        "word_left" => mv(B, By::Word, extend),
        "word_right" => mv(F, By::Word, extend),
        "line_start" => mv(B, By::LineStart, extend),
        "line_end" => mv(F, By::LineEnd, extend),
        "doc_start" => mv(B, By::DocStart, extend),
        "doc_end" => mv(F, By::DocEnd, extend),
        "page_up" => mv(B, By::Page, extend),
        "page_down" => mv(F, By::Page, extend),
        "block_up" => mv(B, By::Block, extend),
        "block_down" => mv(F, By::Block, extend),
        _ => None,
    };
    if motion.is_some() {
        return motion;
    }
    Some(match id {
        "select.all" => Msg::SelectAll,
        "select.collapse" => Msg::Collapse,
        "select.block" => Msg::SelectBlock { id: block? },
        "edit.newline" => Msg::InsertNewline,
        "edit.soft_break" => Msg::SoftBreak,
        "edit.insert_tab" => Msg::InsertText { text: "\t".into() },
        "edit.backspace" => Msg::DeleteBackward,
        "edit.delete_forward" => Msg::DeleteForward,
        "edit.delete_word" => Msg::DeleteWordBackward,
        "edit.delete_word_forward" => Msg::DeleteWordForward,
        "edit.delete_to_line_start" => Msg::DeleteToLineStart,
        "edit.delete_to_line_end" => Msg::DeleteToLineEnd,
        "edit.kill_line" => Msg::KillLine,
        "structure.indent" => Msg::Indent,
        "structure.outdent" => Msg::Outdent,
        "structure.move_up" => Msg::MoveBlock { dir: B },
        "structure.move_down" => Msg::MoveBlock { dir: F },
        "clip.copy" => Msg::Copy,
        "clip.cut" => Msg::Cut,
        "clip.paste" => Msg::Paste { text: None },
        "clip.paste_plain" => Msg::PastePlain { text: None },
        "history.undo" => Msg::Undo,
        "history.redo" => Msg::Redo,
        "view.fold" => Msg::Fold { id: block? },
        "view.unfold" => Msg::Unfold { id: block? },
        "view.fold_toggle" => Msg::ToggleFold { id: block? },
        "file.save" => Msg::Save,
        "file.quit" => Msg::Quit,
        _ => return None,
    })
}

macro_rules! bind {
    ($keys:literal, $cmd:literal) => {
        Binding { keys: $keys, command: $cmd, platform: Platform::Any, outline: false }
    };
    ($keys:literal, $cmd:literal, mac) => {
        Binding { keys: $keys, command: $cmd, platform: Platform::Mac, outline: false }
    };
    ($keys:literal, $cmd:literal, outline) => {
        Binding { keys: $keys, command: $cmd, platform: Platform::Any, outline: true }
    };
}

/// The default keymap: macOS text-field keys first, with Ctrl and Alt twins for terminals
/// that don't forward ⌘. Printable keys without Ctrl, Alt or ⌘ type themselves.
const BINDINGS: &[Binding] = &[
    bind!("<left>", "move.left"),
    bind!("<right>", "move.right"),
    bind!("<up>", "move.up"),
    bind!("<down>", "move.down"),
    bind!("<a-left>", "move.word_left"),
    bind!("<c-left>", "move.word_left"),
    bind!("<a-b>", "move.word_left"),
    bind!("<a-right>", "move.word_right"),
    bind!("<c-right>", "move.word_right"),
    bind!("<a-f>", "move.word_right"),
    bind!("<d-left>", "move.line_start", mac),
    bind!("<home>", "move.line_start"),
    bind!("<c-a>", "move.line_start"),
    bind!("<d-right>", "move.line_end", mac),
    bind!("<end>", "move.line_end"),
    bind!("<c-e>", "move.line_end"),
    bind!("<d-up>", "move.doc_start", mac),
    bind!("<c-home>", "move.doc_start"),
    bind!("<d-home>", "move.doc_start", mac),
    bind!("<d-down>", "move.doc_end", mac),
    bind!("<c-end>", "move.doc_end"),
    bind!("<d-end>", "move.doc_end", mac),
    bind!("<pgup>", "move.page_up"),
    bind!("<pgdn>", "move.page_down"),
    bind!("<c-up>", "move.block_up", outline),
    bind!("<c-down>", "move.block_down", outline),
    bind!("<s-left>", "select.left"),
    bind!("<s-right>", "select.right"),
    bind!("<s-up>", "select.up"),
    bind!("<s-down>", "select.down"),
    bind!("<a-s-left>", "select.word_left"),
    bind!("<c-s-left>", "select.word_left"),
    bind!("<a-s-b>", "select.word_left"),
    bind!("<a-s-right>", "select.word_right"),
    bind!("<c-s-right>", "select.word_right"),
    bind!("<a-s-f>", "select.word_right"),
    bind!("<d-s-left>", "select.line_start", mac),
    bind!("<s-home>", "select.line_start"),
    bind!("<c-s-a>", "select.line_start"),
    bind!("<d-s-right>", "select.line_end", mac),
    bind!("<s-end>", "select.line_end"),
    bind!("<c-s-e>", "select.line_end"),
    bind!("<d-s-up>", "select.doc_start", mac),
    bind!("<c-s-home>", "select.doc_start"),
    bind!("<d-s-home>", "select.doc_start", mac),
    bind!("<d-s-down>", "select.doc_end", mac),
    bind!("<c-s-end>", "select.doc_end"),
    bind!("<d-s-end>", "select.doc_end", mac),
    bind!("<s-pgup>", "select.page_up"),
    bind!("<s-pgdn>", "select.page_down"),
    bind!("<c-s-up>", "select.block_up", outline),
    bind!("<c-s-down>", "select.block_down", outline),
    bind!("<d-a>", "select.all", mac),
    bind!("<a-a>", "select.all"),
    bind!("<esc>", "select.collapse"),
    bind!("<s-esc>", "select.collapse"),
    bind!("<cr>", "edit.newline"),
    bind!("<s-cr>", "edit.newline"),
    bind!("<s-cr>", "edit.soft_break", outline),
    bind!("<c-j>", "edit.soft_break", outline),
    bind!("<tab>", "edit.insert_tab"),
    bind!("<bs>", "edit.backspace"),
    bind!("<s-bs>", "edit.backspace"),
    bind!("<c-h>", "edit.backspace"),
    bind!("<del>", "edit.delete_forward"),
    bind!("<s-del>", "edit.delete_forward"),
    bind!("<a-bs>", "edit.delete_word"),
    bind!("<c-bs>", "edit.delete_word"),
    bind!("<c-w>", "edit.delete_word"),
    bind!("<a-del>", "edit.delete_word_forward"),
    bind!("<c-del>", "edit.delete_word_forward"),
    bind!("<a-d>", "edit.delete_word_forward"),
    bind!("<d-bs>", "edit.delete_to_line_start", mac),
    bind!("<c-u>", "edit.delete_to_line_start"),
    bind!("<d-del>", "edit.delete_to_line_end", mac),
    bind!("<c-k>", "edit.kill_line"),
    bind!("<tab>", "structure.indent", outline),
    bind!("<s-tab>", "structure.outdent", outline),
    bind!("<a-up>", "structure.move_up", outline),
    bind!("<a-down>", "structure.move_down", outline),
    bind!("<d-c>", "clip.copy", mac),
    bind!("<c-c>", "clip.copy"),
    bind!("<d-x>", "clip.cut", mac),
    bind!("<c-x>", "clip.cut"),
    bind!("<d-v>", "clip.paste", mac),
    bind!("<c-v>", "clip.paste"),
    bind!("<a-v>", "clip.paste_plain", outline),
    bind!("<d-z>", "history.undo", mac),
    bind!("<c-z>", "history.undo"),
    bind!("<d-s-z>", "history.redo", mac),
    bind!("<c-s-z>", "history.redo"),
    bind!("<d-y>", "history.redo", mac),
    bind!("<c-y>", "history.redo"),
    bind!("<d-r>", "history.redo", mac),
    bind!("<c-r>", "history.redo"),
    bind!("<d-s>", "file.save", mac),
    bind!("<c-s>", "file.save"),
    bind!("<d-q>", "file.quit", mac),
    bind!("<c-q>", "file.quit"),
];

/// The default bindings: the plain keymap, and with `outline` an outline document's (its
/// bindings replace a plain one on the same keys).
pub fn default_keymap(outline: bool) -> Vec<Binding> {
    let outline_keys: Vec<&str> = BINDINGS.iter().filter(|b| b.outline).map(|b| b.keys).collect();
    BINDINGS
        .iter()
        .filter(|b| if outline { b.outline || !outline_keys.contains(&b.keys) } else { !b.outline })
        .copied()
        .collect()
}

/// A key in the table's notation: modifiers in the order `c-`, `a-`, `d-`, `s-`, then the key.
/// Letters are lowercase (⇧ is the `s-`).
pub fn key_notation(key: &Key) -> String {
    let mut mods = key.mods;
    let name = match key.code {
        KeyCode::Char(c) => {
            if c.is_ascii_uppercase() {
                mods.shift = true;
            }
            match c.to_ascii_lowercase() {
                ' ' => "space".to_string(),
                '<' => "lt".to_string(),
                '>' => "gt".to_string(),
                c => c.to_string(),
            }
        }
        KeyCode::Enter => "cr".into(),
        KeyCode::Backspace => "bs".into(),
        KeyCode::Delete => "del".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pgup".into(),
        KeyCode::PageDown => "pgdn".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::BackTab => {
            mods.shift = true;
            "tab".into()
        }
        KeyCode::Esc => "esc".into(),
    };
    let mut s = String::from("<");
    for (on, p) in [(mods.ctrl, "c-"), (mods.alt, "a-"), (mods.cmd, "d-"), (mods.shift, "s-")] {
        if on {
            s.push_str(p);
        }
    }
    s.push_str(&name);
    s.push('>');
    s
}

/// The key a binding's chord names.
pub fn binding_key(b: &Binding) -> Option<Key> {
    match parse_keys(b.keys).ok()?.as_slice() {
        [ScriptItem::Key(k)] => Some(*k),
        _ => None,
    }
}

fn table(outline: bool) -> &'static HashMap<String, &'static str> {
    static PLAIN: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    static OUTLINE: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    let cell = if outline { &OUTLINE } else { &PLAIN };
    cell.get_or_init(|| {
        default_keymap(outline)
            .into_iter()
            .filter_map(|b| {
                let key = binding_key(&b)?;
                let cmd = COMMANDS.iter().find(|c| c.id == b.command)?.id;
                Some((key_notation(&key), cmd))
            })
            .collect()
    })
}

/// The command a key runs in the default keymap, if any.
pub fn command_for(outline: bool, key: &Key) -> Option<&'static str> {
    table(outline).get(&key_notation(key)).copied()
}

/// [`crate::keymap()`]'s lookup: the table, else a printable key without Ctrl, Alt or ⌘ types
/// itself.
pub(crate) fn lookup(outline: bool, key: &Key) -> Option<Msg> {
    if let Some(id) = command_for(outline, key) {
        return command_msg(id, None);
    }
    match key.code {
        KeyCode::Char(c) if !(key.mods.ctrl || key.mods.alt || key.mods.cmd) => {
            let c = if key.mods.shift { c.to_uppercase().next().unwrap_or(c) } else { c };
            Some(Msg::InsertText { text: c.to_string() })
        }
        _ => None,
    }
}

impl Mods {
    /// No modifier.
    pub fn none() -> Mods {
        Mods::default()
    }
}
