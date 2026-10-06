---
title: Vaults
description: One notebook per project, solo or shared through a synced folder, with Today spanning all of them.
order: 5
group: Guide
---

A **vault** is one project's notes: its journal, pages, tasks and history. You have a home vault
(`personal`, in `~/thought`), and you can make one per project, solo or shared with a team
through a synced folder.

```sh
$ thc vault                 # which vault you're in, and why
$ thc vault ls              # every vault on this machine, with open and inbox counts
$ thc vault new acme        # a new vault in ~/thought-vaults/acme
$ thc vault new acme ~/Dropbox/acme/thought   # one you'll share
$ thc vault add <folder>    # join a vault someone shared with you
$ thc vault use acme        # make it current on this machine
$ thc vault use personal    # back home
$ thc vault info            # path, sync, counts
$ thc vault rename side sidequest
$ thc vault rm side         # unregisters it; never deletes anything
```

A vault is a folder. Each device writes only its own log files inside it, so Dropbox, iCloud or
Syncthing never see two writers on one file, and you never get "conflicted copies".

## Which vault a command uses

First match wins:

1. `--vault <name>` on the command
2. `THC_VAULT` in the environment
3. a `.thc.toml` in the current folder or a parent (`vault = "acme"`)
4. `thc vault use`
5. home

So inside `~/code/acme` with a `.thc.toml`, every `thc` command, and every agent you run there,
uses `acme` on its own. `thc vault` tells you which rule picked it.

**You always see where you are.** Outside your home vault, listings start with a line like
`vault acme · 7 open · thc vault use personal`.

## Colours and names

**Each vault has a colour.** Home is ember; a new vault gets the next of rose, sea, iris and
graphite. The TUI's header shows the vault's name in its colour (`[•] acme`), and the cursor
row, the active tab and the caret wear it too. In a 16-colour terminal the name is a reversed
chip instead. A long name is shortened in the header; set your own short one in the vault's
`settings.toml`:

```toml
[vault]
name_short = "tcrl"   # 12 characters at most
```

## Settings that travel with the vault

A vault can carry settings in `settings.toml` in its folder. They sync with it, so a team shares
them, and they change only what they set. A vault can set how thc looks and feels (`[tui]`,
`[tui.focus]`, `[keys.*]`, `[theme]`), never what runs or what's trusted: those lines are ignored
with a note. To opt out of one for yourself, put your own value under `[vaults.acme.settings]`
in your config.

**Where captures land.** By default, `thc add` writes to today's journal. A vault can send
captures to a page instead:

```toml
[capture]
target = "¶ Issues"
```

That covers `thc add`, `todo` and `remind`, and the capture box. Naming a place
(`--journal today`, `--under <id>`) still wins.

```sh
$ thc config --effective            # every setting in effect, and where it came from
$ thc config --why tui.focus.preset # each layer that sets one key
```

## Switching in the TUI

<kbd>V</kbd> (or <kbd>space</kbd> <kbd>g</kbd> <kbd>v</kbd>) opens the picker: every vault with
its open and inbox counts. <kbd>Enter</kbd> switches this session, saving first, and
<kbd>n</kbd> makes a new vault. The palette (<kbd>:</kbd>) has `vault: acme` too. Outside home, a
page's crumb starts with the vault (`acme › ¶ Pages › Health`).

## Asking across vaults

Add `vault:` to any query:

```sh
$ thc q 'vault:* status:open sort:due'            # every vault
$ thc q 'vault:(acme or personal) #offsite'       # two of them
$ thc q 'vault:* -vault:side status:open'         # all but one
$ thc q 'vault:* status:open group:vault'         # a section per vault
```

When the answer comes from more than one vault, each row says which (`acme · due fri`). An id
from another vault works anywhere: `thc done acme/k7q2m`.

## Today is about you

Today and Agenda show **every vault**: `thc today`, `thc agenda` and the TUI's Today. A row from
another vault starts its meta with that vault's name, in its colour. Anything you do to it
(<kbd>x</kbd>, <kbd>d</kbd>, a tag) happens in its own vault, and <kbd>u</kbd> undoes it there.
<kbd>Enter</kbd> on such a row takes you into its vault while it's open, and <kbd>Esc</kbd> brings
you back. <kbd>space</kbd> <kbd>t</kbd> <kbd>v</kbd> shows a section per vault. Capture
(<kbd>a</kbd>) goes to the vault in the header.

**Agents see only their own vault.** One running in `~/code/acme` reads and writes `acme`, and
writes elsewhere only when you ask (`--vault <name>`).

## Reminders from everywhere

The one background service serves every registered vault, so a reminder in `acme` reaches you
like one at home, named: `acme · Book the venue · due 17:00`. A vault you add starts being
served within half a minute.

## Views that span vaults

A [sectioned view](../views/#views-with-sections) can read several vaults with `--scope`:

```sh
$ thc view copy today acme-today
$ thc view set acme-today --scope vault:acme
```

A view that spans vaults is kept in your home vault. One that reads a single vault is kept in
that vault, where everyone who has it sees it.
