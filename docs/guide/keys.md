# Keys

The essentials first. Every key, by where it works, is at the end: those tables are
**generated** from the keymap (`thc keys --markdown`), so they can't drift from what the keys do.
In thc, `F1` (or `?` in lists) shows the keys for where you are.

## While writing (journal and pages)

`⌃T` task · `Enter` new line (twice: new note) · `Tab` / `⇧Tab` indent · `⇧`+arrows select ·
`⌃C` / `⌃X` copy and cut · `⌃Z` / `⌃Y` undo and redo · `[[` link · `⌃O` open or go to ·
`⌃P` / `⌃N` day · `Esc` save and go back (to the list or view you came from, else Today) · `⌃Q` quit · `F1` keys · `⌥Z` focus

## In lists (Today, Inbox, Tasks, Search, Log)

`j` / `k` move · `Enter` open · `x` done · `d` / `s` dates · `p` priority · `#` tags · `m` move ·
`D` delete · `u` undo · `/` search · `f` filter · `,` sort (Tasks) · `1`–`7` views ·
`:` commands · `⌃O` go to a page or day · `?` keys · `q` back, then quit ·
`space` the leader: a panel shows what can follow (`space g` go, `space f` find, `space n` new…)

## The mouse

Click to place the cursor, drag to select, click a `[ ]` to toggle it, click a date to change it,
and click any tab, row or footer key. Hold `⇧` (`⌥` in iTerm2 and Terminal.app) while dragging for
your terminal's own selection. More in [The mouse](mouse.md).

## Remapping

The easy way: **`thc keys --edit`**, or **`:remap`** in the TUI's command palette. It opens your config at a list of every key, commented
out and grouped by where it works, each with what it does. Uncomment a line (and its
`[keys.…]` line above), change the key on the left, save, and thc checks it for you:
`keys ok · 1 remapped`, or the problem with its line number. Run it again any time: the list
refreshes, and the lines you changed stay. The TUI resumes after the editor closes and picks
up remaps live, including changes made from another terminal. Help marks remapped keys
with `•` and shows the editing command in its footer. `--context write` opens at that part;
`--print` just shows the list. Three examples:

- unbind a key: `"q" = "no_op"`;
- give an action a second key: `"F9" = "focus.toggle"`;
- move an action: `"C-g" = "doc.open"`.

Or by hand: put `[keys.<context>]` tables in `~/.config/thought/config.toml`: a key on the left, an action
(the `Action` column below) on the right. A key replaces what it did in that context;
`"no_op"` unbinds it.

```toml
[keys.list]
"C-x" = "node.done"     # ⌃X completes, as well as x
"q" = "no_op"           # never quit with q

[keys.write]
"A-t" = "doc.task_cycle"
```

thc checks them when it starts and says what it refused in the bar: an unknown action, a plain
letter in `write` (it would stop typing it), a key that starts a longer one, or `Esc` and `⌃C`
taken away. `thc keys --conflicts` lists every problem; `thc keys` shows the keymap in effect.
Contexts: `global`, `list`, `today`, `inbox`, `tasks`, `pages`, `journal`, `search`, `log`,
`write`. The pop-ups keep their own keys for now.

## Every key

Each table is one context: `global` works everywhere outside writing and pop-ups, `list` on a
list's selected row, the view names (`today`, `tasks`…) only there, `write` in a journal day or a
page, and the rest in their pop-ups. **When** names a condition (`has_node`: a row is selected).
Your remaps show here too: `thc keys` prints the keymap in effect.

<!-- keys:begin (generated: scripts/keys-guide.sh) -->
### `global`

