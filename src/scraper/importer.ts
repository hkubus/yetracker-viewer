import { writeFileSync } from 'node:fs';
import type { drizzle } from 'drizzle-orm/node-sqlite';
import { parse } from 'node-html-parser';
import { erasTable, filesTable, songsTable } from '../db/schema.ts';
import type { AvailableLength, Quality, Song } from '../types.ts';

export async function importData(db: ReturnType<typeof drizzle>) {
  db.delete(songsTable).execute();
  db.delete(erasTable).execute();

  const data = await fetch('https://yetracker.net/htmlview/sheet?headers=true&gid=34972268');
  const text = await data.text();
  const parsed = parse(text);
  const rows = parsed.querySelectorAll('tr');
  const erasIds: Record<string, number> = {};
  const songs: Song[] = [];
  const urls: { url: string; filename: string }[] = [];
  let lastEraId: number = 1;
  rows.forEach((row, index) => {
    const cells = row.querySelectorAll('td');
    if (index <= 2) return;
    switch (cells.length) {
      // era
      case 5: {
        const eraData: Record<string, string | number> = {};
        cells.forEach((cell, index) => {
          switch (index) {
            case 1:
              eraData.name = cell.textContent.trim().split('\n')[0];
              break;
            case 2:
              eraData.notes = cell.textContent.trim();
              break;
            case 3: {
              let image = cell.querySelector('img')?.getAttribute('src') || '';
              image = image.split('=').slice(0, -1).join('=');
              eraData.imageUrl = image;
              break;
            }
            case 4:
              eraData.description = cell.textContent.trim();
              break;
          }
        });
        if (!eraData.name) return;
        if (erasIds[eraData.name]) {
          eraData.id = erasIds[eraData.name];
        } else {
          eraData.id = lastEraId;
          erasIds[eraData.name] = eraData.id;
          lastEraId++;
        }
        // console.log(eraData);
        db.insert(erasTable).values(eraData).execute();
        break;
      }
      // song
      case 9: {
        const songData: Song = {};
        songData.id = index - 3;

        for (let i = 0; i < cells.length; i++) {
          const cell = cells[i];
          switch (i) {
            case 0: {
              let name = cell.textContent.trim();
              if (name === 'Travis Scott Collaboration') name = 'Collaboration with Travis Scott';
              if (erasIds[name]) {
                songData.eraId = erasIds[name];
              } else {
                continue;
                // erasIds[name] = lastEraId;
                // console.log(cell.parentNode.innerHTML);
                // songData.eraId = lastEraId;
                // lastEraId++;
              }
              break;
            }
            case 1:
              songData.name = cell.textContent.trim();
              break;
            case 2:
              songData.notes = cell.textContent.trim();
              break;
            case 3: {
              const length = cell.textContent.trim().split(':');
              songData.trackLength = parseInt(length[0], 10) * 60 + parseInt(length[1], 10);
              if (Number.isNaN(songData.trackLength)) songData.trackLength = 0;
              break;
            }
            case 4:
              songData.fileDate = new Date(cell.textContent.trim()).getTime() / 1000;
              if (Number.isNaN(songData.fileDate)) songData.fileDate = 0;
              break;
            case 5:
              songData.leakDate = new Date(cell.textContent.trim()).getTime() / 1000;
              if (Number.isNaN(songData.leakDate)) songData.leakDate = 0;
              break;
            case 6:
              songData.availableLength = cell.textContent.trim() as AvailableLength;
              break;
            case 7:
              songData.quality = cell.textContent.trim() as Quality;
              break;
            case 8: {
              try {
                const urls = cell.textContent
                  .trim()
                  .split('\n')
                  .map((e) => new URL(e));

                songData.url = (urls.find((e) => e.hostname === 'pillows.su') || urls[0]).toString();
                break;
              } catch {
                songData.url = urls[0]?.toString();
              }
            }
          }
        }
        if (songData.url && songData.name) {
          if (songData.url === 'N/A' || songData.url === 'Link Needed' || songData.quality === 'Not Available') return;
          urls.push({ url: songData.url, filename: `${songData.id} - ${songData.name.replace('\n', '')}` });
        }
        songs.push(songData);
        break;
      }
    }
  });
  writeFileSync(`./data.json`, songs.map((song) => JSON.stringify(song)).join('\n'));
  for (let i = 0; i < urls.length; i += 1000) {
    db.insert(filesTable)
      .values(urls.slice(i, i + 1000))
      .onConflictDoNothing()
      .execute();
  }
  for (let i = 0; i < songs.length; i += 1000) {
    db.insert(songsTable)
      .values(songs.slice(i, i + 1000))
      .execute();
  }
}
