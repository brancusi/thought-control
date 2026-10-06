---
title: Project vaults for a team
who: a small team
blurb: A vault per project in a shared folder. Each person sees every vault in their own Today.
icon: team
order: 3
---

Three of you work on two client projects, acme and globex, and each of you keeps a personal vault at home. The client work lives in a shared Dropbox folder: one vault per project, synced like any other files.

You join acme on Monday. You add the folder, and the header turns sea-blue: `[•] acme`. Inside the acme repo there's a `.thc.toml`, so every `thc` you run there, and every agent you start there, writes to acme without being told. Your personal notes stay at home.

Your Today still shows everything that's yours, from every vault. A row from acme says so in its meta, in acme's colour, and when you mark it done it's done in acme, for everyone. A reminder in globex reaches you named: `globex · Send the draft · due 17:00`.

## The commands

```sh
$ thc vault add ~/Dropbox/clients/acme/thought     # join a shared vault
$ thc vault ls
$ cd ~/code/acme && thc vault
```

One `.thc.toml` in the repo points everything at the project:

```toml
vault = "acme"
```

The vault carries settings for everyone who has it, in its own `settings.toml`:

```toml
[vault]
name_short = "acme"

[theme]
accent = "sea"

[capture]
target = "¶ Issues"
```

Ask across vaults:

```sh
$ thc q 'vault:* status:open due<=+7d sort:due'
$ thc q 'vault:* status:open group:vault'
$ thc q 'vault:(acme or globex) #launch'
$ thc done globex/k7q2m                            # an id from another vault works anywhere
```

Make a Today just for one client, and keep the default:

```sh
$ thc view copy today acme-today
$ thc view set acme-today --scope vault:acme
$ thc q @acme-today
```

An agent in the acme repo sees acme only:

```sh
$ cd ~/code/acme && THC_ACTOR=claude thc prime
```

In the app, <kbd>V</kbd> opens the vault picker with open and inbox counts, and <kbd>space</kbd> <kbd>t</kbd> <kbd>v</kbd> splits Today into a section per vault.

## Why thc fits

- **Silos where they help, one view where you need it.** Projects stay separate and shareable; Today is still about you.
- **No server to run.** A shared folder is the whole backend. Each device writes its own log files, so the sync tool never sees a collision, and a real conflict keeps both versions.
- **Agents stay in their lane.** The folder decides the vault, so an agent working in one repo can't wander into another client's notes.
