// The docs' order, sidebar titles and one-line descriptions. Slugs are the file names,
// lower-cased; README.md is the docs index.
export const DOCS: { id: string; slug: string; title: string; blurb: string; group: string }[] = [
  { id: 'quickstart', slug: 'quickstart', title: 'Quickstart', blurb: 'Try it in one line: install the binary, then the tour, the scenes and an agent co-editing.', group: 'Start' },
  { id: 'readme', slug: '', title: 'Overview', blurb: 'What caretline is, what it covers, and the layers.', group: 'Start' },
  { id: 'architecture', slug: 'architecture', title: 'Architecture', blurb: 'State, Msg, update, Effect and view; purity, revisions and replay; Helix inside.', group: 'Start' },
  { id: 'api', slug: 'api', title: 'Rust API', blurb: 'The public API by task, with a complete example.', group: 'Use it' },
  { id: 'messages', slug: 'messages', title: 'Messages', blurb: 'Every message, effect and key binding; the key-script notation.', group: 'Use it' },
  { id: 'outline', slug: 'outline', title: 'Outline documents', blurb: 'Lists, tasks and blocks with identity that survives edits.', group: 'Use it' },
  { id: 'cli', slug: 'cli', title: 'The caretline command', blurb: 'The interactive editor and the headless tools, end to end.', group: 'Use it' },
  { id: 'protocol', slug: 'protocol', title: 'State protocol', blurb: 'Drive a headless engine or a live editor over JSON lines.', group: 'Drive it' },
  { id: 'mcp', slug: 'mcp', title: 'MCP server', blurb: 'Let an agent edit a live editor alongside you: guarded writes, its own caret, replay.', group: 'Drive it' },
  { id: 'embedding', slug: 'embedding', title: 'Embedding', blurb: 'Put it in your own app, as a library or a process; licensing.', group: 'Drive it' },
  { id: 'testing', slug: 'testing', title: 'Testing', blurb: 'Goldens, replay, the fuzzer, fixtures, tests from traces.', group: 'Drive it' },
  { id: 'performance', slug: 'performance', title: 'Performance', blurb: 'How fast it is, where the limits are, how to go faster.', group: 'Drive it' },
];
export const docHref = (slug: string) => (slug ? `/docs/${slug}/` : '/docs/');
