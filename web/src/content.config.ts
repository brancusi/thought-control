import { defineCollection, z } from 'astro:content';
import { glob } from 'astro/loaders';

// Guide pages, feature groups and use cases are Markdown, rendered by the site's own layouts.
export const collections = {
  docs: defineCollection({
    loader: glob({ pattern: '**/*.{md,mdx}', base: './src/content/docs' }),
    schema: z.object({ title: z.string(), description: z.string(), order: z.number(), group: z.string().default('Guide') }),
  }),
  features: defineCollection({
    loader: glob({ pattern: '**/*.{md,mdx}', base: './src/content/features' }),
    schema: z.object({ title: z.string(), blurb: z.string(), icon: z.string(), order: z.number() }),
  }),
  usecases: defineCollection({
    loader: glob({ pattern: '**/*.{md,mdx}', base: './src/content/usecases' }),
    schema: z.object({ title: z.string(), who: z.string(), blurb: z.string(), icon: z.string(), order: z.number() }),
  }),
};
