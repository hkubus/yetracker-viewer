import { writeFileSync } from 'fs';
import { parse } from 'node-html-parser';
const data = await fetch('https://yetracker.net/htmlview/sheet?headers=true&gid=34972268');
const text = await data.text();
const parsed = parse(text);
const rows = parsed.querySelectorAll('tr');
const eras: Record<number, string> = {};
const songs = [];
let lastEraId: number | undefined = 0;

rows.forEach((row, index) => {
  const cells = row.querySelectorAll('td');
  if (index <= 2) return;
  switch (cells.length) {
    // song
    case 9:
      const songData: Song = {};
      cells.forEach((cell, index) => {
        switch (index) {
          // era
          case 0:
            if (eras[cell.textContent.trim()]) {
              songData.era = eras[cell.textContent.trim()];
            } else {
              lastEraId++;
              eras[cell.textContent.trim()] = lastEraId;
              songData.era = lastEraId;
            }
            break;
          // title
          case 1:
            songData.title = cell.textContent.trim();
            break;
          // notes
          case 2:
            songData.notes = cell.textContent.trim();
            break;
          // track length
          case 3:
            songData.trackLength = parseInt(cell.textContent.trim());
            break;
          // file date
          case 4:
            songData.fileDate = new Date(cell.textContent.trim());
            break;
          // leak date
          case 5:
            songData.leakDate = new Date(cell.textContent.trim());
            break;
          // available length
          case 6:
            songData.availableLength = cell.textContent.trim() as AvailableLength;
            break;
          // quality
          case 7:
            songData.quality = cell.textContent.trim() as Quality
            break;
        }
      })
      songs.push(songData);
      break;
  }
});

writeFileSync(`./data.txt`, songs.map(song => JSON.stringify(song)).join('\n'));
