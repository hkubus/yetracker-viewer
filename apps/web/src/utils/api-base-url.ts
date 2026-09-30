/**
 * The browser-facing API base URL for client scripts. The server decides it at runtime (`PUBLIC_API_URL`) and the
 * layout publishes it as `<meta name="yt-api-url" content="…">`, so it is never baked into the bundles.
 * Client-safe: no server imports.
 */

/** `name` of the `<meta>` element that carries the API base URL. */
export const API_URL_META_NAME = 'yt-api-url';

/**
 * The API base URL from the page (`https://api.example.com`, or a same-origin prefix like `/api`), without a
 * trailing slash. Empty when the page has no such meta element.
 */
export function getApiBaseUrl(root: ParentNode = document): string {
  const meta = root.querySelector<HTMLMetaElement>(`meta[name="${API_URL_META_NAME}"]`);
  return (meta?.content ?? '').trim().replace(/\/+$/, '');
}

/** Absolute or same-origin URL of an API path: `apiEndpoint('/songs?q=x')` → `https://api.example.com/songs?q=x`. */
export function apiEndpoint(path: string, root?: ParentNode): string {
  return `${getApiBaseUrl(root)}${path.startsWith('/') ? path : `/${path}`}`;
}
