import { index, integer, real, sqliteTable, text } from 'drizzle-orm/sqlite-core';
export const songsTable = sqliteTable(
  'songs',
  {
    id: integer('id').primaryKey(),
    eraId: integer('era'),
    catalogId: text('catalog_id').notNull().default('unreleased'),
    name: text('name'),
    notes: text('notes'),
    fileDate: integer('file_date'),
    leakDate: integer('leak_date'),
    availableLength: text('available_length'),
    trackLength: integer('track_length'),
    quality: text('quality'),
    url: text('url'),
  },
  (table) => [
    index('songs_catalog_era_id_idx').on(table.catalogId, table.eraId, table.id),
    index('songs_catalog_id_idx').on(table.catalogId, table.id),
    index('songs_quality_idx').on(table.quality),
    index('songs_available_length_idx').on(table.availableLength),
  ],
);
export const erasTable = sqliteTable(
  'eras',
  {
    id: integer('id').primaryKey(),
    name: text('name'),
    notes: text('notes'),
    imageUrl: text('image_url'),
    description: text('description'),
    dominantColor: text('dominant_color'),
    isMain: integer('is_main').notNull().default(1),
  },
  (table) => [index('eras_is_main_idx').on(table.isMain)],
);
export const filesTable = sqliteTable('files', {
  url: text('url').primaryKey(),
  downloaded: integer('downloaded').notNull().default(0),
  filename: text('filename'),
  duration: real('duration'),
});
