import { defineCollection, z } from 'astro:content';
import { glob } from 'astro/loaders';

// The docs are the repo's own docs/caretline/*.md, read in place at build time: the site never
// keeps a copy. Links between them are rewritten by src/lib/rehype-doc-links.mjs.
export const collections = {
  docs: defineCollection({
    loader: glob({ pattern: '*.md', base: '../../docs/caretline' }),
    schema: z.object({}).passthrough(),
  }),
};
