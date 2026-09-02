export const PRIMARY_CATALOG_ID = 'unreleased';

export const CATALOGS = [
  {
    id: PRIMARY_CATALOG_ID,
    name: 'Unreleased',
    gid: '34972268',
    description: 'The main Ye Tracker era catalog.',
  },
  {
    id: 'released',
    name: 'Released',
    gid: '762588265',
    description: 'Released songs, features, and production credits.',
  },
  {
    id: 'recent',
    name: 'Recent',
    gid: '77894385',
    description: 'Recently added songs and updates from the tracker.',
  },
  {
    id: 'best-of',
    name: 'Best Of',
    gid: '787540803',
    description: 'Standout songs selected from the tracker.',
  },
  {
    id: 'worst-of',
    name: 'Worst Of',
    gid: '1371492190',
    description: 'The tracker’s collection of infamous and lowlight songs.',
  },
  {
    id: 'special',
    name: 'Special',
    gid: '812818104',
    description: 'Songs with unusual history, versions, or context.',
  },
  {
    id: 'grails-wanted',
    name: 'Grails / Wanted',
    gid: '1948929917',
    description: 'Highly sought-after songs and recordings.',
  },
  {
    id: 'stems',
    name: 'Stems',
    gid: '495336364',
    description: 'Available stems, instrumentals, and stem bounces.',
  },
  {
    id: 'album-copies',
    name: 'Album Copies',
    gid: '1297512832',
    description: 'Complete album and demo-tape copies.',
    mainPageSection: true,
  },
  {
    id: 'ssc',
    name: 'Sunday Service Choir',
    gid: '1333371598',
    description: 'Sunday Service Choir recordings and performances.',
  },
  {
    id: 'fakes',
    name: 'Fakes',
    gid: '61838480',
    description: 'Documented fake leaks, rumors, and misattributions.',
    downloadable: false,
  },
] as const;

export type CatalogDefinition = (typeof CATALOGS)[number];

export function getCatalog(id: string) {
  return CATALOGS.find((catalog) => catalog.id === id);
}

export function getCategoryCatalogs() {
  return CATALOGS.filter((catalog) => catalog.id !== PRIMARY_CATALOG_ID && !('mainPageSection' in catalog));
}

export function catalogSourceUrl(gid: string) {
  return `https://yetracker.net/#gid=${gid}`;
}
