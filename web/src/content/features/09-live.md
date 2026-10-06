---
title: Live
blurb: A background service keeps every window current, without touching your cursor.
icon: live
order: 9
---

`thc daemon` is a small background service, started at login. It serves every vault you have registered, and it's optional: without it, everything still works, a little less live.

```sh
$ thc daemon status
$ thc daemon start | stop | restart
$ thc daemon install     # run it at every login
```

- **`● live`** in the bottom bar means the service is running for this vault. Without it, the bar says so, and thc works on its own.
- **Changes arrive as they happen:** an agent's write, a capture from a script, another device's sync. Lists refresh in place.
- **Your cursor is sacred.** If an agent or another device changes the line you're typing in, thc tells you at once (`◆ claude changed this line`) and applies it when you move off. It never rewrites the text under your caret. If you both changed it, both versions are kept.
- **A toast for agent writes:** <kbd>L</kbd> reviews the change, <kbd>u</kbd> undoes it.
- **Saving never slows typing.** With the service running, a line saves in the background.
- **Reminders** fire from it, in the app as a toast and as a system notification when no window claims them.
- **It notices updates.** If `thc update` ran in another terminal, an open window says `thc 0.9.43 installed · :update reloads here`, and `:update` reloads in place, keeping your view.
- **One service, every vault.** A vault you add is picked up within half a minute.
