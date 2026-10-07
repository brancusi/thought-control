//! What a host adds to the engine: named **commands**, **input rules** and a **decorator**.
//!
//! caretline edits text; what the text *means* is the host's. A host that wants a key to do
//! something only it understands (turn a line into a task, cycle a status) registers a
//! command: a pure function from the document, the view and some JSON arguments to an
//! [`Edit`]. [`Msg::Command`] runs it as one transaction and one undo step, so it is recorded
//! in traces and replays wherever the same commands are registered. An input rule may take an
//! editing message before the engine does (a shorthand typed at a line's start). A decorator
//! says what to draw in a block's hang and gutter (see [`Decoration`]).
//!
//! Every extension is a pure function: no clock, no randomness, no I/O. The engine never
//! serializes code. A [`Host`] lives on the [`Document`] ([`Document::set_host`]), is shared by
//! its views, compares equal to any other host and is never part of the state's JSON: a state
//! read from JSON has no host until one is set.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::helix::{Assoc, Range, RopeSlice, Selection, Tendril, Transaction};
use crate::marks::{MarkAttrs, MarkId};
use crate::msg::{Effect, Msg};
use crate::outline::{BlockInfo, Outline};
use crate::state::{Document, State, View};
use crate::update::{self, Step};

/// A host command: (document and view, arguments) to an edit, or why it can't run.
pub type CommandFn = dyn Fn(&Ctx, &Value) -> Result<Edit, String> + Send + Sync;
/// An input rule: an editing message to the edit that replaces it, or `None` to let it pass.
pub type InputRuleFn = dyn Fn(&Ctx, &Msg) -> Option<Edit> + Send + Sync;
/// A decorator: what to draw beside a block.
pub type DecoratorFn = dyn Fn(&Ctx, &BlockInfo) -> Decoration + Send + Sync;

/// The extensions a host registers. Cheap to clone (shared).
#[derive(Clone, Default)]
pub struct Host {
    inner: Arc<Inner>,
}

#[derive(Clone, Default)]
struct Inner {
    commands: BTreeMap<String, Arc<CommandFn>>,
    input_rules: Vec<(String, Arc<InputRuleFn>)>,
    decorator: Option<Arc<DecoratorFn>>,
}

impl Host {
    pub fn new() -> Host {
        Host::default()
    }

    /// Registers command `name` (a later registration of the same name replaces it).
    pub fn command(mut self, name: &str, f: impl Fn(&Ctx, &Value) -> Result<Edit, String> + Send + Sync + 'static) -> Host {
        Arc::make_mut(&mut self.inner).commands.insert(name.to_string(), Arc::new(f));
        self
    }

    /// Adds an input rule. Rules run in the order they were added; the first to return an
    /// edit takes the message.
    pub fn input_rule(mut self, name: &str, f: impl Fn(&Ctx, &Msg) -> Option<Edit> + Send + Sync + 'static) -> Host {
        Arc::make_mut(&mut self.inner).input_rules.push((name.to_string(), Arc::new(f)));
        self
    }

    /// Sets the decorator (replacing any before it).
    pub fn decorator(mut self, f: impl Fn(&Ctx, &BlockInfo) -> Decoration + Send + Sync + 'static) -> Host {
        Arc::make_mut(&mut self.inner).decorator = Some(Arc::new(f));
        self
    }

    /// The registered command names, sorted.
    pub fn command_names(&self) -> Vec<&str> {
        self.inner.commands.keys().map(String::as_str).collect()
    }

    /// The input rules' names, in order.
    pub fn input_rule_names(&self) -> Vec<&str> {
        self.inner.input_rules.iter().map(|(n, _)| n.as_str()).collect()
    }

    pub fn has_decorator(&self) -> bool {
        self.inner.decorator.is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.commands.is_empty() && self.inner.input_rules.is_empty() && self.inner.decorator.is_none()
    }

    /// The decoration of `block`, from the decorator (none without one).
    pub fn decorate(&self, ctx: &Ctx, block: &BlockInfo) -> Option<Decoration> {
        self.inner.decorator.as_ref().map(|f| f(ctx, block))
    }
}

impl PartialEq for Host {
    /// Not part of the state's value: any two hosts are equal.
    fn eq(&self, _: &Host) -> bool {
        true
    }
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host")
            .field("commands", &self.command_names())
            .field("input_rules", &self.input_rule_names())
            .field("decorator", &self.has_decorator())
            .finish()
    }
}

/// What an extension sees: the document and the view it acts through.
pub struct Ctx<'a> {
    pub doc: &'a Document,
    pub view: &'a View,
}

impl<'a> Ctx<'a> {
    pub fn new(doc: &'a Document, view: &'a View) -> Ctx<'a> {
        Ctx { doc, view }
    }

    pub fn text(&self) -> RopeSlice<'a> {
        self.doc.text.slice(..)
    }

    /// The blocks, in an outline document.
    pub fn blocks(&self) -> Option<Arc<Outline>> {
        self.doc.blocks()
    }

    pub fn selection(&self) -> &'a Selection {
        &self.view.selection
    }

    /// The primary caret.
    pub fn caret(&self) -> usize {
        self.view.caret()
    }

    /// The line ending Enter inserts.
    pub fn line_ending(&self) -> &'static str {
        self.doc.config.line_ending.as_str()
    }

    /// The current selection mapped through `changes` (sorted, in current positions):
    /// positions at an insertion move past it.
    pub fn mapped_selection(&self, changes: &[(usize, usize, String)]) -> Selection {
        let txn = Transaction::change(&self.doc.text, changes.iter().map(|(a, b, t)| (*a, *b, (!t.is_empty()).then(|| Tendril::from(t.as_str())))));
        let cs = txn.changes();
        self.view.selection.clone().transform(|r| Range {
            anchor: cs.map_pos(r.anchor, Assoc::After),
            head: cs.map_pos(r.head, Assoc::After),
            old_visual_position: None,
        })
    }
}

