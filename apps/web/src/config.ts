function trimTrailingSlash(value: string) {
  return value.replace(/\/+$/, '');
}

export const ERA_PAGE_SIZE = 100;

const FETCH_TIMEOUT_MS = 10_000;

export const publicApiBaseUrl = trimTrailingSlash(
  process.env.PUBLIC_API_URL ?? import.meta.env.PUBLIC_API_URL ?? 'http://localhost:3000',
);

const configuredInternalUrl = process.env.API_INTERNAL_URL ?? import.meta.env.API_INTERNAL_URL ?? publicApiBaseUrl;

function isAbsoluteHttpUrl(value: string) {
  return value.startsWith('http://') || value.startsWith('https://');
}

function isProd() {
  return process.env.NODE_ENV === 'production' || (typeof import.meta.env.PROD === 'boolean' && import.meta.env.PROD);
}

function resolveInternalApiBaseUrl() {
  if (isAbsoluteHttpUrl(configuredInternalUrl)) {
    return trimTrailingSlash(configuredInternalUrl);
  }
  if (isProd()) {
    throw new Error(
      `API_INTERNAL_URL must be an absolute http(s) URL in production, got: ${JSON.stringify(configuredInternalUrl)}. ` +
        'Set API_INTERNAL_URL (e.g. http://127.0.0.1:3000) so SSR fetches do not go through the public URL.',
    );
  }
  return 'http://127.0.0.1:3000';
}

export const internalApiBaseUrl = resolveInternalApiBaseUrl();

export function apiUrl(baseUrl: string, path: string) {
  const base = baseUrl.endsWith('/') ? baseUrl.slice(0, -1) : baseUrl;
  const cleanPath = path.startsWith('/') ? path.slice(1) : path;
  return `${base}/${cleanPath}`;
}

export type ApiError = Error & { status?: number };

function toApiError(status: number, path: string): ApiError {
  const message = status === 404 ? `API resource not found: ${path}` : `API request failed with status ${status}`;
  return Object.assign(new Error(message), { status });
}

/**
 * SSR fetch helper. Backward compatible: still resolves to a `Response`
 * (callers may read headers / call `.json()`), but now with a timeout,
 * a UA header, and a `status` field on thrown errors so pages can
 * return 404 instead of 500.
 */
const STATIC_HEADERS = {
  Accept: 'application/json',
  'User-Agent': 'yetracker-viewer/1.0',
} as const;

export async function fetchApi(path: string, init?: RequestInit): Promise<Response> {
  const url = apiUrl(internalApiBaseUrl, path);
  let response: Response;
  try {
    response = await fetch(url, {
      ...init,
      headers: {
        ...STATIC_HEADERS,
        ...init?.headers,
      },
      signal: init?.signal ?? AbortSignal.timeout(FETCH_TIMEOUT_MS),
    });
  } catch (error) {
    throw Object.assign(new Error(`API request failed for ${path}: ${(error as Error)?.message ?? error}`), {
      cause: error,
    });
  }
  if (!response.ok) {
    throw toApiError(response.status, path);
  }
  return response;
}
