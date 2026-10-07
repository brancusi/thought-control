// @ts-check
import { defineConfig } from 'astro/config';
import rehypeDocLinks from './src/lib/rehype-doc-links.mjs';

// caretline's public site: a landing page, the docs (read from ../../docs/caretline at build
// time) and the brand page. Static output in dist/, served by Cloudflare (wrangler.jsonc).
export default defineConfig({
  site: 'https://caretline.app',
  trailingSlash: 'ignore',
  build: { format: 'directory' },
  markdown: {
    syntaxHighlight: { type: 'shiki', excludeLangs: ['mermaid'] },
    shikiConfig: { theme: 'css-variables', wrap: false },
    rehypePlugins: [rehypeDocLinks],
  },
});
