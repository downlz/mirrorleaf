use std::time::{SystemTime, UNIX_EPOCH};

use mirrorleaf_proto::Change;
use uuid::Uuid;
use yrs::{
    Any, GetString, In, Map, MapPrelim, MapRef, Out, ReadTxn, Text, Transact, TransactionMut,
    WriteTxn, XmlElementPrelim, XmlElementRef, XmlFragment, XmlOut, XmlTextPrelim, XmlTextRef,
};

use crate::replica::decode_update;
use crate::{CoreError, Replica, Result};

/// The notebook index: one Yjs document mapping note ID → {title, trashed,
/// updated} (plan §5.1). Trashing is a flag, never a delete, so a device that was
/// offline can't bring a note back.
pub const INDEX_DOC: &str = "index";
const NOTES: &str = "notes";

/// The XML fragment holding a note's ProseMirror content. "default" is the field
/// TipTap's collaboration extension uses, so the editor (M5) reads the same data.
pub const CONTENT_FRAGMENT: &str = "default";
const PARAGRAPH: &str = "paragraph";

#[derive(Clone, Debug, PartialEq)]
pub struct NoteMeta {
    pub id: String,
    pub title: String,
    pub trashed: bool,
    pub updated_ms: i64,
}

impl Replica {
    /// Runs `edit` on the current state of `doc_id` and logs what it changed as a
    /// local change. Returns `None` for the change if `edit` changed nothing.
    pub fn edit_doc<R>(
        &mut self,
        doc_id: &str,
        edit: impl FnOnce(&mut TransactionMut) -> R,
    ) -> Result<(R, Option<Change>)> {
        let doc = self.load_doc(doc_id)?;
        let (result, update) = {
            let mut txn = doc.transact_mut();
            let result = edit(&mut txn);
            // Only this transaction's inserts and deletes, not the whole history.
            (result, txn.encode_update_v1())
        };
        if decode_update(&update)?.is_empty() {
            return Ok((result, None));
        }
        let change = self.record_local(doc_id, update)?;
        Ok((result, Some(change)))
    }

    pub fn create_note(&mut self, title: &str) -> Result<String> {
        let id = Uuid::now_v7().to_string();
        let updated = now_ms();
        self.edit_doc(INDEX_DOC, |txn| {
            let notes = txn.get_or_insert_map(NOTES);
            let entry = MapPrelim::from([
                ("title", In::Any(Any::from(title))),
                ("trashed", In::Any(Any::from(false))),
                ("updated", In::Any(Any::from(updated as f64))),
            ]);
            notes.insert(txn, id.as_str(), entry);
        })?;
        Ok(id)
    }

