---
title: Vaults
blurb: One vault per project. Each with its colour, its settings, its agents.
icon: vault
order: 5
---

A **vault** is one project's notes: its journal, pages, tasks and history. You have a home vault (`personal`), and you can make one per project, solo or shared through a synced folder.

```sh
$ thc vault                 # which vault you're in, and why
$ thc vault ls              # every vault, with open and inbox counts
$ thc vault new acme        # a new vault
$ thc vault add <folder>    # join one someone shared with you
$ thc vault use acme        # make it current on this machine
$ thc vault rename side sidequest
$ thc vault rm side         # unregisters it; never deletes anything
```

- **The vault follows the folder.** First match wins: `--vault <name>`, `THC_VAULT`, a `.thc.toml` in this folder or a parent (`vault = "acme"`), `thc vault use`, then home. Inside a project with a `.thc.toml`, every command, and every agent, uses that project's vault on its own.
- **You always see where you are.** Outside home, listings start with `vault acme · 7 open · thc vault use personal`.
- **Each vault has a colour.** Home is ember; new vaults take rose, sea, iris and graphite. The header shows `[•] acme` in its colour, and the cursor row, active tab and caret wear it too.
- **Switch in the app** with <kbd>V</kbd> (or <kbd>space</kbd> <kbd>g</kbd> <kbd>v</kbd>): a picker with counts, <kbd>Enter</kbd> to switch, <kbd>n</kbd> (or the `+ new vault` row) for a new vault. Vaults made, renamed or removed elsewhere, say by an agent, show up live in the picker, Today and Agenda.
- **Today and Agenda show every vault.** A row from another vault names it, and anything you do to that row happens in its own vault. <kbd>space</kbd> <kbd>t</kbd> <kbd>v</kbd> gives a section per vault.
- **Reminders fire from every vault.** One background service serves them all.
- **Ask across vaults** in any query:

```sh
$ thc q 'vault:* status:open sort:due'
$ thc q 'vault:(acme or personal) #offsite'
$ thc q 'vault:* status:open group:vault'
```

- **An id from another vault works anywhere:** `thc done acme/k7q2m`.
- **A vault can carry settings** in its own `settings.toml`, synced with it, so a team shares them: look and feel (`[tui]`, `[tui.focus]`, `[keys.*]`, `[theme]`), a short name, and where captures land (`[capture] target`). Never what runs or what's trusted; those lines are ignored with a note.
- **Settings layer:** yours, then the vault's, then your per-vault override. `thc config --effective` lists every setting with where it came from; `thc config --why tui.focus.preset` shows each layer.
