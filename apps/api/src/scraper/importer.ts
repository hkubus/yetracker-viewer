import { createHash } from 'node:crypto';
import type { AvailableLength, Quality, Song } from '@yetracker/types';
import { inArray } from 'drizzle-orm';
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

const VALID_AVAILABLE_LENGTHS: ReadonlySet<string> = new Set([
  'Full',
  'Snippet',
  'Confirmed',
  'Beat Only',
  'Partial',
  'Tagged',
  'OG File',
  'Stem Bounce',
  'Rumored',
  'Conflicting Sources',
]);

const VALID_QUALITIES: ReadonlySet<string> = new Set([
  'Low Quality',
  'High Quality',
  'CD Quality',
  'Lossless',
  'Not Available',
  'Recording',
]);

// Query-string keys stripped from era artwork URLs. Everything else is kept
// verbatim so legitimate size/format params survive.
const TRACKING_PARAMS = new Set(['utm_source', 'utm_medium', 'utm_campaign', 'utm_term', 'utm_content', 'utm_id']);

const CATALOG_FETCH_TIMEOUT_MS = 30_000;

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

function parseDuration(value: string): number | null {
  const trimmed = value.trim();
  if (!trimmed) return null;
  const rawParts = trimmed.split(':');
  if (rawParts.length < 2 || rawParts.length > 3) return null;
  const parts = rawParts.map((part) => Number(part));
  if (parts.some((part) => !Number.isInteger(part) || part < 0)) return null;

  if (parts.length === 2) {
    const [minutes, seconds] = parts;
    if (seconds >= 60) return null;
    return minutes * 60 + seconds;
  }
  const [hours, minutes, seconds] = parts;
  if (minutes >= 60 || seconds >= 60) return null;
  return hours * 60 * 60 + minutes * 60 + seconds;
}

function parseDate(value: string) {
  if (!value) return 0;
  const timestamp = Date.parse(value);
  return Number.isNaN(timestamp) ? 0 : Math.floor(timestamp / 1000);
}

