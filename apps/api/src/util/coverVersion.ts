import { createHash } from 'node:crypto';

export function getCoverVersion(imageUrl: string | null | undefined) {
  return createHash('sha1')
    .update(imageUrl ?? '')
    .digest('hex')
    .slice(0, 12);
}
