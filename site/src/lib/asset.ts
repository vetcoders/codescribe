// Resolve a public/ asset path against the configured base path.
// astro.config `base: '/codescribe'` makes import.meta.env.BASE_URL === '/codescribe/'.
// A hardcoded '/shots/x.webp' would 404 in production — always route through here.
export function asset(path: string): string {
  const base = import.meta.env.BASE_URL; // e.g. "/codescribe/"
  return base.replace(/\/$/, "") + "/" + path.replace(/^\//, "");
}