| Keys | Does | Action | When |
|---|---|---|---|
| `*` | scope | `view.scope` |  |
| `⌘[` `⌃⌥←` | back | `nav.back` |  |
| `⌘]` `⌃⌥→` | forward | `nav.forward` |  |
| `1` | Today | `go.today` |  |
| `2` | Inbox | `go.inbox` |  |
| `3` | Tasks | `go.tasks` |  |
| `4` | Pages | `go.pages` |  |
| `5` | Journal | `go.journal` |  |
| `6` | Search | `go.search` |  |
| `7` | Log | `go.log` |  |
| `Tab` | next view | `view.next` |  |
| `⇧Tab` | previous view | `view.prev` |  |
| `?` `F1` | keys | `help.context` |  |
| `:` | commands | `palette.open` |  |
| `⌃O` | open | `finder.open` |  |
| `/` | filter pages | `pages.filter` | pages_index |
| `/` | search | `search.find` |  |
| `f` | filter | `tasks.filter` |  |
| `\` | detail | `pane.detail_toggle` |  |
| `⌃W` | next pane | `pane.next` |  |
| `C` | context | `context.toggle` |  |
| `T` | today's journal | `go.journal_today` |  |
| `g d` | go to date | `go.date` |  |
| `u` `U` | undo | `undo` |  |
| `q` | back | `back` | can_back |
| `q` `⌃C` `⌃Q` | quit | `quit` |  |
| `⌃L` |  | `redraw` |  |
| `⌥S` | sidebar | `sidebar.focus` | sidebar_has_panels |
| `⌥⇧T` | reopen | `sidebar.reopen` | sidebar_closed_any |
| `⌥\` | hide · show | `sidebar.toggle` | sidebar_has_panels |
| `⌥=` | width | `sidebar.wider` | sidebar_shown |
| `⌥-` | width | `sidebar.narrower` | sidebar_shown |
| `⌥0` | width | `sidebar.width_auto` | sidebar_shown |

### `leader`

| Keys | Does | Action | When |
|---|---|---|---|
| `space space` | commands | `palette.open` |  |
| `space f p` | page or day | `finder.open` |  |
| `space f d` `space g d` | day | `go.date` |  |
| `space f t` | tag | `find.tag` |  |
| `space f s` | search everything | `search.find` |  |
| `space g t` | Today | `go.today` |  |
| `space g i` | Inbox | `go.inbox` |  |
| `space g k` | Tasks | `go.tasks` |  |
| `space g p` | Pages | `go.pages` |  |
| `space g v` | Vaults | `vault.picker` |  |
| `space g h` | history | `nav.history` |  |
| `space g j` | Journal | `go.journal` |  |
| `space g s` | Search | `go.search` |  |
| `space g l` | Log | `go.log` |  |
| `space g g` | today's journal | `go.journal_today` |  |
| `space n p` | page | `page.new` |  |
| `space n c` | capture here | `capture.here` |  |
| `space n i` | to inbox | `capture.inbox` |  |
| `space t f` | focus | `focus.toggle` |  |
| `space t F` | focus elements | `focus.overlay` |  |
| `space t c` | context | `context.toggle` |  |
| `space t d` | detail pane | `pane.detail_toggle` |  |
| `space t i` | ids | `ids.toggle` |  |
| `space t s` | sort | `tasks.sort_cycle` | view_tasks |
| `space t w` | agenda | `today.agenda_toggle` | view_today |
| `space t v` | by vault | `today.by_vault` | view_today |
| `space v 1` | view 1 | `view.slot.1` |  |
| `space v 2` | view 2 | `view.slot.2` |  |
| `space v 3` | view 3 | `view.slot.3` |  |
| `space v 4` | view 4 | `view.slot.4` |  |
| `space v 5` | view 5 | `view.slot.5` |  |
| `space v 6` | view 6 | `view.slot.6` |  |
| `space v 7` | view 7 | `view.slot.7` |  |
| `space v 8` | view 8 | `view.slot.8` |  |
| `space v 9` | view 9 | `view.slot.9` |  |
| `space v s` | save this filter | `view.save` |  |
| `space v e` | edit views | `views.edit` |  |
| `space c` | compare | `node.compare` | any_conflict |
| `space r` | review lane | `review.lane_open` | to_review |
| `space u` | update thc | `update` | update_available |
| `space e` | $EDITOR | `node.edit_external` |  |
| `space E` | $EDITOR page | `doc.edit_external` |  |
| `space q` | quit | `quit` |  |
| `space ?` | every key | `help.all` |  |
| `space a` | about · what's new | `about` |  |
| `space w w` | focus | `sidebar.focus` |  |
| `space w o` | aside | `sidebar.open_aside` |  |
| `space w x` | close | `sidebar.close` |  |
| `space w X` | close all | `sidebar.close_all` |  |
| `space w p` | pin | `sidebar.pin` |  |
| `space w c` | fold | `sidebar.fold` |  |
| `space w m` | to main | `sidebar.to_main` |  |
| `space w h` | hide · show | `sidebar.toggle` |  |
| `space w r` | reopen | `sidebar.reopen` |  |
| `space w =` | wider | `sidebar.wider` |  |
| `space w -` | narrower | `sidebar.narrower` |  |
| `space w 0` | auto width | `sidebar.width_auto` |  |

### `list`

| Keys | Does | Action | When |
|---|---|---|---|
| `V` | vaults | `vault.picker` |  |
| `j` `↓` | down | `cursor.down` |  |
| `k` `↑` | up | `cursor.up` |  |
| `g g` `Home` | top | `cursor.top` |  |
| `G` `End` | bottom | `cursor.bottom` |  |
| `⌃D` | half page down | `cursor.half_down` |  |
| `⌃U` | half page up | `cursor.half_up` |  |
| `PgDn` | page down | `cursor.page_down` |  |
| `PgUp` | page up | `cursor.page_up` |  |
| `Enter` `→` | open | `open` |  |
| `Esc` `h` `←` | back | `back` |  |
| `h` `←` | fold | `fold.close` | foldable |
| `l` `→` | unfold | `fold.open` | foldable |
| `x` | done | `node.done` | node_is_task |
| `X` | reopen | `node.reopen` | node_is_task |
| `t` | task | `node.task_toggle` | has_node |
| `S space` | todo | `node.status.todo` | has_node |
| `S /` | doing | `node.status.doing` | has_node |
| `S w` | waiting | `node.status.waiting` | has_node |
| `S x` | done | `node.status.done` | has_node |
| `S -` | cancelled | `node.status.cancelled` | has_node |
| `d` | due | `node.due` | has_node |
| `s` | scheduled | `node.scheduled` | has_node |
| `p h` | high | `node.priority.high` | has_node |
| `p m` | med | `node.priority.med` | has_node |
| `p l` | low | `node.priority.low` | has_node |
| `p -` | none | `node.priority.none` | has_node |
| `#` | tags | `node.tags` | has_node |
| `i` | edit text | `node.text` | has_node |
| `m` | move | `node.move` | has_node |
| `D` | delete | `node.delete` | has_node |
| `r` | skip | `node.skip` | node_repeats |
| `z` | snooze | `node.snooze` | node_has_alert |
| `Z` | ack | `node.ack` | node_has_alert |
| `c` | compare | `node.compare` |  |
| `e` | $EDITOR | `node.edit_external` | has_node |
| `E` | $EDITOR page | `doc.edit_external` |  |
| `L` | history | `node.history` |  |
| `y` | copy id | `node.copy_id` | has_node |
| `Y` | copy full id | `node.copy_full_id` | has_node |
| `space` | leader | `leader` |  |
| `a` | add | `capture.here` |  |
| `⌥O` `⇧Enter` `o` | aside | `sidebar.open_aside` |  |
| `A` | inbox | `capture.inbox` |  |

