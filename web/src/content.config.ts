import { defineCollection, z } from 'astro:content';
import { glob } from 'astro/loaders';

// Guide pages, feature groups and use cases are Markdown, rendered by the site's own layouts.
// The landscape is data: one JSON dossier per tool (src/content/landscape/<id>.json), the same
// shape for every tool (see src/lib/landscape.ts), so the board, the map and the dossier pages
// all read one source.
export const collections = {
  docs: defineCollection({
    loader: glob({ pattern: '**/*.{md,mdx}', base: './src/content/docs' }),
    schema: z.object({ title: z.string(), description: z.string(), order: z.number(), group: z.string().default('Guide') }),
  }),
  features: defineCollection({
    loader: glob({ pattern: '**/*.{md,mdx}', base: './src/content/features' }),
    schema: z.object({ title: z.string(), blurb: z.string(), icon: z.string(), order: z.number() }),
  }),
  landscape: defineCollection({
    loader: glob({ pattern: '*.json', base: './src/content/landscape' }),
    schema: z.object({
      id: z.string(),
      name: z.string(),
      group: z.string(),
      tagline: z.string(),
      scores: z.record(z.string(), z.number()),
    }).passthrough(),
  }),
  usecases: defineCollection({
    loader: glob({ pattern: '**/*.{md,mdx}', base: './src/content/usecases' }),
    schema: z.object({ title: z.string(), who: z.string(), blurb: z.string(), icon: z.string(), order: z.number() }),
  }),
};
