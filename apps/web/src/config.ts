/**
 * Server-side configuration and the SSR API client. Server-only: it reads `process.env` at runtime, so client
 * scripts must not import it (they read the API URL from the page, see `utils/api-base-url.ts`).
 */
import { readRuntimeEnv } from '../env.mjs';
import { isLocalNetworkHost } from './utils/local-network';

export const ERA_PAGE_SIZE = 100;

/** SSR requests to the API give up after this long, so a hung API turns into a 503 page instead of a hung page. */
export const API_TIMEOUT_MS = 5_000;

/** `Retry-After` (seconds) sent with pages that could not be rendered because the API is unavailable. */
export const OUTAGE_RETRY_AFTER_SECONDS = 30;

/**
 * Edge/browser cache policy of successfully rendered pages. The middleware applies it to 200 HTML responses that did
 * not set their own `Cache-Control` (see `noStore`).
 */
export const PAGE_CACHE_CONTROL = 'public, max-age=60, s-maxage=300, stale-while-revalidate=600';

/**
 * Internal response header through which the error pages ask the middleware for a different status: Astro always
 * sends the `/500` page with status 500, but an API outage deserves a 503. The middleware applies and removes it.
 */
export const ERROR_STATUS_HEADER = 'X-Error-Status';

// Validated once. `server.mjs` runs the same validation before the app loads, so in production a bad value stops
// the process at startup instead of failing here on the first request.
const runtimeEnv = readRuntimeEnv(process.env, { production: import.meta.env.PROD });

/** Browser-facing API base URL (`PUBLIC_API_URL`): absolute, or a same-origin prefix such as `/api`. */
export const publicApiBaseUrl = runtimeEnv.publicApiBaseUrl;

/** Absolute API base URL for server-side requests (`API_INTERNAL_URL`). */
export const internalApiBaseUrl = runtimeEnv.internalApiBaseUrl;

/**
 * The site's public origin (`SITE_URL`), used for canonical, Open Graph and sitemap URLs; `null` when unset (absolute
 * URLs then come from the request, see `requestOrigin`).
 */
export const siteUrl = runtimeEnv.siteUrl;

/** Whether `X-Forwarded-Proto` / `X-Forwarded-Host` are trusted (`TRUST_PROXY`). */
export const trustProxy = runtimeEnv.trustProxy;

/** Origin of the browser-facing API when it is absolute (for the CSP and preconnect); `null` when same-origin. */
export const publicApiOrigin = /^https?:\/\//.test(publicApiBaseUrl) ? new URL(publicApiBaseUrl).origin : null;

export function apiUrl(baseUrl: string, path: string) {
  const base = baseUrl.endsWith('/') ? baseUrl.slice(0, -1) : baseUrl;
  const cleanPath = path.startsWith('/') ? path.slice(1) : path;
  return `${base}/${cleanPath}`;
}

/**
 * Marks the current response as uncacheable. The middleware only applies its public cache policy when a page has
 * not set `Cache-Control` itself, so a page that rendered without some of its data can opt out of edge caching.
 */
export function noStore(response: { headers: Headers }): void {
  response.headers.set('Cache-Control', 'no-store');
}

function firstHeaderValue(value: string | null): string | undefined {
  const first = value?.split(',')[0]?.trim();
  return first ? first : undefined;
}

/** The scheme a trusted proxy reports the client used (`X-Forwarded-Proto`), if any. */
function forwardedProtocol(headers: Headers): 'http' | 'https' | undefined {
  if (!trustProxy) return undefined;
  const proto = firstHeaderValue(headers.get('x-forwarded-proto'))?.toLowerCase();
  return proto === 'http' || proto === 'https' ? proto : undefined;
}

/**
 * Whether the client reached the site over HTTPS: a TLS connection to this server, or a trusted TLS-terminating proxy
 * saying so (`X-Forwarded-Proto: https` with `TRUST_PROXY`). Decides HSTS and `upgrade-insecure-requests`, so a page
 * opened over plain HTTP (an internal address, a proxy without TLS) keeps loading its own scripts.
 */
export function isSecureRequest(request: Request): boolean {
  const forwarded = forwardedProtocol(request.headers);
  if (forwarded) return forwarded === 'https';
  return new URL(request.url).protocol === 'https:';
}

/** A plain `host[:port]` (name, IPv4 or bracketed IPv6): no credentials, path or anything else. */
const HOST_HEADER = /^(?:[a-z0-9-]+(?:\.[a-z0-9-]+)*\.?|\[[0-9a-f:.]+\])(?::\d{1,5})?$/i;

/**
 * Public origin for absolute URLs (sitemap, robots.txt, Open Graph images): `SITE_URL` when set, else the origin the
 * request came in on — with `TRUST_PROXY`, the scheme and host the proxy reports. Without `SITE_URL` the result comes
 * from request headers: responses that embed it must not be stored by shared caches (see
 * `originDependentCacheControl`).
 */
export function requestOrigin(request: Request): string {
  if (siteUrl) return siteUrl;
  const url = new URL(request.url);
  const protocol = forwardedProtocol(request.headers);
  const host = trustProxy ? firstHeaderValue(request.headers.get('x-forwarded-host')) : undefined;
  if (!protocol && !host) return url.origin;
  const candidate = host && HOST_HEADER.test(host) ? host : url.host;
  try {
    return new URL(`${protocol ?? url.protocol.slice(0, -1)}://${candidate}`).origin;
  } catch {
    return url.origin;
  }
}

/**
 * `Cache-Control` for a response whose body embeds `requestOrigin()`: `publicPolicy` when `SITE_URL` is set. Without
 * it the origin comes from request headers, so only the browser may keep the response: another client's `Host`
 * header must never be served from a shared cache.
 */