### `today`

| Keys | Does | Action | When |
|---|---|---|---|
| `x` | done | `node.done` | node_is_task |
| `a` | add | `capture.here` |  |
| `w` | agenda | `today.agenda_toggle` |  |
| `space` | leader | `leader` |  |
| `v` | done today | `today.show_done` |  |
| `[` | day | `day.prev` |  |
| `]` | day | `day.next` |  |
| `{` | week | `week.prev` |  |
| `}` | week | `week.next` |  |

### `inbox`

| Keys | Does | Action | When |
|---|---|---|---|
| `m` | move | `node.move` | has_node |
| `t` | task | `node.task_toggle` | has_node |
| `d` | date | `node.due` | has_node |
| `x` | done | `node.done` | node_is_task |
| `D` | delete | `node.delete` | has_node |

### `tasks`

| Keys | Does | Action | When |
|---|---|---|---|
| `f` | filter | `tasks.filter` |  |
| `,` | sort | `tasks.sort_cycle` |  |
| `x` | done | `node.done` | node_is_task |
| `space` | leader | `leader` |  |

### `pages`

| Keys | Does | Action | When |
|---|---|---|---|
| `.` | ids | `ids.toggle` |  |
| `Enter` | open | `open` | pages_index |
| `/` | filter | `pages.filter` | pages_index |

