//! Startup repair for databases written by importer versions that treated era
//! aliases as separate rows. Port of `util/repairEras.ts`.

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use crate::error::ApiError;
use crate::text;

#[derive(Debug, Clone)]
struct EraRow {
    id: i64,
    name: Option<String>,
    is_main: i64,
}

fn normalize_name(value: Option<&str>) -> String {
    text::collapse_whitespace(value.unwrap_or(""))
}

fn name_key(value: Option<&str>) -> String {
    normalize_name(value).to_lowercase()
}

/// `/\s+\([^()]*\)\s*$/` → the prefix before the trailing parenthetical.
fn remove_trailing_parenthetical(value: &str) -> Option<String> {
    let end = value.trim_end_matches(text::is_js_whitespace).len();
    if end == 0 || !value[..end].ends_with(')') {
        return None;
    }
    let close = end - 1;
    let bytes = value.as_bytes();

    // The last unmatched '(' before the closing paren (`[^()]*` in between).
    let mut open = None;
    let mut index = close;
    while index > 0 {
        index -= 1;
        match bytes[index] {
            b')' => return None,
            b'(' => {
                open = Some(index);
                break;
            }
            _ => {}
        }
    }
    let open = open?;

    // `\s+` immediately before the '('.
    let mut run_start = None;
    for (position, character) in value[..open].char_indices().rev() {
        if text::is_js_whitespace(character) {
            run_start = Some(position);
        } else {
            break;
        }
    }
    let run_start = run_start?;
    Some(text::trim(&value[..run_start]).to_string())
}

fn choose_era(eras: &[EraRow], primary_song_counts: &HashMap<i64, i64>) -> i64 {
    let name_lengths: HashMap<i64, usize> = eras
        .iter()
        .map(|era| {
            (
                era.id,
                text::utf16_len(&normalize_name(era.name.as_deref())),
            )
        })
        .collect();

    let mut sorted: Vec<&EraRow> = eras.iter().collect();
    sorted.sort_by(|left, right| {
        let main_difference = right.is_main - left.is_main;
        if main_difference != 0 {
            return main_difference.cmp(&0);
        }
        let song_difference = primary_song_counts.get(&right.id).copied().unwrap_or(0)
            - primary_song_counts.get(&left.id).copied().unwrap_or(0);
        if song_difference != 0 {
            return song_difference.cmp(&0);
        }
        let name_difference = name_lengths.get(&left.id).copied().unwrap_or(0) as i64
            - name_lengths.get(&right.id).copied().unwrap_or(0) as i64;
        if name_difference != 0 {
            return name_difference.cmp(&0);
        }
        left.id.cmp(&right.id)
    });
    sorted[0].id
}

fn resolve_era_id(era_id: i64, merge_targets: &[(i64, i64)]) -> i64 {
    let lookup = |id: i64| {
        merge_targets
            .iter()
            .find(|(from, _)| *from == id)
            .map(|(_, to)| *to)
    };
    let mut current_id = era_id;
    let mut seen = HashSet::new();
    while !seen.contains(&current_id) {
        let Some(next_id) = lookup(current_id) else {
            break;
        };
        seen.insert(current_id);
        current_id = next_id;
    }
    current_id
}

pub fn repair_era_duplicates(conn: &Connection) -> Result<(), ApiError> {
    let eras = {
        let mut statement = conn.prepare("SELECT id, name, is_main FROM eras ORDER BY id ASC")?;
        let rows = statement.query_map([], |row| {
            Ok(EraRow {
                id: row.get(0)?,
                name: row.get(1)?,
                is_main: row.get::<_, Option<i64>>(2)?.unwrap_or(1),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    if eras.len() < 2 {
        return Ok(());
    }

    let primary_song_counts: HashMap<i64, i64> = {
        let mut statement = conn.prepare(
            "SELECT era, count(id) FROM songs WHERE catalog_id = 'unreleased' GROUP BY era",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.filter_map(|row| match row {
            Ok((Some(era_id), count)) => Some(Ok((era_id, count))),
            Ok((None, _)) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<HashMap<_, _>, _>>()?
    };

    // Insertion-ordered groups (`Map` iteration order in the original).
    let mut eras_by_name: Vec<(String, Vec<EraRow>)> = Vec::new();
    for era in &eras {
        let key = name_key(era.name.as_deref());
        if key.is_empty() {
            continue;
        }
        match eras_by_name
            .iter_mut()
            .find(|(existing, _)| *existing == key)
        {
            Some((_, group)) => group.push(era.clone()),
            None => eras_by_name.push((key, vec![era.clone()])),
        }
    }
    let group_for = |key: &str| {
        eras_by_name
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, group)| group)
    };

    let mut merge_targets: Vec<(i64, i64)> = Vec::new();
    for (_, group) in &eras_by_name {
        if group.len() < 2 {
            continue;
        }
        let chosen = choose_era(group, &primary_song_counts);
        for era in group {
            if era.id != chosen {
                merge_targets.push((era.id, chosen));
            }
        }
    }

    for era in &eras {
        if merge_targets.iter().any(|(from, _)| *from == era.id)
            || primary_song_counts.get(&era.id).copied().unwrap_or(0) > 0
        {
            continue;
        }

        let mut candidate = normalize_name(era.name.as_deref());
        while !candidate.is_empty() {
            candidate = remove_trailing_parenthetical(&candidate).unwrap_or_default();
            if candidate.is_empty() {
                break;
            }
            if let Some(matching_era) =
                group_for(&name_key(Some(&candidate))).and_then(|group| group.first())
            {
                if matching_era.id != era.id {
                    let resolved = resolve_era_id(matching_era.id, &merge_targets);
                    merge_targets.push((era.id, resolved));
                    break;
                }
            }
        }
    }

    let resolved_targets: Vec<(i64, i64)> = merge_targets
        .iter()
        .map(|(from_id, to_id)| (*from_id, resolve_era_id(*to_id, &merge_targets)))
        .filter(|(from_id, to_id)| from_id != to_id)
        .collect();
    if resolved_targets.is_empty() {
        return Ok(());
    }

    let transaction = conn.unchecked_transaction()?;
    for (from_id, to_id) in &resolved_targets {
        transaction.execute("UPDATE songs SET era = ?1 WHERE era = ?2", [to_id, from_id])?;
        transaction.execute("DELETE FROM eras WHERE id = ?1", [from_id])?;
    }
    transaction.commit()?;

    println!("merged {} duplicate era rows", resolved_targets.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_only_trailing_parentheticals() {
        assert_eq!(
            remove_trailing_parenthetical("Era (Annotated)").as_deref(),
            Some("Era")
        );
        assert_eq!(
            remove_trailing_parenthetical("Era (A) (B)").as_deref(),
            Some("Era (A)")
        );
        assert_eq!(remove_trailing_parenthetical("Era(Annotated)"), None);
        assert_eq!(remove_trailing_parenthetical("Era (a(b))"), None);
        assert_eq!(
            remove_trailing_parenthetical("Era (Annotated) ").as_deref(),
            Some("Era")
        );
        assert_eq!(remove_trailing_parenthetical("Era"), None);
    }
}
