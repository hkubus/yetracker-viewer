import {  sqliteTable, integer, text } from 'drizzle-orm/sqlite-core'
export const songs = sqliteTable('song_entries', {
  id: integer("id").primaryKey(),
  eraId: integer("era"),
  name: text("name"),
  notes: text("notes"),
  availableLength: integer("available_length"),
  quality: text("quality"),
  url: text("url"),
});
export const eras = sqliteTable('eras', {
  id: integer("id").primaryKey(),
  name: text("name"),
  notes: text("notes"),
  description: text("description"),
});