    /// Every note, most recently updated first.
    pub fn list_notes(&self) -> Result<Vec<NoteMeta>> {
        let doc = self.load_doc(INDEX_DOC)?;
        let txn = doc.transact();
        let Some(notes) = txn.get_map(NOTES) else {
            return Ok(Vec::new());
        };
        let mut list: Vec<NoteMeta> = notes
            .iter(&txn)
            .filter_map(|(id, value)| match value {
                Out::YMap(entry) => Some(NoteMeta {
                    id: id.to_owned(),
                    title: match entry.get(&txn, "title") {
                        Some(Out::Any(Any::String(s))) => s.to_string(),
                        _ => String::new(),
                    },
                    trashed: matches!(entry.get(&txn, "trashed"), Some(Out::Any(Any::Bool(true)))),
                    updated_ms: match entry.get(&txn, "updated") {
                        Some(Out::Any(Any::Number(n))) => n.as_f64().unwrap_or(0.0) as i64,
                        _ => 0,
                    },
                }),
                _ => None,
            })
            .collect();
        list.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms).then(a.id.cmp(&b.id)));
        Ok(list)
    }

    /// Finds a note by its ID or a unique prefix of it.
    pub fn find_note(&self, id_prefix: &str) -> Result<NoteMeta> {
        let mut matches = self
            .list_notes()?
            .into_iter()
            .filter(|n| n.id.starts_with(id_prefix));
        match (matches.next(), matches.next()) {
            (Some(note), None) if !id_prefix.is_empty() => Ok(note),
            (Some(_), Some(_)) => Err(CoreError::Invalid(format!(
                "{id_prefix} matches more than one note"
            ))),
            _ => Err(CoreError::NoSuchNote(id_prefix.to_owned())),
        }
    }

    /// The note's paragraphs as plain text.
    pub fn note_paragraphs(&self, id: &str) -> Result<Vec<String>> {
        let doc = self.load_doc(id)?;
        let txn = doc.transact();
        let Some(fragment) = txn.get_xml_fragment(CONTENT_FRAGMENT) else {
            return Ok(Vec::new());
        };
        Ok(fragment
            .children(&txn)
            .filter_map(|node| match node {
                XmlOut::Element(p) => Some(
                    p.children(&txn)
                        .filter_map(|child| match child {
                            XmlOut::Text(t) => Some(t.get_string(&txn)),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect())
    }

    pub fn append_paragraph(&mut self, id: &str, text: &str) -> Result<()> {
        self.edit_doc(id, |txn| {
            let content = txn.get_or_insert_xml_fragment(CONTENT_FRAGMENT);
            let end = content.len(txn);
            let paragraph = content.insert(txn, end, XmlElementPrelim::empty(PARAGRAPH));
            paragraph.insert(txn, 0, XmlTextPrelim::new(text));
        })?;
        self.touch(id)
    }

    /// Inserts `text` at character `offset` of paragraph `index` (both from 0).
    pub fn insert_text(&mut self, id: &str, index: u32, offset: u32, text: &str) -> Result<()> {
        self.edit_doc(id, |txn| {
            let target = paragraph_text(txn, index)?;
            if offset > target.len(txn) {
                return Err(CoreError::Invalid(format!(
                    "offset {offset} is past the end"
                )));
            }
            target.insert(txn, offset, text);
            Ok(())
        })?
        .0?;
        self.touch(id)
    }

    /// Deletes `len` characters from `offset` in paragraph `index`.
    pub fn delete_text(&mut self, id: &str, index: u32, offset: u32, len: u32) -> Result<()> {
        self.edit_doc(id, |txn| {
            let target = paragraph_text(txn, index)?;
            if offset.saturating_add(len) > target.len(txn) {
                return Err(CoreError::Invalid("range runs past the end".into()));
            }
            target.remove_range(txn, offset, len);
            Ok(())
        })?
        .0?;
        self.touch(id)
    }

    pub fn set_title(&mut self, id: &str, title: &str) -> Result<()> {
        self.update_entry(id, |entry, txn| {
            entry.insert(txn, "title", title);
        })
    }

    pub fn trash_note(&mut self, id: &str) -> Result<()> {
        self.update_entry(id, |entry, txn| {
            entry.insert(txn, "trashed", true);
        })
    }

    fn touch(&mut self, id: &str) -> Result<()> {
        self.update_entry(id, |_, _| {})
    }

    /// Edits a note's index entry and stamps its `updated` time.
    fn update_entry(
        &mut self,
        id: &str,
        edit: impl FnOnce(&MapRef, &mut TransactionMut),
    ) -> Result<()> {
        let updated = now_ms();
        self.edit_doc(INDEX_DOC, |txn| {
            let notes = txn.get_or_insert_map(NOTES);
            let Some(Out::YMap(entry)) = notes.get(txn, id) else {
                return Err(CoreError::NoSuchNote(id.to_owned()));
            };
            edit(&entry, txn);
            entry.insert(txn, "updated", updated as f64);
            Ok(())
        })?
        .0
    }
}

fn paragraph_text(txn: &mut TransactionMut, index: u32) -> Result<XmlTextRef> {
    let content = txn.get_or_insert_xml_fragment(CONTENT_FRAGMENT);
    let Some(XmlOut::Element(paragraph)) = content.get(txn, index) else {
        return Err(CoreError::Invalid(format!("no paragraph {index}")));
    };
    Ok(first_text(txn, &paragraph))
}

/// The paragraph's text node, created if the paragraph is empty.
fn first_text(txn: &mut TransactionMut, paragraph: &XmlElementRef) -> XmlTextRef {
    let existing = paragraph.children(txn).find_map(|child| match child {
        XmlOut::Text(t) => Some(t),
        _ => None,
    });
    existing.unwrap_or_else(|| paragraph.insert(txn, 0, XmlTextPrelim::new("")))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use mirrorleaf_proto::DeviceId;

    use super::*;

    #[test]
    fn notes_round_trip_through_the_log() {
        let mut replica = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
        let id = replica.create_note("Groceries").unwrap();
        replica.append_paragraph(&id, "milk").unwrap();
        replica.append_paragraph(&id, "bread").unwrap();
        replica.insert_text(&id, 0, 4, " and eggs").unwrap();
        replica.delete_text(&id, 1, 0, 1).unwrap();
        assert_eq!(
            replica.note_paragraphs(&id).unwrap(),
            ["milk and eggs", "read"]
        );

        replica.set_title(&id, "Shopping").unwrap();
        replica.trash_note(&id).unwrap();
        let note = replica.find_note(&id[..8]).unwrap();
        assert_eq!((note.title.as_str(), note.trashed), ("Shopping", true));
    }

    #[test]
    fn invalid_edits_change_nothing() {
        let mut replica = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
        let id = replica.create_note("Empty").unwrap();
        let before = replica.change_count().unwrap();
        assert!(replica.insert_text(&id, 3, 0, "x").is_err());
        assert!(replica.trash_note("no-such-note").is_err());
        assert_eq!(replica.change_count().unwrap(), before);
    }
}
