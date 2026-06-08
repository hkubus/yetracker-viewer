import { defineRelations } from 'drizzle-orm';
import * as schema from './schema.ts';

export const relations = defineRelations(schema, (r) => ({
  songsTable: {
    erasTable: r.one.erasTable({
      from: r.songsTable.eraId,
      to: r.erasTable.id,
    }),
    filesTable: r.one.filesTable({
      from: r.songsTable.url,
      to: r.filesTable.url,
    }),
  },
  filesTable: {
    songsTable: r.one.songsTable({
      from: r.filesTable.url,
      to: r.songsTable.url,
    }),
  },
  erasTable: {
    songsTable: r.many.songsTable(),
  },
}));
