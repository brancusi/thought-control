# Vaults

A **vault** is one project's notes: its journal, pages, tasks and history. You have a home vault
(`personal`, in `~/thought`), and you can make one per project, solo or shared with a team
through a synced folder.

```
thc vault                 # which vault you're in, and why
thc vault ls              # every vault on this Mac, with open and inbox counts
thc vault new acme        # a new vault in ~/thought-vaults/acme
thc vault new acme ~/Library/CloudStorage/Dropbox/acme/thought   # one you'll share
thc vault add <folder>    # join a vault someone shared with you
thc vault use acme        # make it current on this Mac
thc vault use personal    # back home
thc vault rename side sidequest
thc vault rm side         # unregisters it; never deletes anything
```

**Which vault a command uses**, first match wins: `--vault <name>`, `THC_VAULT`, a
`.thc.toml` in the folder or a parent (`vault = "acme"`), `thc vault use`, then home. So inside
`~/code/acme` with a `.thc.toml`, every `thc` command (and every agent) uses `acme` on its own.

**You always see where you are.** Outside your home vault, listings start with a line like
`vault acme · 7 open · thc vault use personal`.

**Each vault has a colour.** Home is ember; a new vault gets the next of rose, sea, iris and
graphite. The TUI's header shows the vault's name in its colour (`[•] acme`), and the cursor
row, the active tab and the caret wear it too. In a 16-colour terminal the name is a reversed
chip instead.

**A vault can carry settings** in `settings.toml` in its folder: synced with it, so a team
shares them, and only what it changes. It can set how thc looks and feels (`[tui]`, `[tui.focus]`,
`[keys.*]`, `[theme] accent` and `theme`), never what runs or what's trusted (hooks, policy,
the daemon): those lines are ignored with a note. To opt out of one for yourself, put your own
value under `[vaults.acme.settings]` in your config.

```
thc config --effective            # every setting in effect, and where it came from
thc config --why tui.focus.preset # each layer that sets one key
```

**Switch in the TUI** with `V` (or `space g v`): the picker lists every vault with its open and
inbox counts. `Enter` switches this session, saving first, and `n` makes a new vault. The
palette (`:`) has `vault: acme` too. Outside home, a page's crumb starts with the vault
(`acme › ¶ Pages › Health`). A long name is shortened in the header; set your own short one in
the vault's `settings.toml`:

```toml
[vault]
name_short = "tcrl"   # 12 characters at most
```

**Ask across vaults** with `vault:` in any query:

```
thc q 'vault:* status:open sort:due'            # every vault
thc q 'vault:(acme or personal) #offsite'       # two of them
thc q 'vault:* -vault:side status:open'         # all but one
thc q 'vault:* status:open group:vault'         # a section per vault
```

When the answer comes from more than one vault, each row says which (`acme · due fri`). And an
id from another vault works anywhere: `thc done acme/k7q2m`.

**Today and Agenda are about you**, so they show every vault: `thc today`, `thc agenda` and
the TUI's Today. A row from another vault starts its meta with that vault's name (in its colour
in the TUI), and anything you do to it (`x`, `d`, a date, a tag…) happens in its own vault, and
`u` undoes it there. `Enter` on such a row takes you into its vault while it's open, and `Esc`
brings you back. `space t v` shows a section per vault. Capture (`a`) goes to the vault in the
header. Agents see only their own vault.

**Reminders fire from every vault.** The one background service (`thc daemon`) serves every
registered vault, so a reminder in `acme` reaches you like one at home, named:
`acme · Book the venue · due 17:00`. A vault you add starts being served within half a minute.

**Views with sections.** Today, Agenda, Inbox and Tasks are built-in views made of sections
(`thc view ls` shows them). Make Today yours:

```
thc view set today --section Due 'due<=today status:open' --section Waiting 'status:waiting'
thc view reset today                   # back to the built-in
thc view copy today acme-today         # then narrow it:
thc view set acme-today --scope vault:acme
thc q @acme-today
```

A view that spans vaults is kept in your home vault; one that reads a single vault is kept in
that vault, where everyone who has it sees it.

Coming next: sending a capture to another vault with `vault:acme`.
