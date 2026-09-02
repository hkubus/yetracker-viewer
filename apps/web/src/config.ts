function trimTrailingSlash(value: string) {
  return value.replace(/\/+$/, '');
}

export const ERA_PAGE_SIZE = 100;

export const publicApiBaseUrl = trimTrailingSlash(
  process.env.PUBLIC_API_URL ?? import.meta.env.PUBLIC_API_URL ?? 'http://localhost:3000',
);

const configuredInternalUrl = process.env.API_INTERNAL_URL ?? import.meta.env.API_INTERNAL_URL ?? publicApiBaseUrl;

export const internalApiBaseUrl =
  configuredInternalUrl.startsWith('http://') || configuredInternalUrl.startsWith('https://')
    ? trimTrailingSlash(configuredInternalUrl)
    : 'http://127.0.0.1:3000';

export function apiUrl(baseUrl: string, path: string) {
  return `${trimTrailingSlash(baseUrl)}/${path.replace(/^\/+/, '')}`;
}

export async function fetchApi(path: string) {
  const response = await fetch(apiUrl(internalApiBaseUrl, path));
  if (!response.ok) {
    throw new Error(`API request failed with status ${response.status}`);
  }
  return response;
}
