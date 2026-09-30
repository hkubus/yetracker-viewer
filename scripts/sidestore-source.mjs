#!/usr/bin/env node
// Writes the SideStore/AltStore source for the iOS app: every published GitHub release tagged `ios-v<version>` that
// has a `YeTracker.ipa` asset becomes one version, newest first. Run by .github/workflows/ios.yml, which deploys the
// output directory to GitHub Pages; locally: `GITHUB_REPOSITORY=owner/repo node scripts/sidestore-source.mjs _site`.
//
// Env: GITHUB_REPOSITORY (required), GITHUB_TOKEN (optional, raises the API rate limit), GITHUB_API_URL.
import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

const TAG_PREFIX = 'ios-v';
const IPA_NAME = 'YeTracker.ipa';
const IOS = new URL('../apps/ios/', import.meta.url);

const repository = process.env.GITHUB_REPOSITORY;
if (!repository?.includes('/')) throw new Error('GITHUB_REPOSITORY must be owner/repo');
const [owner, repo] = repository.split('/');
const outDir = process.argv[2] ?? '_site';
const api = process.env.GITHUB_API_URL ?? 'https://api.github.com';
const pagesUrl = `https://${owner.toLowerCase()}.github.io/${repo}`;

/** `KEY = value` from an xcconfig (the first plain assignment). */
function xcconfigValue(text, key) {
  const match = text.match(new RegExp(`^${key}\\s*=\\s*(.+?)\\s*$`, 'm'));
  if (!match) throw new Error(`${key} is missing from Shared.xcconfig`);
  return match[1];
}

/** `#D7D7D7` from an asset catalog colour set. */
function colorHex(colorSet) {
  const { components } = colorSet.colors[0].color;
  const channel = (value) => {
    const number = value.startsWith('0x') ? Number.parseInt(value, 16) : Math.round(Number(value) * 255);
    return number.toString(16).padStart(2, '0').toUpperCase();
  };
  return `#${channel(components.red)}${channel(components.green)}${channel(components.blue)}`;
}

async function releases() {
  const headers = { accept: 'application/vnd.github+json', 'x-github-api-version': '2022-11-28' };
  if (process.env.GITHUB_TOKEN) headers.authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
  const all = [];
  for (let page = 1; ; page += 1) {
    const response = await fetch(`${api}/repos/${owner}/${repo}/releases?per_page=100&page=${page}`, { headers });
    if (!response.ok) throw new Error(`GitHub releases: HTTP ${response.status} ${await response.text()}`);
    const batch = await response.json();
    all.push(...batch);
    if (batch.length < 100) return all;
  }
}

const xcconfig = await readFile(new URL('Config/Shared.xcconfig', IOS), 'utf8');
const project = await readFile(new URL('YeTracker.xcodeproj/project.pbxproj', IOS), 'utf8');
const plist = await readFile(new URL('Config/Info.plist', IOS), 'utf8');
const accent = JSON.parse(
  await readFile(new URL('YeTracker/Assets.xcassets/AccentColor.colorset/Contents.json', IOS), 'utf8'),
);

const bundleIdentifier = xcconfigValue(xcconfig, 'PRODUCT_BUNDLE_IDENTIFIER');
const minOSVersion = project.match(/IPHONEOS_DEPLOYMENT_TARGET = ([\d.]+);/)?.[1] ?? '17.0';
const localNetwork = plist.match(/<key>NSLocalNetworkUsageDescription<\/key>\s*<string>([^<]*)<\/string>/)?.[1];
const tintColor = colorHex(accent);
const iconURL = `${pagesUrl}/icon.png`;

const versions = (await releases())
  .filter((release) => !release.draft && !release.prerelease && release.tag_name.startsWith(TAG_PREFIX))
  .map((release) => ({ release, ipa: release.assets.find((asset) => asset.name === IPA_NAME) }))
  .filter(({ ipa }) => ipa)
  .sort((a, b) => Date.parse(b.release.published_at) - Date.parse(a.release.published_at))
  .map(({ release, ipa }) => {
    const version = release.tag_name.slice(TAG_PREFIX.length);
    return {
      version,
      // The workflow builds releases with CFBundleVersion = CFBundleShortVersionString.
      buildVersion: version,
      date: release.published_at,
      localizedDescription: release.body?.trim() || `Ye Tracker ${version}`,
      downloadURL: ipa.browser_download_url,
      size: ipa.size,
      minOSVersion,
    };
  });

const description =
  'Browse the YeTracker catalog of unreleased songs by era, search it, and listen with a background player with ' +
  'lock-screen controls. Needs a YeTracker API server: set its URL in Settings → Server.';

const source = {
  name: 'Ye Tracker',
  identifier: `io.github.${owner.toLowerCase()}.${repo}`,
  subtitle: 'The YeTracker viewer for iOS',
  description,
  iconURL,
  website: `https://github.com/${owner}/${repo}`,
  tintColor,
  apps: [
    {
      name: 'Ye Tracker',
      bundleIdentifier,
      developerName: owner,
      subtitle: 'Browse and play the YeTracker catalog',
      localizedDescription: description,
      iconURL,
      tintColor,
      category: 'entertainment',
      screenshots: [],
      versions,
      appPermissions: {
        entitlements: [],
        privacy: localNetwork ? { NSLocalNetworkUsageDescription: localNetwork } : {},
      },
    },
  ],
  news: [],
};

await mkdir(outDir, { recursive: true });
await writeFile(join(outDir, 'sidestore.json'), `${JSON.stringify(source, null, 2)}\n`);
await copyFile(new URL('YeTracker/Assets.xcassets/AppIcon.appiconset/AppIcon.png', IOS), join(outDir, 'icon.png'));
const landing = `<!doctype html>
<meta charset="utf-8">
<title>Ye Tracker for iOS</title>
<h1>Ye Tracker for iOS</h1>
<p>Add this source in SideStore (Sources → +): <code>${pagesUrl}/sidestore.json</code></p>
<p><a href="sidestore://source?url=${encodeURIComponent(`${pagesUrl}/sidestore.json`)}">Add to SideStore</a></p>
`;
await writeFile(join(outDir, 'index.html'), landing);
console.log(`${outDir}/sidestore.json: ${versions.length} version(s) of ${bundleIdentifier}`);
