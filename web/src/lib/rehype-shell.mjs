// Terminal sessions in code blocks, styled the way a terminal shows them: `$ command` lines in
// ink with a muted prompt, `# comments` dim, and everything else (output) muted. No syntax
// colours: one accent is enough. Applies to every fenced block (sh, console, text, none).
const text = (node) => (node.type === 'text' ? node.value : (node.children || []).map(text).join(''));
const span = (cls, children) => ({ type: 'element', tagName: 'span', properties: { className: [cls] }, children });
const t = (value) => ({ type: 'text', value });

function lineNodes(line) {
  const m = line.match(/^(\s*)\$ (.*)$/);
  if (m) {
    // A trailing "  # comment" on a command line is dimmed.
    const c = m[2].match(/^(.*?)(\s+# .*)$/);
    return span('l-cmd', [t(m[1]), span('l-ps', [t('$ ')]), t(c ? c[1] : m[2]), ...(c ? [span('l-com', [t(c[2])])] : [])]);
  }
  if (/^\s*#/.test(line)) return span('l-com', [t(line)]);
  return span('l-out', [t(line)]);
}

export default function rehypeShell() {
  const walk = (node) => {
    if (node.type === 'element' && node.tagName === 'pre') {
      const code = (node.children || []).find((c) => c.type === 'element' && c.tagName === 'code');
      if (code) {
        const lang = ((code.properties && code.properties.className) || []).join(' ');
        const raw = text(code).replace(/\n$/, '');
        const lines = raw.split('\n');
        const session = lines.some((l) => /^\s*\$ /.test(l));
        if (session || /language-(sh|bash|shell|console|zsh)/.test(lang)) {
          code.children = lines.flatMap((l, i) => (i ? [t('\n'), lineNodes(l)] : [lineNodes(l)]));
          node.properties = { ...(node.properties || {}), className: ['shell'] };
        }
      }
      return;
    }
    (node.children || []).forEach(walk);
  };
  return (tree) => walk(tree);
}