### `journal`

| Keys | Does | Action | When |
|---|---|---|---|
| `[` | day | `day.prev` |  |
| `]` | day | `day.next` |  |
| `{` | week | `week.prev` |  |
| `}` | week | `week.next` |  |
| `.` | ids | `ids.toggle` |  |

### `search`

| Keys | Does | Action | When |
|---|---|---|---|
| `Enter` `→` | open | `open` |  |
| `/` | search | `search.find` |  |

### `log`

| Keys | Does | Action | When |
|---|---|---|---|
| `a` | accept | `review.accept` | lane_on |
| `u` | undo | `review.undo` | lane_on |
| `A` | accept all | `review.accept_all` | lane_on |
| `r` | all changes | `log.lane_toggle` |  |
| `u` | undo tx | `log.undo_tx` | log_all |
| `@` | actor | `log.actor_cycle` |  |
| `R` | rewind | `node.rewind` | node_log |
| `Enter` | open | `open` | log_browse |

### `toast.alert`

| Keys | Does | Action | When |
|---|---|---|---|
| `x` | done | `toast.done` |  |
| `z` | snooze | `toast.snooze` |  |
| `Z` | ack | `toast.ack` |  |

### `toast.agent`

| Keys | Does | Action | When |
|---|---|---|---|
| `L` | review | `review.last_agent_tx` |  |
| `u` | undo | `undo` |  |

### `toast.confirm`

| Keys | Does | Action | When |
|---|---|---|---|
| `u` | undo | `undo` |  |

### `write`

