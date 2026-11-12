//! Full-text search over whole notes (M12, E5): an SQLite FTS5 index inside
//! the (encrypted) database, kept up to date from the change log. The index is
//! this device's own and never syncs. It is built only while the screen is in
//! use, never by a push-woken job (the ≤ 50 MB job target): a note's text is
//! read by replaying its Yjs document, which the job must not do.

use std::collections::{HashMap, HashSet};

use rusqlite::{OptionalExtension, params};

use crate::notes::Block;
use crate::{INDEX_DOC, Replica, Result};

/// Schema 5: the index and what it was built from.
pub(crate) const SCHEMA_V5: &str = "
    CREATE VIRTUAL TABLE search USING fts5(
        note UNINDEXED,
        title,
        body,
        tokenize = \"unicode61 remove_diacritics 2 categories 'L* N* Co M*'\"
    );
    CREATE TABLE search_state (
        note        TEXT PRIMARY KEY,
        fts_row     INTEGER NOT NULL,
        title       TEXT NOT NULL,
        indexed_row INTEGER NOT NULL
    );
";

/// Meta key: the change-log row up to which every change is in the index.
const CURSOR: &str = "search_cursor";

/// Marks around matching words in a snippet: Start of Text and End of Text,
/// which never occur in a note.
pub const MATCH_START: char = '\u{2}';
pub const MATCH_END: char = '\u{3}';

/// One note found by a search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub id: String,
    /// A few words of the note around the first match in its text, the
    /// matches between [`MATCH_START`] and [`MATCH_END`]. Empty when only the
    /// title matched.
    pub snippet: String,
}

impl Replica {
    /// Brings the search index up to date with at most `limit` notes read.
    /// Returns whether notes are still waiting, so the caller can go on in
    /// small steps and let others use the database in between.
    pub fn update_search(&mut self, limit: usize) -> Result<bool> {
        let cursor = self
            .meta(CURSOR)?
            .and_then(|v| String::from_utf8(v).ok())
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let last = self.last_row()?;
        // The notes changed since the cursor, by their newest change.
        let mut newest: HashMap<String, i64> = HashMap::new();
        let mut index_changed = false;
        {
            let mut stmt = self.conn.prepare_cached(
                "SELECT doc_id, MAX(rowid) FROM changes WHERE rowid > ?1 GROUP BY doc_id",
            )?;
            let rows = stmt.query_map([cursor], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (doc, row) = row?;
                if doc == INDEX_DOC {
                    index_changed = true;
                } else {
                    newest.insert(doc, row);
                }
            }
        }
        if newest.is_empty() && !index_changed && cursor == last {
            return Ok(false);
        }

        let notes: HashMap<String, String> = self
            .list_notes()?
            .into_iter()
            .map(|n| (n.id, n.title))
            .collect();
        let state: HashMap<String, (i64, String, i64)> = {
            let mut stmt = self
                .conn
                .prepare_cached("SELECT note, fts_row, title, indexed_row FROM search_state")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get(0)?, (row.get(1)?, row.get(2)?, row.get(3)?)))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };

        // Deleted for good (or gone): out of the index.
        let gone: Vec<String> = state
            .keys()
            .filter(|id| !notes.contains_key(*id))
            .cloned()
            .collect();
        let mut waiting: Vec<(String, String)> = notes
            .iter()
            .filter(|(id, title)| match state.get(*id) {
                None => true,
                Some((_, indexed_title, indexed_row)) => {
                    indexed_title != *title || newest.get(*id).is_some_and(|row| row > indexed_row)
                }
            })
            .map(|(id, title)| (id.clone(), title.clone()))
            .collect();
        // Newest notes first: what was just edited is searchable soonest.
        waiting.sort_by_key(|(id, _)| std::cmp::Reverse(newest.get(id).copied().unwrap_or(0)));
        let more = waiting.len() > limit;
        waiting.truncate(limit);

        // Read the texts before writing, outside the transaction.
        let mut texts = Vec::with_capacity(waiting.len());
        for (id, title) in waiting {
            let body = self.note_text(&id).unwrap_or_default();
            texts.push((id, title, body));
        }

        let tx = self.conn.transaction()?;
        for id in &gone {
            if let Some((fts_row, _, _)) = state.get(id) {
                tx.execute("DELETE FROM search WHERE rowid = ?1", [fts_row])?;
            }
            tx.execute("DELETE FROM search_state WHERE note = ?1", [id])?;
        }
        for (id, title, body) in &texts {
            if let Some((fts_row, _, _)) = state.get(id) {
                tx.execute("DELETE FROM search WHERE rowid = ?1", [fts_row])?;
            }
            tx.execute(
                "INSERT INTO search (note, title, body) VALUES (?1, ?2, ?3)",
                params![id, title, body],
            )?;
            let fts_row = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO search_state (note, fts_row, title, indexed_row)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (note) DO UPDATE SET
                   fts_row = excluded.fts_row, title = excluded.title,
                   indexed_row = excluded.indexed_row",
                params![id, fts_row, title, last],
            )?;
        }
        if !more {
            tx.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![CURSOR, last.to_string().as_bytes()],
            )?;
        }
        tx.commit()?;
        Ok(more)
    }

    /// The notes matching `query`, best first: every word must appear in the
    /// title or the text, each as a word or the start of one ("veg" finds
    /// "vegetable"), accents ignored. Searches the index as it stands; call
    /// [`Replica::update_search`] first.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let Some(expr) = match_expression(query) else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT note, snippet(search, 2, char(2), char(3), '…', 12)
             FROM search WHERE search MATCH ?1
             ORDER BY bm25(search, 0.0, 5.0, 1.0) LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![expr, limit as i64], |row| {
            Ok(SearchHit {
                id: row.get(0)?,
                snippet: row.get(1)?,
            })
        })?;
        let mut hits: Vec<SearchHit> = rows.collect::<rusqlite::Result<_>>()?;
        // A snippet with no match in it means the title matched, not the text.
        for hit in &mut hits {
            if !hit.snippet.contains(MATCH_START) {
                hit.snippet.clear();
            }
        }
        let mut seen = HashSet::new();
        hits.retain(|h| seen.insert(h.id.clone()));
        Ok(hits)
    }

    /// The text of a note after its first line (the title, which the index
    /// keeps on its own), one block per line.
    fn note_text(&self, id: &str) -> Result<String> {
        let lines: Vec<String> = self
            .note_blocks(id)?
            .into_iter()
            .filter_map(|block| match block {
                Block::Paragraph(text) | Block::Other { text, .. } => Some(text),
                Block::Photo { .. } => None,
            })
            .filter(|line| !line.trim().is_empty())
            .skip(1)
            .collect();
        Ok(lines.join("\n"))
    }

    /// Whether the index has been started (for tests and the screen).
    pub fn search_cursor(&self) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [CURSOR], |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .optional()?
            .and_then(|v| String::from_utf8(v).ok())
            .and_then(|v| v.parse().ok()))
    }
}

