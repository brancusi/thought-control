# web/: thought-central's public site

The public website for thc: home, features, use cases, docs, changelog, under the hood. It's
built with Astro to static HTML. The only JavaScript is a few small vanilla scripts: the typed
demo, the copy button, the header bar and the keyboard (1–6, t, ?).

```sh
cd web
mise exec -- npm install
mise exec -- npm run dev        # http://localhost:4321
mise exec -- npm run build      # → dist/  (BASE=/sub/ npm run build to serve under a sub-path)
mise exec -- npm run renders    # re-render the TUI frames (scratch vaults only; THC=/path/to/thc)
mise exec -- npm run gen        # regenerate src/generated/ (keys, help) from the binary, scratch HOME
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
  - `src/styles/site.css` holds the design-system tokens, dark first.
  - `src/components/Icon.astro` is the single-stroke icon set.
  - `src/lib/rehype-shell.mjs` styles terminal sessions in code blocks.
- **Public-safety rules:**
  - Sample data only.
  - No personal paths or names.
  - Download links point only at the public releases repo.
  - Nothing that hasn't shipped.
