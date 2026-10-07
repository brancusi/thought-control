# sites/caretline: caretline.app

The public site for caretline: the landing page, the docs and the brand page. Astro, static
output, served from Cloudflare Workers static assets.

```sh
cd sites/caretline
mise exec -- npm install
mise exec -- npm run dev        # http://localhost:4321
mise exec -- npm run build      # → dist/, then the Pagefind search index
mise exec -- npm run scan       # fails on anything private in dist/
npx wrangler deploy --config ./wrangler.jsonc   # from this directory, after build + scan
```

- **Docs** are `docs/caretline/*.md`, read in place at build time (`src/content.config.ts`).
  Order and sidebar titles are in `src/lib/docs.ts`. `src/lib/rehype-doc-links.mjs` rewrites
  `x.md#y` to `/docs/x/#y` and `../../crates/…` to GitHub. Mermaid renders in the browser.
- **The ASCII scenes** (warp, torus, cube, tunnel, plasma, fire) are `public/js/field.js`: a
  pure function of time per scene, drawn two-tone on a canvas. Lit cells are the selection
  colour with the glyph knocked out, the way caretline draws a selection.
- **The playground** is the real engine: `crates/caretline` built for
  `wasm32-unknown-unknown` by `wasm/` (a C-ABI wrapper around `Session::handle`) into
  `public/wasm/caretline.wasm`, driven by `public/js/engine.js` over the state protocol.
  Rebuild after engine changes: `npm run wasm` (needs `rustup target add wasm32-unknown-unknown`).
- **The logo files** in `public/brand/` come from `npm run brand` (`scripts/brand.mjs`), which
  outlines Geist Mono so the SVGs need no font.
- **Tokens** are in `src/styles/tokens.css`: dark first, light under
  `prefers-color-scheme: light` and `[data-theme]`.
