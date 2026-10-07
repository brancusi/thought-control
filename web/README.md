# web/: Thought Control's public site, thoughtcontrol.app

The public website for Thought Control (the `thc` command): home, features, use cases, docs, changelog, under the hood. It's
built with Astro to static HTML. The only JavaScript is a few small vanilla scripts: the typed
demo, the copy button, the header bar and the keyboard (1–6, t, ?).

```sh
cd web
mise exec -- npm install
mise exec -- npm run dev        # http://localhost:4321
mise exec -- npm run build      # → dist/  (BASE=/sub/ npm run build to serve under a sub-path)
mise exec -- npm run renders    # re-render the TUI frames (scratch vaults only; THC=/path/to/thc)
mise exec -- npm run gen        # regenerate src/generated/ (keys, help) from the binary, scratch HOME
mise exec -- npm run scan       # fails on anything private in dist/ (run after every build)
mise exec -- npm run deploy     # build, scan, then wrangler deploy to thoughtcontrol.app
```

- **Content:**
  - `src/content/features/` (one file per group)
  - `src/content/usecases/`
  - `src/content/docs/` (the guide and references)
  - The changelog is read from `../RELEASE_NOTES.md` at build time.
- **Generated:**
  - `src/generated/` (`thc keys --markdown`, `thc --help`, `thc instructions all`)
  - `src/renders/` (real `thc tui` frames)
  - Both are committed. Regenerate them after a release.
- **Look:**
  - The brand is Thought Control v2 "Redacted": ink, newsprint and one acid accent; Archivo,
    Martian Mono and Instrument Serif (all OFL, self-hosted via Fontsource). Devices mean
    something: a censor bar is something withheld, a paper scrap is a card, a stamp is a state.
  - `src/styles/site.css` holds the tokens, dark first (light is newsprint paper).
  - TUI frames are rendered in the `redacted` (dark) and `newsprint` (light) themes.
  - `src/components/Icon.astro` is the single-stroke icon set.
  - `src/lib/rehype-shell.mjs` styles terminal sessions in code blocks.
- **Public-safety rules:**
  - Sample data only.
  - No personal paths or names.
  - Download links point only at the public releases repo.
  - Nothing that hasn't shipped.