/// A change to the marks, made with an [`Edit`] (positions in the text after its changes).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MarkOp {
    /// A new mark at the start of the line holding `pos` (a line that already has one keeps
    /// it, as it is).
    Mint {
        pos: usize,
        #[serde(default)]
        attrs: MarkAttrs,
    },
    Remove { id: MarkId },
    /// A mark's blank row before its block.
    SetGap {
        id: MarkId,
        #[serde(default)]
        gap: Option<bool>,
    },
    /// A mark's payload.
    SetData {
        id: MarkId,
        #[serde(default)]
        data: Option<Value>,
    },
}

/// What a host command or input rule does: one transaction and one undo step.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Edit {
    /// `[from, to)` replaced by text, in chars of the current text, sorted and apart.
    pub changes: Vec<(usize, usize, String)>,
    /// The selection after, in the new text. `None`: the selection mapped through the changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<Selection>,
    /// Mark changes, applied after the text changes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<MarkOp>,
    /// A one-line message for the view's status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Effects for the host's runtime, returned from `update` as [`Effect::Host`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<(String, Value)>,
    /// Every block keeps its blank row (a change of shape never moves another block).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub keep_gaps: bool,
}

impl Edit {
    /// An edit that only says something.
    pub fn status(text: impl Into<String>) -> Edit {
        Edit { status: Some(text.into()), ..Edit::default() }
    }

    /// Whether it changes neither text nor marks.
    pub fn is_noop(&self) -> bool {
        self.changes.is_empty() && self.marks.is_empty()
    }
}

/// One drawn decoration: text in a slot beside a block, styled by a role the host names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deco {
    /// Drawn from the slot's left edge, clipped to its width.
    pub text: String,
    /// A style name the host defines (`"box.done"`). The renderer reports it per cell; colours
    /// stay with the host.
    pub role: String,
    /// Reported by hit-testing when the slot is clicked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// What to draw beside a block in an outline layout: in its hang (the columns before its
/// content) and its gutter (the columns before everything).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decoration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hang: Option<Deco>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gutter: Option<Deco>,
}

/// Runs `Msg::Command`.
pub(crate) fn run_command(state: &mut State, name: &str, args: &Value) -> Vec<Effect> {
    let host = state.doc.host.clone();
    let Some(f) = host.inner.commands.get(name).cloned() else {
        state.view.status = Some(format!("no command '{name}'"));
        return Vec::new();
    };
    let result = f(&Ctx::new(&state.doc, &state.view), args);
    match result {
        Ok(edit) => apply(state, edit),
        Err(why) => {
            state.view.status = Some(why);
            Vec::new()
        }
    }
}

/// The first input rule that takes `msg`, applied. `None`: no rule took it.
pub(crate) fn input_rules(state: &mut State, msg: &Msg) -> Option<Vec<Effect>> {
    if state.doc.host.inner.input_rules.is_empty() || !takes_input(msg) {
        return None;
    }
    let host = state.doc.host.clone();
    let edit = host.inner.input_rules.iter().find_map(|(_, f)| f(&Ctx::new(&state.doc, &state.view), msg))?;
    Some(apply(state, edit))
}

/// The messages input rules see: those that edit through the keyboard or the clipboard.
fn takes_input(msg: &Msg) -> bool {
    msg.edits() && !matches!(msg, Msg::Undo | Msg::Redo | Msg::Command { .. } | Msg::Edit { .. } | Msg::External { .. } | Msg::InsertBlocks { .. })
}

/// Applies an edit as one undo step. A malformed edit (ranges out of order or past the end)
/// changes nothing and says so.
pub(crate) fn apply(state: &mut State, edit: Edit) -> Vec<Effect> {
    let len = state.doc.text.len_chars();
    let mut at = 0;
    for &(from, to, _) in &edit.changes {
        if from < at || to < from || to > len {
            state.view.status = Some("a command's edit was out of range; nothing changed".into());
            return Vec::new();
        }
        at = to;
    }
    let pins = if edit.keep_gaps { crate::outline::rules::pins_all(state) } else { None };
    if !edit.is_noop() || edit.selection.is_some() {
        let selection = match edit.selection {
            Some(s) => s,
            None => Ctx::new(&state.doc, &state.view).mapped_selection(&edit.changes),
        };
        let txn = Transaction::change(
            &state.doc.text,
            edit.changes.iter().map(|(a, b, t)| (*a, *b, (!t.is_empty()).then(|| Tendril::from(t.as_str())))),
        )
        .with_selection(selection);
        let ops = edit.marks;
        update::commit_with(state, txn, Step::default(), move |m, new| {
            for op in ops {
                match op {
                    MarkOp::Mint { pos, attrs } => {
                        m.mint_with(crate::marks::line_start_at(new, pos), attrs);
                    }
                    MarkOp::Remove { id } => {
                        m.remove(id);
                    }
                    MarkOp::SetGap { id, gap } => {
                        m.set_gap(id, gap);
                    }
                    MarkOp::SetData { id, data } => {
                        m.set_data(id, data);
                    }
                }
            }
        });
        state.view.fit(&state.doc);
    }
    if let Some(pins) = pins {
        crate::outline::rules::pin(state, pins);
    }
    if let Some(text) = edit.status {
        state.view.status = Some(text);
    }
    edit.effects.into_iter().map(|(name, data)| Effect::Host { name, data }).collect()
}