export function originDependentCacheControl(publicPolicy: string, maxAgeSeconds: number): string {
  return siteUrl ? publicPolicy : `private, max-age=${maxAgeSeconds}`;
}

/** The browser-facing API base URL as an absolute URL (resolving a same-origin prefix against the request). */
export function absolutePublicApiBaseUrl(request: Request): string {
  return publicApiOrigin ? publicApiBaseUrl : `${requestOrigin(request)}${publicApiBaseUrl}`;
}

/** A failed API request. `status` is the HTTP status, or `undefined` when no response arrived. */
export class ApiError extends Error {
  readonly status: number | undefined;
  readonly path: string;

  constructor(message: string, path: string, status?: number, options?: ErrorOptions) {
    super(message, options);
    this.name = new.target.name;
    this.path = path;
    this.status = status;
  }
}

/** 400: the API rejected a parameter. `message` is the API's explanation (e.g. "Search query is too long"). */
export class ApiBadRequestError extends ApiError {}

/** 404: the resource does not exist. */
export class ApiNotFoundError extends ApiError {}

/** The API answered with a server error (5xx), an unexpected status, or a body that could not be read. */
export class ApiServerError extends ApiError {
  /** The API's `Retry-After` in seconds, when it sent one. */
  readonly retryAfter: number | undefined;

  constructor(message: string, path: string, status?: number, options?: ErrorOptions & { retryAfter?: number }) {
    super(message, path, status, options);
    this.retryAfter = options?.retryAfter;
  }
}

/** No response: the API could not be reached, or did not answer within `API_TIMEOUT_MS`. */
export class ApiUnreachableError extends ApiError {
  readonly timedOut: boolean;

  constructor(message: string, path: string, timedOut: boolean, options?: ErrorOptions) {
    super(message, path, undefined, options);
    this.timedOut = timedOut;
  }
}

/** Errors that mean "the catalog is unavailable right now" (render a 503 page) rather than a bad request. */
export function isApiOutage(error: unknown): error is ApiServerError | ApiUnreachableError {
  return error instanceof ApiServerError || error instanceof ApiUnreachableError;
}

function isTimeout(error: unknown): boolean {
  return error instanceof Error && error.name === 'TimeoutError';
}

function parseRetryAfter(value: string | null): number | undefined {
  if (value === null || !/^\d{1,6}$/.test(value.trim())) return undefined;
  return Number(value.trim());
}

async function readErrorText(response: Response): Promise<string> {
  try {
    return (await response.text()).trim().slice(0, 300);
  } catch {
    return '';
  }
}

async function toApiError(response: Response, path: string): Promise<ApiError> {
  const { status } = response;
  if (status === 400) {
    const text = await readErrorText(response);
    return new ApiBadRequestError(text || 'Bad request', path, status);
  }
  if (status === 404) {
    void response.body?.cancel().catch(() => {});
    return new ApiNotFoundError(`API resource not found: ${path}`, path, status);
  }
  void response.body?.cancel().catch(() => {});
  return new ApiServerError(`API request failed with status ${status}: ${path}`, path, status, {
    retryAfter: parseRetryAfter(response.headers.get('retry-after')),
  });
}

const STATIC_HEADERS = {
  Accept: 'application/json',
  'User-Agent': 'yetracker-viewer/1.0',
  // Compression only costs CPU on both ends when the API is this close; a remote API still compresses.
  ...(isLocalNetworkHost(new URL(internalApiBaseUrl).hostname) ? { 'Accept-Encoding': 'identity' } : {}),
} as const;

/**
 * SSR request to the API (`API_INTERNAL_URL`). Resolves to the `Response` for 2xx answers (callers read headers
 * and the body; reading the body is covered by the same timeout). Otherwise it throws an `ApiError` subclass:
 * `ApiBadRequestError` (400), `ApiNotFoundError` (404), `ApiServerError` (5xx and other statuses) or
 * `ApiUnreachableError` (network error or timeout after `API_TIMEOUT_MS`). An abort through `init.signal` is
 * re-thrown as is.
 */
export async function fetchApi(path: string, init?: RequestInit): Promise<Response> {
  const url = apiUrl(internalApiBaseUrl, path);
  const timeout = AbortSignal.timeout(API_TIMEOUT_MS);
  const signal = init?.signal ? AbortSignal.any([init.signal, timeout]) : timeout;
  let response: Response;
  try {
    response = await fetch(url, {
      ...init,
      headers: {
        ...STATIC_HEADERS,
        ...init?.headers,
      },
      signal,
    });
  } catch (error) {
    if (init?.signal?.aborted) throw error;
    const timedOut = isTimeout(error);
    const reason = timedOut ? `no answer within ${API_TIMEOUT_MS} ms` : ((error as Error)?.message ?? String(error));
    throw new ApiUnreachableError(`API request failed for ${path}: ${reason}`, path, timedOut, { cause: error });
  }
  if (!response.ok) {
    throw await toApiError(response, path);
  }
  return response;
}

/**
 * `fetchApi` plus JSON parsing. A body that is not valid JSON (or cannot be read in time) throws
 * `ApiServerError` / `ApiUnreachableError` like any other outage.
 */
export async function fetchJson<T>(path: string, init?: RequestInit): Promise<{ data: T; response: Response }> {
  const response = await fetchApi(path, init);
  try {
    return { data: (await response.json()) as T, response };
  } catch (error) {
    if (init?.signal?.aborted) throw error;
    if (isTimeout(error)) {
      throw new ApiUnreachableError(`API response for ${path} did not arrive in time`, path, true, { cause: error });
    }
    throw new ApiServerError(`API response for ${path} is not valid JSON`, path, response.status, { cause: error });
  }
}
