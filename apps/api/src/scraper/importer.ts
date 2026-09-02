import { createHash } from 'node:crypto';
import type { AvailableLength, Quality, Song } from '@yetracker/types';
import type { drizzle } from 'drizzle-orm/node-sqlite';
import { parse } from 'node-html-parser';
import { CATALOGS, type CatalogDefinition, catalogSourceUrl, PRIMARY_CATALOG_ID } from '../catalogs.ts';
import { erasTable, filesTable, songsTable } from '../db/schema.ts';

type EraRecord = {
  id: number;
  name: string;
  notes: string;
  imageUrl: string;
  description: string;
  dominantColor: string | null;
  isMain: number;
};

type ImportedSong = Song & {
  catalogId: string;
  id: number;
};

type ImportState = {
  eras: EraRecord[];
  songs: ImportedSong[];
  urls: { url: string; filename: string }[];
  eraByName: Map<string, EraRecord>;
  seenMainSongs: Set<string>;
  nextEraId: number;
  nextSongId: number;
  mainEraCount: number;
  mainSongCount: number;
};

const DOWNLOADABLE_HOSTS = new Set(['pillows.su', 'youtu.be', 'www.youtube.com', 'www.instagram.com', 'twitter.com']);

function normalizeText(value: string) {
  return value.trim().replace(/\s+/g, ' ');
}

function normalizeHeader(value: string) {
  return normalizeText(value).toLocaleLowerCase();
}

function normalizeEraKey(value: string) {
  return normalizeEraName(value).toLocaleLowerCase();
}

function findColumn(headers: string[], predicate: (header: string) => boolean) {
  return headers.findIndex(predicate);
}

function readCell(cells: ReturnType<ReturnType<typeof parse>['querySelectorAll']>, index: number) {
  return index >= 0 ? normalizeText(cells[index]?.textContent ?? '') : '';
}

function parseDuration(value: string) {
  const parts = value.split(':').map((part) => Number(part));
  if (parts.length < 2 || parts.some((part) => !Number.isFinite(part))) return 0;

  if (parts.length === 2) return parts[0] * 60 + parts[1];
  if (parts.length === 3) return parts[0] * 60 * 60 + parts[1] * 60 + parts[2];
  return 0;
}

function parseDate(value: string) {
  if (!value) return 0;
  const timestamp = Date.parse(value);
  return Number.isNaN(timestamp) ? 0 : timestamp / 1000;
}

function parseUrls(value: string) {
  return value.split(/\s+/).flatMap((part) => {
    try {
      const url = new URL(part);
      return url.protocol === 'https:' ? [url] : [];
    } catch {
      return [];
    }
  });
}

function getSourceUrl(value: string) {
  const urls = parseUrls(value);
  return (urls.find((url) => url.hostname === 'pillows.su') ?? urls[0])?.toString();
}

function canDownload(url: string) {
  try {
    return DOWNLOADABLE_HOSTS.has(new URL(url).hostname);
  } catch {
    return false;
  }
}

function canImportFiles(catalog: CatalogDefinition) {
  return !('downloadable' in catalog) || catalog.downloadable !== false;
}

function normalizeEraName(value: string) {
  const normalized = normalizeText(value);
  return normalized === 'Travis Scott Collaboration' ? 'Collaboration with Travis Scott' : normalized;
}

function createImportState(): ImportState {
  return {
    eras: [],
    songs: [],
    urls: [],
    eraByName: new Map(),
    seenMainSongs: new Set(),
    nextEraId: 1,
    nextSongId: 1,
    mainEraCount: 0,
    mainSongCount: 0,
  };
}

