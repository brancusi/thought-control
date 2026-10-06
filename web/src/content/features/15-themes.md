---
title: Themes
blurb: Ember, dark or light, in truecolor, 256 or 16 colours. ASCII if you like.
icon: theme
order: 15
---

thc is square, dense and monospaced, with one accent colour. It looks right in whatever terminal you have.

| Theme | For |
|---|---|
| `ember-dark` | The default: warm dark, truecolor |
| `ember-light` | Warm paper, truecolor |
| `ember-dark-256`, `ember-light-256` | The full palette in 256 colours (Terminal.app, tmux, SSH) |
| `ansi` | Your terminal's own 16 colours: its palette decides the hues |

- **Picked for you.** Truecolor when `COLORTERM` says so, otherwise 256 colours. `THC_THEME=ember-light` (or any name above) chooses.
- **`NO_COLOR`** strips colours and keeps bold, dim and reverse. The CLI drops colour on its own when output isn't a terminal.
- **`THC_GLYPHS=ascii`** swaps box drawing and symbols for plain ASCII, for fonts and terminals that need it.
- **An accent per vault:** ember at home; rose, sea, iris or graphite elsewhere. Set it in the vault's `settings.toml`:

```toml
[theme]
accent = "iris"
```

  In a 16-colour terminal, the vault's name becomes a reversed chip instead.
- **Agents are marked,** not coloured only: `◆ claude` reads the same in any theme, and you're `you`.
- **A calm screen.** Each update reaches the terminal as one piece, so scrolling doesn't tear; the writing caret is a steady bar; nothing redraws while you're idle.
- **Works in any terminal:** WezTerm, kitty, Ghostty, iTerm2, Terminal.app, tmux, over SSH.
