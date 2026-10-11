// The demo link: a name, a date, the one line that runs it, and a copy button. One markup for
// every page (components/DemoLink.astro). The copy button is wired by Base.astro (any [data-copy]).
// A pinned demo is a frozen script from scripts/freeze-demo.sh: try/<slug>.sh.
const SITE = 'https://thoughtcontrol.app';
const esc = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
const day = (d) => new Date(d).toLocaleDateString('en-GB', { day: 'numeric', month: 'short', year: 'numeric', timeZone: 'UTC' });

/** name, date (ISO), script (a path on the site: try.sh, try/slug.sh), commit (pinned: a full
 * commit, or a release tag like v0.10.1) or latest, needs (what a reader must have installed) */
export function demoHtml({ name, date, script, commit = '', latest = false, needs = 'git, and Rust or a Mac', compact = false }) {
  const cmd = `curl -fsSL ${SITE}/${script.replace(/^\//, '')} | sh`;
  const version = latest
    ? `<span class="dl-tag dl-latest">latest</span>`
    : /^v\d/.test(commit)
      ? `<span class="dl-tag">pinned · <a href="https://github.com/brancusi/thought-control-releases/releases/tag/${esc(commit)}" rel="noopener">${esc(commit)}</a></span>`
      : commit
        ? `<span class="dl-tag">pinned · <a href="https://github.com/brancusi/thought-control/commit/${esc(commit)}" rel="noopener">${esc(commit.slice(0, 7))}</a></span>`
        : '';
  return `<div class="demo-link${compact ? ' dl-compact' : ''}" role="group" aria-label="${esc(`Demo: ${name}`)}">
<div class="dl-head"><span class="dl-label">Try it in one line</span><b class="dl-name">${esc(name)}</b>${date ? `<time class="dl-date" datetime="${esc(date)}">${esc(day(date))}</time>` : ''}${version}</div>
<div class="dl-cmd"><code><span class="d">$ </span>${esc(cmd)}</code><button type="button" data-copy="${esc(cmd)}" aria-label="Copy the demo command">copy</button></div>
<p class="dl-foot">Made-up notes in a sandbox under <code>~/.thc-try</code>; your own vaults, config and daemon aren't touched. Needs ${esc(needs)}.</p>
</div>`;
}
