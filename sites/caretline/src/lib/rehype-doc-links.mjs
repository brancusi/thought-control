// Rewrites the links in docs/caretline/*.md so they work on the site:
//   architecture.md#x      → /docs/architecture/#x
//   README.md              → /docs/
//   ../../crates/…         → the file or folder on GitHub (main)
// and marks external links. Runs on every Markdown file the site renders.
const REPO = 'https://github.com/brancusi/thought-control';

function rewrite(href) {
  if (!href || /^[a-z]+:/i.test(href) || href.startsWith('#') || href.startsWith('/')) return href;
  const [path, hash = ''] = href.split('#');
  const frag = hash ? `#${hash}` : '';
  const doc = path.match(/^(?:\.\/)?([A-Za-z0-9_-]+)\.md$/);
  if (doc) {
    const slug = doc[1].toLowerCase();
    return (slug === 'readme' ? '/docs/' : `/docs/${slug}/`) + frag;
  }
  if (path.startsWith('../../')) {
    const rel = path.slice(6).replace(/\/$/, '');
    const isFile = /\.[A-Za-z0-9]+$/.test(rel.split('/').pop() || '');
    return `${REPO}/${isFile ? 'blob' : 'tree'}/main/${rel}${frag}`;
  }
  return href;
}

export default function rehypeDocLinks() {
  return (tree) => {
    const walk = (node) => {
      if (node.type === 'element' && node.tagName === 'a' && node.properties && typeof node.properties.href === 'string') {
        node.properties.href = rewrite(node.properties.href);
        if (/^https?:/.test(node.properties.href)) node.properties.rel = ['noopener'];
      }
      if (node.children) node.children.forEach(walk);
    };
    walk(tree);
  };
}