function ensureEra(state: ImportState, name: string, isMain: boolean, metadata?: Partial<EraRecord>) {
  const normalizedName = normalizeEraName(name);
  const existing = state.eraByName.get(normalizeEraKey(normalizedName));
  if (existing) {
    if (isMain) existing.isMain = 1;
    return existing.id;
  }

  const era: EraRecord = {
    id: state.nextEraId++,
    name: normalizedName,
    notes: metadata?.notes ?? '',
    imageUrl: metadata?.imageUrl ?? '',
    description: metadata?.description ?? '',
    dominantColor: metadata?.dominantColor ?? null,
    isMain: isMain ? 1 : 0,
  };
  state.eraByName.set(normalizeEraKey(normalizedName), era);
  state.eras.push(era);
  if (isMain) state.mainEraCount++;
  return era.id;
}

function findHeaderRow(rows: ReturnType<ReturnType<typeof parse>['querySelectorAll']>) {
  return rows.findIndex((row) => {
    const headers = row.querySelectorAll('td').map((cell) => normalizeHeader(cell.textContent));
    return (
      headers.some((header) => header === 'era') &&
      headers.some((header) => header === 'name' || header.startsWith('name '))
    );
  });
}

function importCatalog(text: string, catalog: CatalogDefinition, state: ImportState) {
  const rows = parse(text).querySelectorAll('tr');
  const headerRowIndex = findHeaderRow(rows);
  if (headerRowIndex < 0) {
    throw new Error(`Catalog ${catalog.name} did not contain a recognizable header row`);
  }

  const headerCells = rows[headerRowIndex].querySelectorAll('td');
  const headers = headerCells.map((cell) => normalizeHeader(cell.textContent));
  const eraColumn = findColumn(headers, (header) => header === 'era' || header === 'main era');
  const nameColumn = findColumn(headers, (header) => header === 'name' || header.startsWith('name '));
  const notesColumn = findColumn(headers, (header) => header === 'notes');
  const trackLengthColumn = findColumn(
    headers,
    (header) =>
      header === 'track length' || header === 'length' || header === 'full length' || header === 'copy length',
  );
  const fileDateColumn = findColumn(
    headers,
    (header) => header === 'file date' || header === 'date made' || header === 'release date',
  );
  const leakDateColumn = findColumn(headers, (header) => header === 'leak date');
  const availableLengthColumn = findColumn(headers, (header) => header === 'available length');
  const qualityColumn = findColumn(headers, (header) => header === 'quality');
  const linkColumn = findColumn(headers, (header) => header.startsWith('link'));
  const typeColumn = findColumn(headers, (header) => header === 'type');
  const streamingColumn = findColumn(headers, (header) => header === 'streaming');
  let importedSongs = 0;

  for (let rowIndex = headerRowIndex + 1; rowIndex < rows.length; rowIndex++) {
    const cells = rows[rowIndex].querySelectorAll('td');

    if (catalog.id === PRIMARY_CATALOG_ID && cells.length === 5) {
      const name = normalizeEraName((cells[1]?.textContent ?? '').split(/\r?\n/)[0] ?? '');
      if (!name) continue;

      let imageUrl = cells[3]?.querySelector('img')?.getAttribute('src') ?? '';
      if (imageUrl.includes('=')) imageUrl = imageUrl.split('=').slice(0, -1).join('=');
      try {
        const parsedImageUrl = new URL(imageUrl);
        imageUrl = parsedImageUrl.protocol === 'https:' ? parsedImageUrl.toString() : '';
      } catch {
        imageUrl = '';
      }

      ensureEra(state, name, true, {
        notes: normalizeText(cells[2]?.textContent ?? ''),
        imageUrl,
        description: normalizeText(cells[4]?.textContent ?? ''),
      });
      continue;
    }

    if (cells.length !== headers.length || eraColumn < 0 || nameColumn < 0) continue;

    const eraName = normalizeEraName(readCell(cells, eraColumn));
    const songName = readCell(cells, nameColumn);
    if (!eraName || !songName || eraName.toLocaleLowerCase() === 'era') continue;

    const notes = readCell(cells, notesColumn);
    const catalogId = catalog.id;
    if (catalogId === PRIMARY_CATALOG_ID) {
      const duplicateKey = `${songName}\u0000${notes}`;
      if (state.seenMainSongs.has(duplicateKey)) continue;
      state.seenMainSongs.add(duplicateKey);
    }

    const eraId = ensureEra(state, eraName, catalogId === PRIMARY_CATALOG_ID);
    const type = readCell(cells, typeColumn);
    const streaming = readCell(cells, streamingColumn);
    const extraNotes = [type && `Type: ${type}`, streaming && `Streaming: ${streaming}`].filter(Boolean).join(' · ');
    const song: ImportedSong = {
      id: state.nextSongId++,
      catalogId,
      eraId,
      name: songName,
      notes: extraNotes ? [notes, extraNotes].filter(Boolean).join('\n') : notes,
      trackLength: parseDuration(readCell(cells, trackLengthColumn)),
      fileDate: parseDate(readCell(cells, fileDateColumn)),
      leakDate: parseDate(readCell(cells, leakDateColumn)),
      availableLength: readCell(cells, availableLengthColumn) as AvailableLength,
      quality: readCell(cells, qualityColumn) as Quality,
      url: getSourceUrl(readCell(cells, linkColumn)),
    };

    if (
      song.url &&
      song.url !== 'N/A' &&
      song.url !== 'Link Needed' &&
      song.quality !== 'Not Available' &&
      canImportFiles(catalog) &&
      canDownload(song.url)
    ) {
      const hash = createHash('sha256').update(song.url).digest('hex');
      state.urls.push({ url: song.url, filename: hash });
    }

    state.songs.push(song);
    importedSongs++;
    if (catalogId === PRIMARY_CATALOG_ID) state.mainSongCount++;
  }

  return importedSongs;
}