| Keys | Does | Action | When |
|---|---|---|---|
| `⌃T` `⌃Enter` | task | `doc.task_cycle` |  |
| `⌃O` `⌥Enter` | open | `doc.open` |  |
| `⌃P` `⌥[` | day | `doc.day_prev` |  |
| `⌃N` `⌥]` | day | `doc.day_next` |  |
| `⌥O` | aside | `sidebar.open_aside` | has_target |
| `⌥S` | sidebar | `sidebar.focus` | sidebar_has_panels |
| `⌥⇧T` | reopen | `sidebar.reopen` | sidebar_closed_any |
| `⌥\` | hide · show | `sidebar.toggle` | sidebar_has_panels |
| `⌥=` | width | `sidebar.wider` | sidebar_shown |
| `⌥-` | width | `sidebar.narrower` | sidebar_shown |
| `⌥0` | width | `sidebar.width_auto` | sidebar_shown |
| `Esc` | done | `doc.done` |  |
| `⌘Z` `⌃Z` | undo | `doc.undo` |  |
| `⇧⌘Z` `⌘Y` `⌘R` `⌃Y` `⌃R` `⌃⇧Z` | redo | `doc.redo` |  |
| `F1` `⌥?` | keys | `help.context` |  |
| `⌃Q` | quit | `quit` |  |
| `⌘C` `⌃C` | copy | `clip.copy` |  |
| `⌘X` `⌃X` | cut | `clip.cut` |  |
| `⌥V` | plain paste | `paste.plain_next` |  |
| `⌘A` `⌥A` | select all | `select.all` |  |
| `⌃J` `⇧Enter` | line break | `line.soft_break` |  |
| `Enter` | new line · twice: new note | `line.newline` |  |
| `Tab` | indent | `line.indent` |  |
| `⇧Tab` | outdent | `line.outdent` |  |
| `⌥↑` | move line | `line.move_up` |  |
| `⌥↓` | move line | `line.move_down` |  |
| `⌥Z` | focus | `focus.toggle` |  |
| `⌥T` | today | `go.journal_today` |  |
| `⌘[` `⌃⌥←` | back | `nav.back` |  |
| `⌘]` `⌃⌥→` | forward | `nav.forward` |  |
| `⌥1` | Today | `go.today` |  |
| `⌥2` | Inbox | `go.inbox` |  |
| `⌥3` | Tasks | `go.tasks` |  |
| `⌥4` | Pages | `go.pages` |  |
| `⌥5` | Journal | `go.journal` |  |
| `⌥6` | Search | `go.search` |  |
| `⌥7` | Log | `go.log` |  |
| `⌥:` | commands | `palette.open` |  |
| `⌃D` `Del` `⇧Del` | delete | `edit.delete_forward` |  |
| `⌘V` `⌃V` | paste (screenshots too) | `clip.paste_system` |  |
| `←` `⇧←` | left (⇧ selects) | `move.left` |  |
| `→` `⇧→` | right (⇧ selects) | `move.right` |  |
| `↑` `⇧↑` | up (⇧ selects) | `move.up` |  |
| `↓` `⇧↓` | down (⇧ selects) | `move.down` |  |
| `⌥←` `⌃←` `⌥B` `⌥⇧←` `⌃⇧←` `⌥⇧B` | word left (⇧ selects) | `move.word_left` |  |
| `⌥→` `⌃→` `⌥F` `⌥⇧→` `⌃⇧→` `⌥⇧F` | word right (⇧ selects) | `move.word_right` |  |
| `⌘←` `⇧⌘←` `Home` `⌃A` `⇧Home` `⌃⇧A` | line start (⇧ selects) | `move.home` |  |
| `⌘→` `⇧⌘→` `End` `⌃E` `⇧End` `⌃⇧E` | line end (⇧ selects) | `move.end` |  |
| `⌘↑` `⌘Home` `⇧⌘↑` `⇧⌘Home` `⌃Home` `⌃⇧Home` | document start (⇧ selects) | `move.doc_start` |  |
| `⌘↓` `⌘End` `⇧⌘↓` `⇧⌘End` `⌃End` `⌃⇧End` | document end (⇧ selects) | `move.doc_end` |  |
| `PgUp` `⇧PgUp` | page up (⇧ selects) | `move.page_up` |  |
| `PgDn` `⇧PgDn` | page down (⇧ selects) | `move.page_down` |  |
| `⌃↑` `⌃⇧↑` | block up (⇧ selects) | `move.para_up` |  |
| `⌃↓` `⌃⇧↓` | block down (⇧ selects) | `move.para_down` |  |
| `⌫` `⇧⌫` `⌃H` | backspace | `edit.backspace` |  |
| `⌥⌫` `⌃⌫` `⌃W` | delete word | `edit.delete_word` |  |
| `⌘⌫` `⌃U` | delete to line start | `edit.kill_to_start` |  |
| `⌃K` | kill line | `edit.kill_to_end` |  |

### `sidebar`

| Keys | Does | Action | When |
|---|---|---|---|
| `⌥S` | main | `sidebar.focus` |  |
| `Esc` | back | `sidebar.back` |  |
| `⌥J` | panel | `sidebar.next` |  |
| `⌥K` | panel | `sidebar.prev` |  |
| `⌥C` | fold | `sidebar.fold` |  |
| `⌥W` | close | `sidebar.close` |  |
| `⌥M` | to main | `sidebar.to_main` | panel_is_doc |
| `⌥P` | pin · unpin | `sidebar.pin` |  |
| `⌥⇧K` | move | `sidebar.move_up` |  |
| `⌥⇧J` | move | `sidebar.move_down` |  |
| `⌥⇧T` | reopen | `sidebar.reopen` | sidebar_closed_any |
| `⌥\` | hide · show | `sidebar.toggle` |  |
| `⌥=` | width | `sidebar.wider` | sidebar_shown |
| `⌥-` | width | `sidebar.narrower` | sidebar_shown |
| `⌥0` | width | `sidebar.width_auto` | sidebar_shown |
| `⌃W` | next pane | `pane.next` |  |
| `F1` | keys | `help.context` |  |

### `link`

| Keys | Does | Action | When |
|---|---|---|---|
| `↑` `⌃P` | choose | `link.prev` |  |
| `↓` `⌃N` | choose | `link.next` |  |
| `Enter` `Tab` | link | `link.insert` |  |
| `Esc` | close | `link.close` |  |

### `prompt`

| Keys | Does | Action | When |
|---|---|---|---|
| `Enter` | save | `prompt.submit` |  |
| `Esc` `⌃C` | cancel | `prompt.cancel` |  |

### `palette`

| Keys | Does | Action | When |
|---|---|---|---|
| `Enter` | run | `palette.run` |  |
| `Tab` | complete | `palette.complete` |  |
| `Esc` `⌃C` |  | `palette.close` |  |
| `↓` `⌃N` |  | `palette.next` |  |
| `↑` `⌃P` |  | `palette.prev` |  |

### `finder`

| Keys | Does | Action | When |
|---|---|---|---|
| `↑` `⌃P` | choose | `finder.prev` |  |
| `↓` `⌃N` | choose | `finder.next` |  |
| `Enter` | go | `finder.go` |  |
| `Esc` | close | `finder.close` |  |

### `vaults`

| Keys | Does | Action | When |
|---|---|---|---|
| `↑` | choose | `vaults.prev` |  |
| `↓` | choose | `vaults.next` |  |
| `Enter` | switch | `vaults.switch` |  |
| `n` | new vault | `vaults.new` |  |
| `Esc` | close | `vaults.close` |  |

### `capture`

| Keys | Does | Action | When |
|---|---|---|---|
| `Enter` | save | `capture.save` |  |
| `Tab` | target | `capture.target_next` |  |
| `⇧Tab` |  | `capture.target_prev` |  |
| `Esc` `⌃C` |  | `capture.close` |  |

### `move`

| Keys | Does | Action | When |
|---|---|---|---|
| `Enter` | move | `move.go` |  |
| `1` `2` `3` | recent | `move.recent` |  |
| `↓` `Tab` |  | `move.next` |  |
| `↑` `⇧Tab` |  | `move.prev` |  |
| `Esc` |  | `move.close` |  |

### `compare`

| Keys | Does | Action | When |
|---|---|---|---|
| `1` | keep current | `compare.keep_current` | compare_text |
| `2` | keep other | `compare.keep_other` | compare_text |
| `b` | both | `compare.both` | compare_text |
| `e` | $EDITOR | `compare.edit` | compare_text |
| `Enter` | ok | `compare.ok` | compare_move |
| `1` | keep here | `compare.keep_here` | compare_rehomed |
| `2` | delete | `compare.delete` | compare_rehomed |
| `Esc` | later | `compare.later` |  |

### `focus`

| Keys | Does | Action | When |
|---|---|---|---|
| `1` `2` `3` | preset | `focus.preset` |  |
| `Enter` | save | `focus.save` |  |
| `Esc` `⌃C` | close | `focus.close` |  |

### `help`

| Keys | Does | Action | When |
|---|---|---|---|
| `?` | every key | `help.all` |  |
| `Esc` | close | `help.close` |  |

### `notes`

| Keys | Does | Action | When |
|---|---|---|---|
| `Esc` | close | `notes.close` |  |

### `layers`

| Keys | Does | Action | When |
|---|---|---|---|
| `F2` `⌥N` | next | `tour.next` | tour |
| `⇧F2` `⌥P` | back | `tour.back` | tour |
| `F3` `⌥G` | stop | `tour.stop` | tour |
| `Esc` | dismiss | `layers.dismiss` | agent_layers |
| `⌘[` `⌃⌥←` | take back | `layers.back` | agent_layers |
<!-- keys:end -->
