# Keys and commands

Keys are layered. **caretline owns the editing vocabulary and a default keymap**: every generic
editing command has a stable id, and a table binds key chords to those ids. **Your app owns the
final bindings**: which context a key works in, your own actions, the user's remaps and the help
you show. caretline's keymap is a default you take, override or ignore.

| Layer | Owner | What it is |
|---|---|---|
| The command catalog | caretline | `commands()`: id, name, description, category; each id is one `Msg` (`command_msg`) |
| The default keymap | caretline | `default_keymap(outline)`: chord → command id, with a platform note for ⌘ |
| Host commands | your app | Commands you register (`Host::command`), with your own ids and keys |
| Final bindings, contexts, app actions | your app | Your key table: caretline's defaults merged with yours |
| User remaps, help, palette | your app | Built from your table and caretline's catalog, so they can't drift |

## The command catalog

Ids are `category.verb` and never change within a major version:

| Category | Commands |
|---|---|
| `move` | `left` `right` `up` `down` `word_left` `word_right` `line_start` `line_end` `doc_start` `doc_end` `page_up` `page_down` `block_up` `block_down` |
| `select` | The same motions, extending the selection (`select.word_right`…), and `all`, `collapse`, `block` |
| `edit` | `newline` `soft_break` `insert_tab` `backspace` `delete_forward` `delete_word` `delete_word_forward` `delete_to_line_start` `delete_to_line_end` `kill_line` |
| `structure` | `indent` `outdent` `move_up` `move_down` |
| `clip` | `copy` `cut` `paste` `paste_plain` |
| `history` | `undo` `redo` |
| `view` | `fold` `unfold` `fold_toggle` (each takes a block) |
| `file` | `save` `quit` |

```rust
use caretline::commands::{command, command_for};
use caretline::{command_msg, commands, default_keymap, Key, KeyCode, Mods, Msg};

assert!(commands().iter().any(|c| c.id == "history.undo"));
assert_eq!(command("move.word_right").unwrap().name, "Word right");
assert_eq!(command_msg("history.undo", None), Some(Msg::Undo));
let ctrl_z = Key { code: KeyCode::Char('z'), mods: Mods { ctrl: true, ..Mods::default() } };
assert_eq!(command_for(false, &ctrl_z), Some("history.undo"));
assert!(default_keymap(true).iter().any(|b| b.keys == "<tab>" && b.command == "structure.indent"));
```

The catalog has no host's commands in it. Commands you register with `Host::command` are yours:
give them your own ids and your own keys.

## The default keymap

Chords are written in key-script notation: `<c-x>` Ctrl, `<a-x>` Alt, `<d-x>` ⌘ (macOS),
`<s-x>` Shift, combined in that order (`<c-s-z>`). Printable keys without Ctrl, Alt or ⌘ type
themselves. Bindings marked `mac` are ⌘ chords, for a terminal that forwards ⌘ (the kitty
keyboard protocol's "super"); each has a Ctrl or Alt twin. `keymap(&key)` is a lookup in this
table, and `outline_keymap` adds the outline document's keys on top.

`caretline keys` prints the whole table (`--outline` for an outline document, `--json` for the
data); in the editor, **F1** or **Alt-?** shows it.

```console
$ caretline keys --outline
caretline keys (outline documents)

Move
  Left                     Left
  Right                    Right
  Up                       Up
  Down                     Down
  Word left                Alt-Left, Ctrl-Left, Alt-B
  Word right               Alt-Right, Ctrl-Right, Alt-F
  Line start               Cmd-Left (mac), Home, Ctrl-A
  …
```

Over the protocol, `commands.list` returns the catalog (and the host's command names) and
`keymap.get {outline}` the table; caretline-mcp's `commands` tool returns both.

## Building your own key table

1. **Take the defaults.** Read `default_keymap(outline)` and translate each command id to your
   action (or run `command_msg(id, None)` directly).
2. **Override.** Your own rows on a key win over the default on that key (Esc might close your
   editor instead of collapsing the selection).
3. **Add your commands.** Bind your keys to `Msg::Command { name, args }`.
4. **Generate help** from your merged table and caretline's names and descriptions.
5. **Apply user remaps last**, with your conflict check.

thc does exactly this: its write context imports caretline's table (translating
`select.word_right` to its `move.word_right` with ⇧, `structure.indent` to `line.indent`…), keeps
its own rows for ⌃D, its system paste and Esc, binds ⌃T to its own command, and lists the
editing keys in caretline's words in `thc keys` and its palette.

## Key scripts

Tests, the headless CLI and the protocol's `keys` op take a key script: literal characters
plus `<...>` tokens. Tokens: `<cr>` `<enter>` `<bs>` `<del>` `<tab>` `<s-tab>` `<esc>` `<space>`
`<left>` `<right>` `<up>` `<down>` `<home>` `<end>` `<pgup>` `<pgdn>` `<lt>` (a literal `<`) and
`<wait:MS>`, with the modifier prefixes above. See [messages.md](messages.md#key-scripts).
