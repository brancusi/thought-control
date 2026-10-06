// Base-path-safe internal links: u('features/') → '/features/' at the root, '/thc/features/' under BASE=/thc/.
const base = import.meta.env.BASE_URL.endsWith('/') ? import.meta.env.BASE_URL : import.meta.env.BASE_URL + '/';
export const u = (path = '') => base + path.replace(/^\//, '');
