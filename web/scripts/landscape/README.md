# The landscape board's research kit

The board at `/landscape/` reads one JSON dossier per tool from `src/content/landscape/`. To refresh
it or add a tool:

1. Give a research agent `dossier-spec.md` (the exact schema, the 0–5 rubric for every axis, and the
   rules: primary sources, steelman each tool, never invent numbers) and a list of tools. Have it
   write `<id>.json` files to a scratch folder.
2. Import them: `python3 scripts/landscape/import.py <scratch>/dossiers src/content/landscape`
   (normalises verdicts and applies known corrections).
3. `npm run build`, and check the board and the new pages.

Axes, groups and verdicts live in `src/lib/landscape.ts`; change the rubric there and in the spec
together. thc-scene's own dossier (`thc-scene.json`) is written by hand, to the same rules, with
`planned_scores` for what's designed but not built.
