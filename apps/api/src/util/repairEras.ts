import { asc, eq } from 'drizzle-orm';
import type { drizzle } from 'drizzle-orm/node-sqlite';
import { erasTable, songsTable } from '../db/schema.ts';

type EraRow = typeof erasTable.$inferSelect;

function normalizeName(value: string | null) {
  return (value ?? '').trim().replace(/\s+/g, ' ');
}

function nameKey(value: string | null) {
  return normalizeName(value).toLocaleLowerCase();
}

function removeTrailingParenthetical(value: string) {
  const match = /\s+\([^()]*\)\s*$/.exec(value);
  return match?.index === undefined ? null : value.slice(0, match.index).trim();
}

function chooseEra(eras: EraRow[], primarySongCounts: Map<number, number>) {
  return [...eras].sort((left, right) => {
    const mainDifference = Number(right.isMain) - Number(left.isMain);
    if (mainDifference !== 0) return mainDifference;

    const songDifference = (primarySongCounts.get(right.id) ?? 0) - (primarySongCounts.get(left.id) ?? 0);
    if (songDifference !== 0) return songDifference;

    const nameDifference = normalizeName(left.name).length - normalizeName(right.name).length;
    if (nameDifference !== 0) return nameDifference;
    return left.id - right.id;
  })[0];
}

function resolveEraId(eraId: number, mergeTargets: Map<number, number>) {
  let currentId = eraId;
  const seen = new Set<number>();
  while (mergeTargets.has(currentId) && !seen.has(currentId)) {
    seen.add(currentId);
    const nextId = mergeTargets.get(currentId);
    if (nextId === undefined) break;
    currentId = nextId;
  }
  return currentId;
}

/**
 * Repairs databases written by the importer versions that treated era aliases
 * as separate rows. This is intentionally conservative: annotated rows are
 * merged only when they have no primary-catalog songs and a shorter exact era
 * name already exists.
 */
export async function repairEraDuplicates(db: ReturnType<typeof drizzle>) {
  const eras = await db.select().from(erasTable).orderBy(asc(erasTable.id));
  if (eras.length < 2) return;

  const songs = await db.select({ eraId: songsTable.eraId, catalogId: songsTable.catalogId }).from(songsTable);
  const primarySongCounts = new Map<number, number>();
  for (const song of songs) {
    if (song.catalogId !== 'unreleased' || song.eraId === null) continue;
    primarySongCounts.set(song.eraId, (primarySongCounts.get(song.eraId) ?? 0) + 1);
  }

  const erasByName = new Map<string, EraRow[]>();
  for (const era of eras) {
    const key = nameKey(era.name);
    if (!key) continue;
    const group = erasByName.get(key) ?? [];
    group.push(era);
    erasByName.set(key, group);
  }

  const mergeTargets = new Map<number, number>();
  for (const group of erasByName.values()) {
    if (group.length < 2) continue;
    const chosen = chooseEra(group, primarySongCounts);
    for (const era of group) {
      if (era.id !== chosen.id) mergeTargets.set(era.id, chosen.id);
    }
  }

  for (const era of eras) {
    if (mergeTargets.has(era.id) || (primarySongCounts.get(era.id) ?? 0) > 0) continue;

    let candidate = normalizeName(era.name);
    while (candidate) {
      candidate = removeTrailingParenthetical(candidate) ?? '';
      if (!candidate) break;
      const matchingEra = erasByName.get(nameKey(candidate))?.[0];
      if (matchingEra && matchingEra.id !== era.id) {
        mergeTargets.set(era.id, resolveEraId(matchingEra.id, mergeTargets));
        break;
      }
    }
  }

  const resolvedTargets = new Map(
    [...mergeTargets.entries()]
      .map(([fromId, toId]) => [fromId, resolveEraId(toId, mergeTargets)] as const)
      .filter(([fromId, toId]) => fromId !== toId),
  );
  if (resolvedTargets.size === 0) return;

  db.transaction((tx) => {
    for (const [fromId, toId] of resolvedTargets) {
      tx.update(songsTable).set({ eraId: toId }).where(eq(songsTable.eraId, fromId)).run();
      tx.delete(erasTable).where(eq(erasTable.id, fromId)).run();
    }
  });

  console.log(`merged ${resolvedTargets.size} duplicate era rows`);
}