/// The person's words as an FTS5 query: each word quoted (so nothing they type
/// is read as FTS5 syntax) and matched as a prefix, all of them required.
fn match_expression(query: &str) -> Option<String> {
    let words: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && !is_mark(c))
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// Combining marks (Indic vowel signs, for example), which belong inside words.
fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x0900..=0x0DFF) && !c.is_whitespace()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeviceId;

    fn replica() -> Replica {
        Replica::open_in_memory(DeviceId([1; 32])).unwrap()
    }

    fn note(replica: &mut Replica, title: &str, text: &str) -> String {
        let id = replica.create_note(title).unwrap();
        replica.append_paragraph(&id, title).unwrap();
        for line in text.lines() {
            replica.append_paragraph(&id, line).unwrap();
        }
        id
    }

    fn ids(hits: &[SearchHit]) -> Vec<&str> {
        hits.iter().map(|h| h.id.as_str()).collect()
    }

    #[test]
    fn finds_words_anywhere_in_a_note_by_their_start() {
        let mut r = replica();
        let market = note(
            &mut r,
            "Vegetable market",
            "Fruits done\nOnions and garlic left",
        );
        let packing = note(&mut r, "Packing list", "Walking boots\nMap and compass");
        assert!(!r.update_search(100).unwrap());

        assert_eq!(ids(&r.search("garl", 10).unwrap()), [market.as_str()]);
        assert_eq!(ids(&r.search("veg", 10).unwrap()), [market.as_str()]);
        assert_eq!(
            ids(&r.search("map COMPASS", 10).unwrap()),
            [packing.as_str()]
        );
        assert!(r.search("map garlic", 10).unwrap().is_empty(), "all words");
        assert!(r.search("  ", 10).unwrap().is_empty());
        // FTS5 syntax is taken as plain words.
        assert!(r.search("\"boots\" OR NEAR(", 10).is_ok());

        let hit = &r.search("onions", 10).unwrap()[0];
        assert!(
            hit.snippet.contains("\u{2}Onions\u{3}"),
            "{:?}",
            hit.snippet
        );
        // A title-only match has no snippet.
        assert_eq!(r.search("packing", 10).unwrap()[0].snippet, "");
    }

    #[test]
    fn ignores_accents_and_keeps_indic_words_whole() {
        let mut r = replica();
        let cafe = note(&mut r, "Café list", "Crème brûlée");
        let hindi = note(&mut r, "सूची", "नमस्ते दुनिया");
        r.update_search(100).unwrap();
        assert_eq!(ids(&r.search("creme", 10).unwrap()), [cafe.as_str()]);
        assert_eq!(ids(&r.search("cafe", 10).unwrap()), [cafe.as_str()]);
        assert_eq!(ids(&r.search("नमस्", 10).unwrap()), [hindi.as_str()]);
        assert_eq!(ids(&r.search("दुनिया", 10).unwrap()), [hindi.as_str()]);
    }

    #[test]
    fn follows_edits_titles_and_deletions_in_small_steps() {
        let mut r = replica();
        let a = note(&mut r, "First", "alpha");
        let b = note(&mut r, "Second", "beta");
        let c = note(&mut r, "Third", "gamma");
        // Two at a time: more waiting, then done.
        assert!(r.update_search(2).unwrap());
        assert!(!r.update_search(2).unwrap());
        assert_eq!(r.search("alpha beta gamma", 10).unwrap().len(), 0);
        assert_eq!(r.search("gamma", 10).unwrap().len(), 1);

        r.append_paragraph(&a, "delta").unwrap();
        r.set_title(&b, "Renamed").unwrap();
        r.delete_note(&c).unwrap();
        assert!(!r.update_search(10).unwrap());
        assert_eq!(ids(&r.search("delta", 10).unwrap()), [a.as_str()]);
        assert_eq!(ids(&r.search("renamed", 10).unwrap()), [b.as_str()]);
        assert!(
            r.search("gamma", 10).unwrap().is_empty(),
            "deleted for good"
        );
        // Nothing new: nothing read.
        assert!(!r.update_search(10).unwrap());
        assert!(r.search_cursor().unwrap().is_some());
    }
}
