import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
// The shipped version, from the repo's VERSION file (read at build time).
export const version = (() => {
  try { return readFileSync(resolve(process.cwd(), '../VERSION'), 'utf8').trim(); } catch { return ''; }
})();
export const INSTALL = 'curl -fsSL https://github.com/brancusi/thought-central-releases/releases/latest/download/install.sh | sh';
export const RELEASES = 'https://github.com/brancusi/thought-central-releases/releases';
