type SearchableSong = {
  id: number;
  name: string | null;
  notes: string | null;
  quality: string | null;
  availableLength: string | null;
  eraName: string | null;
  playable: boolean;
};

const CATEGORY_PRIORITY = new Map([
  ['⭐', 0],
  ['✨', 1],
  ['🏅', 2],
  ['🗑️', 3],
  ['🤖', 4],
]);

function normalize(value: string | null) {
  return (value ?? '').trim().replaceAll(/\s+/g, ' ').toLocaleLowerCase();
}

function stripCategoryMarkers(value: string | null) {
  let title = (value ?? '').trimStart();
  let categoryPriority = CATEGORY_PRIORITY.size;
  let foundMarker = true;

  while (foundMarker) {
    foundMarker = false;
    for (const [marker, priority] of CATEGORY_PRIORITY) {
      if (!title.startsWith(marker)) continue;
      categoryPriority = Math.min(categoryPriority, priority);
      foundMarker = true;
      title = title.slice(marker.length).trimStart();
      break;
    }
  }

  return { title, categoryPriority };
}

function splitParentheticalText(value: string | null) {
  let depth = 0;
  let outside = '';
  let inside = '';

  for (const character of value ?? '') {
    if (character === '(') {
      depth += 1;
      continue;
    }
    if (character === ')') {
      depth = Math.max(0, depth - 1);
      continue;
    }
    if (depth > 0) inside += character;
    else outside += character;
  }

  return { outside: normalize(outside), inside: normalize(inside) };
}

function fieldScore(value: string, query: string, base: number) {
  const position = value.indexOf(query);
  if (position === -1) return Number.POSITIVE_INFINITY;

  const before = position === 0 ? '' : value[position - 1];
  const after = value[position + query.length] ?? '';
  const startsAtWord = position === 0 || !/[\p{L}\p{N}]/u.test(before);
  const endsAtWord = !after || !/[\p{L}\p{N}]/u.test(after);
  const lengthDifference = Math.max(0, value.length - query.length);
  const closeness = Math.min(position, 99) / 100 + Math.min(lengthDifference, 999) / 100_000;

  if (value === query) return base;
  if (position === 0) return base + 10 + closeness;
  if (startsAtWord && endsAtWord) return base + 20 + closeness;
  if (startsAtWord) return base + 30 + closeness;
  return base + 40 + closeness;
}

function relevanceScore(song: SearchableSong, query: string, titleWithoutMarkers: string) {
  const title = normalize(titleWithoutMarkers);
  if (title === query) return 0;

  const { outside, inside } = splitParentheticalText(titleWithoutMarkers);
  const outsideScore = fieldScore(outside, query, 0);
  if (Number.isFinite(outsideScore)) return outsideScore;
  const insideScore = fieldScore(inside, query, 1_000);
  if (Number.isFinite(insideScore)) return insideScore;
  const titleScore = fieldScore(title, query, 1_500);
  if (Number.isFinite(titleScore)) return titleScore;
  const eraScore = fieldScore(normalize(song.eraName), query, 2_000);
  if (Number.isFinite(eraScore)) return eraScore;
  const notesScore = fieldScore(normalize(song.notes), query, 3_000);
  if (Number.isFinite(notesScore)) return notesScore;
  const qualityScore = fieldScore(normalize(song.quality), query, 4_000);
  if (Number.isFinite(qualityScore)) return qualityScore;
  const availabilityScore = fieldScore(normalize(song.availableLength), query, 4_100);
  if (Number.isFinite(availabilityScore)) return availabilityScore;
  return Number.POSITIVE_INFINITY;
}

export function rankSongSearch<T extends SearchableSong>(songs: T[], query: string, limit: number = songs.length) {
  const rankedSongs = songs.map((song) => {
    const { title, categoryPriority } = stripCategoryMarkers(song.name);
    return {
      song,
      score: relevanceScore(song, query, title),
      categoryPriority,
      normalizedTitle: undefined as string | undefined,
    };
  });

  const compareRankedSongs = (left: (typeof rankedSongs)[number], right: (typeof rankedSongs)[number]) => {
    const relevanceGroupDifference = Math.trunc(left.score) - Math.trunc(right.score);
    if (relevanceGroupDifference !== 0) return relevanceGroupDifference;

    if (left.categoryPriority !== right.categoryPriority) {
      return left.categoryPriority - right.categoryPriority;
    }

    if (left.song.playable !== right.song.playable) return left.song.playable ? -1 : 1;

    const closenessDifference = left.score - right.score;
    if (closenessDifference !== 0) return closenessDifference;

    left.normalizedTitle ??= normalize(left.song.name);
    right.normalizedTitle ??= normalize(right.song.name);
    const titleDifference = left.normalizedTitle.localeCompare(right.normalizedTitle);
    if (titleDifference !== 0) return titleDifference;
    return left.song.id - right.song.id;
  };

  if (limit < rankedSongs.length) {
    const heap: typeof rankedSongs = [];

    function moveUp(index: number) {
      while (index > 0) {
        const parentIndex = Math.floor((index - 1) / 2);
        if (compareRankedSongs(heap[parentIndex], heap[index]) >= 0) break;
        [heap[parentIndex], heap[index]] = [heap[index], heap[parentIndex]];
        index = parentIndex;
      }
    }

    function moveDown(index: number) {
      while (true) {
        const leftIndex = index * 2 + 1;
        if (leftIndex >= heap.length) return;
        const rightIndex = leftIndex + 1;
        const worseChildIndex =
          rightIndex < heap.length && compareRankedSongs(heap[rightIndex], heap[leftIndex]) > 0
            ? rightIndex
            : leftIndex;
        if (compareRankedSongs(heap[worseChildIndex], heap[index]) <= 0) return;
        [heap[index], heap[worseChildIndex]] = [heap[worseChildIndex], heap[index]];
        index = worseChildIndex;
      }
    }

    for (const rankedSong of rankedSongs) {
      if (heap.length < limit) {
        heap.push(rankedSong);
        moveUp(heap.length - 1);
      } else if (compareRankedSongs(rankedSong, heap[0]) < 0) {
        heap[0] = rankedSong;
        moveDown(0);
      }
    }

    heap.sort(compareRankedSongs);
    return heap.map(({ song }) => song);
  }

  rankedSongs.sort(compareRankedSongs);

  return rankedSongs.map(({ song }) => song);
}
