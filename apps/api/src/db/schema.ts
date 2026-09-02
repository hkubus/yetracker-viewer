import { integer, real, sqliteTable, text } from 'drizzle-orm/sqlite-core';
export const songsTable = sqliteTable('songs', {
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
});
export const erasTable = sqliteTable('eras', {
  id: integer('id').primaryKey(),
  name: text('name'),
  notes: text('notes'),
  imageUrl: text('image_url'),
  description: text('description'),
  dominantColor: text('dominant_color'),
  isMain: integer('is_main').notNull().default(1),
});
export const filesTable = sqliteTable('files', {
  url: text('url').primaryKey(),
  downloaded: integer('downloaded'),
  filename: text('filename'),
  duration: real('duration'),
});
