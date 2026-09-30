//! Merges era rows that share a normalized name, left behind by importer
//! versions that treated era aliases as separate rows. v2 keys eras by that
//! normalized name (`eras.key`, unique), so the schema upgrade runs this before
//! building the unique index; afterwards duplicates cannot arise.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::error::ApiError;
use crate::importer;
use crate::text;

struct EraRow {
    id: i64,
    key: String,
    is_main: i64,
}

/// Merges eras whose names normalize to the same key into one row (main eras
/// first, then the one with the most songs, then the lowest id) and moves
/// their songs over. Returns the number of rows removed. Eras are never
/// deleted for any other reason here: an era without songs is legitimate.
///
/// Runs in the caller's transaction (the schema upgrade has one open).
pub fn merge_duplicate_eras(conn: &Connection) -> Result<usize, ApiError> {
    let eras: Vec<EraRow> = {
        let mut statement = conn.prepare("SELECT id, name, is_main FROM eras ORDER BY id ASC")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?;
        let mut eras = Vec::new();
        for row in rows {
            let (id, name, is_main) = row?;
            let name = text::clean_line(name.as_deref().unwrap_or_default());
            if name.is_empty() {
                continue;
            }
            eras.push(EraRow {
                id,
                key: importer::era_key(&name),
                is_main: is_main.unwrap_or(1),
            });
        }
        eras
    };

    let mut groups: HashMap<&str, Vec<&EraRow>> = HashMap::new();
    for era in &eras {
        groups.entry(era.key.as_str()).or_default().push(era);
    }
    if groups.values().all(|group| group.len() < 2) {
        return Ok(0);
    }

    let song_counts: HashMap<i64, i64> = {
        let mut statement =
            conn.prepare("SELECT era, count(*) FROM songs WHERE era IS NOT NULL GROUP BY era")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };

    let mut merged = 0;
    for group in groups.values().filter(|group| group.len() > 1) {
        let survivor = group
            .iter()
            .max_by(|left, right| {
                left.is_main
                    .cmp(&right.is_main)
                    .then_with(|| {
                        let left_songs = song_counts.get(&left.id).copied().unwrap_or(0);
                        let right_songs = song_counts.get(&right.id).copied().unwrap_or(0);
                        left_songs.cmp(&right_songs)
                    })
                    .then_with(|| right.id.cmp(&left.id))
            })
            .expect("groups are non-empty");
        for era in group.iter().filter(|era| era.id != survivor.id) {
            conn.execute(
                "UPDATE songs SET era = ?1 WHERE era = ?2",
                [survivor.id, era.id],
            )?;
            conn.execute(
                "UPDATE eras SET is_main = max(is_main, (SELECT is_main FROM eras WHERE id = ?2)) \
                 WHERE id = ?1",
                [survivor.id, era.id],
            )?;
            conn.execute("DELETE FROM eras WHERE id = ?1", [era.id])?;
            merged += 1;
        }
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE eras (id INTEGER PRIMARY KEY, name TEXT, is_main INTEGER NOT NULL DEFAULT 1);
             CREATE TABLE songs (id INTEGER PRIMARY KEY, era INTEGER);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn merges_same_name_eras_into_the_main_one() {
        let conn = database();
        conn.execute_batch(
            "INSERT INTO eras VALUES (1, 'Yandhi [V1]', 0), (2, 'yandhi  [v1]', 1), (3, 'Other', 1);
             INSERT INTO songs VALUES (10, 1), (11, 2), (12, 3);",
        )
        .unwrap();
        assert_eq!(merge_duplicate_eras(&conn).unwrap(), 1);
        let eras: Vec<i64> = conn
            .prepare("SELECT id FROM eras ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(eras, [2, 3]);
        let era_of_10: i64 = conn
            .query_row("SELECT era FROM songs WHERE id = 10", [], |row| row.get(0))
            .unwrap();
        assert_eq!(era_of_10, 2);
        assert_eq!(merge_duplicate_eras(&conn).unwrap(), 0);
    }

    #[test]
    fn keeps_eras_without_songs() {
        let conn = database();
        conn.execute_batch(
            "INSERT INTO eras VALUES (1, 'Good Ass Job', 1), (2, 'Good Ass Job (2018)', 1);
             INSERT INTO songs VALUES (10, 1);",
        )
        .unwrap();
        assert_eq!(merge_duplicate_eras(&conn).unwrap(), 0);
        let count: i64 = conn
            .query_row("SELECT count(*) FROM eras", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}
