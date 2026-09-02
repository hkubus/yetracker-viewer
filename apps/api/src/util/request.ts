import { HTTPException } from 'hono/http-exception';

export function positiveInteger(value: string | undefined, label: string) {
  if (!value || !/^\d+$/.test(value)) {
    throw new HTTPException(400, { message: `Invalid ${label}` });
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 1) {
    throw new HTTPException(400, { message: `Invalid ${label}` });
  }
  return parsed;
}

export function paginationValue(
  value: string | undefined,
  fallback: number,
  maximum: number,
  label: 'limit' | 'offset',
) {
  if (value === undefined || value === '') return fallback;
  if (!/^\d+$/.test(value)) throw new HTTPException(400, { message: `Invalid ${label}` });

  const parsed = Number(value);
  const minimum = label === 'limit' ? 1 : 0;
  if (!Number.isSafeInteger(parsed) || parsed < minimum) {
    throw new HTTPException(400, { message: `Invalid ${label}` });
  }
  return Math.min(parsed, maximum);
}
