import { defineRelations } from 'drizzle-orm'
import * as schema from './schema.ts'

export const relations = defineRelations(schema, (r) => ({
  songs: {
    era: r.one.eras({
      from: r.songs.eraId,
      to: r.eras.id,
    }),
  },
  eras: {
    songs: r.many.songs(),
  },
}))