function parseUrls(value: string) {
  return value.split(/\s+/).flatMap((part) => {
    if (!part.startsWith('https://')) return [];
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

function canDownload(url: string | URL) {
  try {
    const hostname = typeof url === 'string' ? new URL(url).hostname : url.hostname;
    return DOWNLOADABLE_HOSTS.has(hostname);
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

function sanitizeImageUrl(raw: string): string {
  const trimmed = raw.trim();
  if (!trimmed) return '';
  let parsed: URL;
  try {
    parsed = new URL(trimmed);
  } catch {
    return '';
  }
  if (parsed.protocol !== 'https:') return '';
  // Drop only known tracking params; keep legitimate query strings intact.
  let stripped = false;
  for (const key of TRACKING_PARAMS) {
    if (parsed.searchParams.has(key)) {
      parsed.searchParams.delete(key);
      stripped = true;
    }
  }
  void stripped;
  return parsed.toString();
}

function parseAvailableLength(value: string): AvailableLength | null {
  if (!value) return null;
  if (VALID_AVAILABLE_LENGTHS.has(value)) return value as AvailableLength;
  console.warn(`Unknown AvailableLength "${value}", defaulting to null`);
  return null;
}

function parseQuality(value: string): Quality | null {
  if (!value) return null;
  if (VALID_QUALITIES.has(value)) return value as Quality;
  console.warn(`Unknown Quality "${value}", defaulting to null`);
  return null;
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
  const key = normalizedName.toLowerCase();
  const existing = state.eraByName.get(key);
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
  state.eraByName.set(key, era);
  state.eras.push(era);
  if (isMain) state.mainEraCount++;
  return era.id;
}

type RowCells = ReturnType<ReturnType<typeof parse>['querySelectorAll']>;

function* iterTableRowHtml(text: string): Generator<string> {
  // Yield one <tr>…</tr> fragment at a time so huge sheets never need a
  // full-document DOM in memory. Parsing the main catalog in one go retains
  // hundreds of MB of heap that the runtime is slow to hand back to the OS.
  const pattern = /<tr[\s>][\s\S]*?<\/tr\s*>/gi;
  for (const match of text.matchAll(pattern)) {
    yield match[0];
  }
}

function isHeaderRow(cells: RowCells) {
  const headers = cells.map((cell) => normalizeHeader(cell.textContent));
  return (
    headers.some((header) => header === 'era') &&
    headers.some((header) => header === 'name' || header.startsWith('name '))
  );
}

function importCatalog(text: string, catalog: CatalogDefinition, state: ImportState) {
  let headers: string[] | null = null;
  let eraColumn = -1;
  let nameColumn = -1;
  let notesColumn = -1;
  let trackLengthColumn = -1;
  let fileDateColumn = -1;
  let leakDateColumn = -1;
  let availableLengthColumn = -1;
  let qualityColumn = -1;
  let linkColumn = -1;
  let typeColumn = -1;
  let streamingColumn = -1;
  let importedSongs = 0;

  for (const rowHtml of iterTableRowHtml(text)) {
    const cells = parse(rowHtml).querySelectorAll('td, th');

    if (headers === null) {
      if (!isHeaderRow(cells)) continue;
      headers = cells.map((cell) => normalizeHeader(cell.textContent));
      eraColumn = findColumn(headers, (header) => header === 'era' || header === 'main era');
      nameColumn = findColumn(headers, (header) => header === 'name' || header.startsWith('name '));
      notesColumn = findColumn(headers, (header) => header === 'notes');
      trackLengthColumn = findColumn(
        headers,
        (header) =>
          header === 'track length' || header === 'length' || header === 'full length' || header === 'copy length',
      );
      fileDateColumn = findColumn(
        headers,
        (header) => header === 'file date' || header === 'date made' || header === 'release date',
      );
      leakDateColumn = findColumn(headers, (header) => header === 'leak date');
      availableLengthColumn = findColumn(headers, (header) => header === 'available length');
      qualityColumn = findColumn(headers, (header) => header === 'quality');
      linkColumn = findColumn(headers, (header) => header.startsWith('link'));
      typeColumn = findColumn(headers, (header) => header === 'type');
      streamingColumn = findColumn(headers, (header) => header === 'streaming');
      continue;
    }

    if (catalog.id === PRIMARY_CATALOG_ID && cells.length === 5) {
      const name = normalizeEraName((cells[1]?.textContent ?? '').split(/\r?\n/)[0] ?? '');
      if (!name) continue;

      const rawImageUrl = cells[3]?.querySelector('img')?.getAttribute('src') ?? '';
      const imageUrl = sanitizeImageUrl(rawImageUrl);

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
    if (!eraName || !songName || eraName.toLowerCase() === 'era') continue;

    const notes = readCell(cells, notesColumn);
    const catalogId = catalog.id;
    if (catalogId === PRIMARY_CATALOG_ID) {
      const duplicateKey = `${songName.toLowerCase()}\u0000${notes.toLowerCase()}\u0000${eraName.toLowerCase()}`;
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
      trackLength: parseDuration(readCell(cells, trackLengthColumn)) ?? undefined,
      fileDate: parseDate(readCell(cells, fileDateColumn)),
      leakDate: parseDate(readCell(cells, leakDateColumn)),
      availableLength: parseAvailableLength(readCell(cells, availableLengthColumn)) ?? undefined,
      quality: parseQuality(readCell(cells, qualityColumn)) ?? undefined,
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

  if (headers === null) {
    throw new Error(`Catalog ${catalog.name} did not contain a recognizable header row`);
  }

  return importedSongs;
}

async function fetchCatalogText(catalog: CatalogDefinition): Promise<string> {
  const url = `https://yetracker.net/htmlview/sheet?headers=true&gid=${catalog.gid}`;
  let delayMs = 500;
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      const response = await fetch(url, { signal: AbortSignal.timeout(CATALOG_FETCH_TIMEOUT_MS) });
      if (!response.ok) {
        throw new Error(`Failed to fetch ${catalog.name} catalog: ${response.status} ${response.statusText}`);
      }
      return await response.text();
    } catch (error) {
      if (attempt === 2) throw error;
      console.warn(`Retrying catalog ${catalog.name} after fetch failure (attempt ${attempt + 1})`, error);
      await new Promise((resolve) => setTimeout(resolve, delayMs + Math.random() * 250));
      delayMs *= 2;
    }
  }
  throw new Error(`Failed to fetch ${catalog.name} catalog after retry`);
}

async function limitedMap<T, R>(items: T[], limit: number, fn: (item: T, index: number) => Promise<R>): Promise<R[]> {
  const results = new Array<R>(items.length);
  let nextIndex = 0;
  const workers = Array.from({ length: Math.min(Math.max(limit, 1), items.length) }, async () => {
    while (nextIndex < items.length) {
      const index = nextIndex++;
      results[index] = await fn(items[index], index);
    }
  });
  await Promise.all(workers);
  return results;
}

export async function importData(db: ReturnType<typeof drizzle>) {
  const state = createImportState();

  const catalogTexts = await limitedMap(CATALOGS, 4, async (catalog) => {
    try {
      return { catalog, text: await fetchCatalogText(catalog) };
    } catch (error) {
      console.error(`skipping catalog ${catalog.name} after fetch/import failure`, error);
      return { catalog, text: null as string | null };
    }
  });
  for (const { catalog, text } of catalogTexts) {
    if (!text) continue;
    try {
      const importedSongs = importCatalog(text, catalog, state);
      console.log(`imported ${catalog.name}: ${importedSongs} songs (${catalogSourceUrl(catalog.gid)})`);
    } catch (error) {
      console.error(`skipping catalog ${catalog.name} after import failure`, error);
    }
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
  const liveUrls = new Set(uniqueUrls.map((file) => file.url));
  // Read outside the write transaction so the write lock is held briefly.
  const existingFiles = await db.select({ url: filesTable.url }).from(filesTable);
  const staleUrls = existingFiles
    .map((row) => row.url)
    .filter((url): url is string => typeof url === 'string' && !liveUrls.has(url));
  db.transaction((tx) => {
    tx.delete(songsTable).run();
    tx.delete(erasTable).run();
    // Drop files rows whose URL no longer appears in any song, in batches
    // so the statement stays under SQLite's variable limit.
    for (let i = 0; i < staleUrls.length; i += 500) {
      tx.delete(filesTable)
        .where(inArray(filesTable.url, staleUrls.slice(i, i + 500)))
        .run();
    }
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
