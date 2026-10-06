// @ts-check
import { defineConfig } from 'astro/config';
import mdx from '@astrojs/mdx';
import rehypeShell from './src/lib/rehype-shell.mjs';

// thc's public site. Static output in dist/. Set BASE (e.g. BASE=/thc/) to serve under a sub-path;
// every internal link goes through src/lib/url.ts, so nothing assumes the domain root.
export default defineConfig({
  site: 'https://thoughtcontrol.app',
  integrations: [mdx({ syntaxHighlight: false, rehypePlugins: [rehypeShell] })],
  base: process.env.BASE || '/',
  trailingSlash: 'ignore',
  build: { format: 'directory' },
  markdown: { syntaxHighlight: false, rehypePlugins: [rehypeShell] },
});