export async function importData(db: ReturnType<typeof drizzle>) {
  const state = createImportState();

  for (const catalog of CATALOGS) {
    const response = await fetch(`https://yetracker.net/htmlview/sheet?headers=true&gid=${catalog.gid}`);
    if (!response.ok) {
      throw new Error(`Failed to fetch ${catalog.name} catalog: ${response.status} ${response.statusText}`);
    }

    const importedSongs = importCatalog(await response.text(), catalog, state);
    console.log(`imported ${catalog.name}: ${importedSongs} songs (${catalogSourceUrl(catalog.gid)})`);
  }

  if (state.mainEraCount === 0 || state.mainSongCount === 0) {
    throw new Error('Fetched song catalog did not contain any main eras or songs');
  }

  const existingEras = await db
    .select({ name: erasTable.name, dominantColor: erasTable.dominantColor })
    .from(erasTable);
  const colorsByEraName = new Map(
    existingEras
      .filter((era): era is { name: string; dominantColor: string | null } => Boolean(era.name))
      .map((era) => [normalizeEraKey(era.name), era.dominantColor] as const),
  );
  const eras = state.eras.map((era) => ({
    ...era,
    dominantColor: colorsByEraName.get(normalizeEraKey(era.name)) ?? null,
  }));
  const uniqueUrls = Array.from(new Map(state.urls.map((file) => [file.url, file])).values());
  db.transaction((tx) => {
    tx.delete(songsTable).run();
    tx.delete(erasTable).run();
    for (let i = 0; i < eras.length; i += 1000) {
      tx.insert(erasTable)
        .values(eras.slice(i, i + 1000))
        .onConflictDoNothing()
        .run();
    }
    for (let i = 0; i < uniqueUrls.length; i += 1000) {
      tx.insert(filesTable)
        .values(uniqueUrls.slice(i, i + 1000))
        .onConflictDoNothing()
        .run();
    }
    for (let i = 0; i < state.songs.length; i += 1000) {
      tx.insert(songsTable)
        .values(state.songs.slice(i, i + 1000))
        .onConflictDoNothing()
        .run();
    }
  });
}
