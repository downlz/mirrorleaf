use std::time::{SystemTime, UNIX_EPOCH};

use mirrorleaf_proto::Change;
use uuid::Uuid;
use yrs::types::text::YChange;
use yrs::{
    Any, In, Map, MapPrelim, MapRef, Out, ReadTxn, Text, Transact, TransactionMut, WriteTxn, Xml,
    XmlElementPrelim, XmlElementRef, XmlFragment, XmlOut, XmlTextPrelim, XmlTextRef,
};

use crate::replica::decode_update;
use crate::{CoreError, PhotoRef, Replica, Result};

/// The notebook index: one Yjs document mapping note ID → {title, trashed,
/// updated, folder, pinned, preview} and folder ID → {name, deleted, created}
/// (plan §5.1). Trashing and deleting are flags, never deletes, so a device that
/// was offline can't bring a note or folder back.
pub const INDEX_DOC: &str = "index";
const NOTES: &str = "notes";
/// One level of folders (M5, owner's decision 4 Nov 2026).
const FOLDERS: &str = "folders";

/// An edit that changes neither a note's title nor its preview moves its
/// `updated` time at most this often, so typing doesn't add an index change to
/// every batch (plan §5.2).
pub const TOUCH_INTERVAL_MS: i64 = 60_000;
/// The longest preview the index keeps for a note.
pub const PREVIEW_CHARS: usize = 160;

/// The XML fragment holding a note's ProseMirror content. "default" is the field
/// TipTap's collaboration extension uses, so the editor (M5) reads the same data.
pub const CONTENT_FRAGMENT: &str = "default";
const PARAGRAPH: &str = "paragraph";
/// TipTap's Image node, with the photo's address in `src` (plan §5.6).
const IMAGE: &str = "image";

/// One top-level block of a note's content.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Paragraph(String),
    Photo {
        photo: PhotoRef,
        width: u32,
        height: u32,
    },
    /// Any other block (a heading, a list, a quote, or a type a later editor
    /// adds), with its text run together.
    Other {
        kind: String,
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct NoteMeta {
    pub id: String,
    pub title: String,
    pub trashed: bool,
    pub updated_ms: i64,
    /// The folder the note is in. `None` if it is in none, or its folder was deleted.
    pub folder: Option<String>,
    pub pinned: bool,
    /// The start of the note's text after its title, for the note list.
    pub preview: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FolderMeta {
    pub id: String,
    pub name: String,
    pub created_ms: i64,
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
        self.create_note_in(title, None)
    }

    /// Creates a note, in `folder` if given.
    pub fn create_note_in(&mut self, title: &str, folder: Option<&str>) -> Result<String> {
        let id = Uuid::now_v7().to_string();
        let updated = now_ms();
        self.edit_doc(INDEX_DOC, |txn| {
            if let Some(folder) = folder {
                live_folder(txn, folder)?;
            }
            let notes = txn.get_or_insert_map(NOTES);
            let entry = MapPrelim::from([
                ("title", In::Any(Any::from(title))),
                ("trashed", In::Any(Any::from(false))),
                ("updated", In::Any(Any::from(updated as f64))),
                ("folder", In::Any(Any::from(folder.unwrap_or("")))),
                ("pinned", In::Any(Any::from(false))),
                ("preview", In::Any(Any::from(""))),
            ]);
            notes.insert(txn, id.as_str(), entry);
            Ok::<_, CoreError>(())
        })?
        .0?;
        Ok(id)
    }

    /// Every note, most recently updated first.
    pub fn list_notes(&self) -> Result<Vec<NoteMeta>> {
        Ok(self.list_index()?.0)
    }

    /// Every note (most recently updated first) and every folder that hasn't
    /// been deleted (by name), from one load of the index document.
    pub fn list_index(&self) -> Result<(Vec<NoteMeta>, Vec<FolderMeta>)> {
        let doc = self.load_doc(INDEX_DOC)?;
        let txn = doc.transact();
        let folders = folder_entries(&txn);
        let Some(notes) = txn.get_map(NOTES) else {
            return Ok((Vec::new(), folders));
        };
        let mut list: Vec<NoteMeta> = notes
            .iter(&txn)
            .filter_map(|(id, value)| match value {
                Out::YMap(entry) => {
                    let folder = string_field(&entry, &txn, "folder");
                    let live = folders.iter().any(|f| f.id == folder);
                    Some(NoteMeta {
                        id: id.to_owned(),
                        title: string_field(&entry, &txn, "title"),
                        trashed: bool_field(&entry, &txn, "trashed"),
                        updated_ms: number_field(&entry, &txn, "updated"),
                        folder: live.then_some(folder),
                        pinned: bool_field(&entry, &txn, "pinned"),
                        preview: string_field(&entry, &txn, "preview"),
                    })
                }
                _ => None,
            })
            .collect();
        list.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms).then(a.id.cmp(&b.id)));
        Ok((list, folders))
    }

    /// The folders that haven't been deleted, by name.
    pub fn list_folders(&self) -> Result<Vec<FolderMeta>> {
        let doc = self.load_doc(INDEX_DOC)?;
        let txn = doc.transact();
        Ok(folder_entries(&txn))
    }

    pub fn create_folder(&mut self, name: &str) -> Result<String> {
        let name = folder_name(name)?;
        let id = Uuid::now_v7().to_string();
        let created = now_ms();
        self.edit_doc(INDEX_DOC, |txn| {
            let folders = txn.get_or_insert_map(FOLDERS);
            let entry = MapPrelim::from([
                ("name", In::Any(Any::from(name))),
                ("deleted", In::Any(Any::from(false))),
                ("created", In::Any(Any::from(created as f64))),
            ]);
            folders.insert(txn, id.as_str(), entry);
        })?;
        Ok(id)
    }

    pub fn rename_folder(&mut self, id: &str, name: &str) -> Result<()> {
        let name = folder_name(name)?;
        self.edit_doc(INDEX_DOC, |txn| {
            live_folder(txn, id)?.insert(txn, "name", name);
            Ok(())
        })?
        .0
    }

    /// Deletes a folder and moves its notes to the trash, as Apple Notes does.
    /// A note moved into the folder on another device meanwhile shows outside
    /// any folder once the two devices sync.
    pub fn delete_folder(&mut self, id: &str) -> Result<()> {
        let updated = now_ms();
        self.edit_doc(INDEX_DOC, |txn| {
            live_folder(txn, id)?.insert(txn, "deleted", true);
            let notes = txn.get_or_insert_map(NOTES);
            let inside: Vec<MapRef> = notes
                .iter(txn)
                .filter_map(|(_, value)| match value {
                    Out::YMap(entry) if string_field(&entry, txn, "folder") == id => Some(entry),
                    _ => None,
                })
                .collect();
            for entry in inside {
                if !bool_field(&entry, txn, "trashed") {
                    entry.insert(txn, "trashed", true);
                    entry.insert(txn, "updated", updated as f64);
                }
            }
            Ok(())
        })?
        .0
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
                XmlOut::Element(p) => Some(element_text(&txn, &p)),
                _ => None,
            })
            .collect())
    }

    /// The note's top-level blocks, in order. A block's position is the index the
    /// text edits take.
    pub fn note_blocks(&self, id: &str) -> Result<Vec<Block>> {
        let doc = self.load_doc(id)?;
        let txn = doc.transact();
        let Some(fragment) = txn.get_xml_fragment(CONTENT_FRAGMENT) else {
            return Ok(Vec::new());
        };
        let number = |element: &XmlElementRef, name| match element.get_attribute(&txn, name) {
            Some(Out::Any(Any::Number(n))) => n.as_f64().unwrap_or(0.0) as u32,
            _ => 0,
        };
        Ok(fragment
            .children(&txn)
            .filter_map(|node| match node {
                XmlOut::Element(e) if e.tag().as_ref() == PARAGRAPH => {
                    Some(Block::Paragraph(element_text(&txn, &e)))
                }
                XmlOut::Element(e) if e.tag().as_ref() == IMAGE => {
                    let photo = match e.get_attribute(&txn, "src") {
                        Some(Out::Any(Any::String(src))) => PhotoRef::parse(&src),
                        _ => None,
                    };
                    Some(match photo {
                        Some(photo) => Block::Photo {
                            photo,
                            width: number(&e, "width"),
                            height: number(&e, "height"),
                        },
                        None => Block::Other {
                            kind: IMAGE.to_owned(),
                            text: String::new(),
                        },
                    })
                }
                XmlOut::Element(e) => Some(Block::Other {
                    kind: e.tag().to_string(),
                    text: element_text(&txn, &e),
                }),
                _ => None,
            })
            .collect())
    }

    /// Adds a photo at the end of the note. Its blobs must already be stored or
    /// staged (see `photos::prepare`).
    pub fn add_photo(&mut self, id: &str, photo: &PhotoRef, width: u32, height: u32) -> Result<()> {
        self.edit_doc(id, |txn| {
            let content = txn.get_or_insert_xml_fragment(CONTENT_FRAGMENT);
            let end = content.len(txn);
            let image = content.insert(txn, end, XmlElementPrelim::empty(IMAGE));
            image.insert_attribute(txn, "src", photo.to_uri());
            image.insert_attribute(txn, "width", f64::from(width));
            image.insert_attribute(txn, "height", f64::from(height));
        })?;
        self.touch(id)
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

    /// Takes a note out of the trash. If its folder was deleted meanwhile, it
    /// comes back outside any folder.
    pub fn restore_note(&mut self, id: &str) -> Result<()> {
        let updated = now_ms();
        self.edit_doc(INDEX_DOC, |txn| {
            let entry = note_entry(txn, id)?;
            let folder = string_field(&entry, txn, "folder");
            if !folder.is_empty() && live_folder(txn, &folder).is_err() {
                entry.insert(txn, "folder", "");
            }
            entry.insert(txn, "trashed", false);
            entry.insert(txn, "updated", updated as f64);
            Ok(())
        })?
        .0
    }

    /// Pins or unpins a note. Leaves its `updated` time alone.
    pub fn set_pinned(&mut self, id: &str, pinned: bool) -> Result<()> {
        self.edit_doc(INDEX_DOC, |txn| {
            note_entry(txn, id)?.insert(txn, "pinned", pinned);
            Ok(())
        })?
        .0
    }

    /// Moves a note into `folder`, or out of any folder with `None`. Leaves its
    /// `updated` time alone.
    pub fn move_note(&mut self, id: &str, folder: Option<&str>) -> Result<()> {
        self.edit_doc(INDEX_DOC, |txn| {
            if let Some(folder) = folder {
                live_folder(txn, folder)?;
            }
            note_entry(txn, id)?.insert(txn, "folder", folder.unwrap_or(""));
            Ok(())
        })?
        .0
    }

    /// Records what the editor shows as a note's title and preview. Writes only
    /// what changed; an edit that changes neither moves the note's `updated`
    /// time at most every [`TOUCH_INTERVAL_MS`]. Returns whether it wrote.
    pub fn update_note_meta(&mut self, id: &str, title: &str, preview: &str) -> Result<bool> {
        let preview: String = preview.chars().take(PREVIEW_CHARS).collect();
        let now = now_ms();
        self.edit_doc(INDEX_DOC, |txn| {
            let entry = note_entry(txn, id)?;
            let new_title = string_field(&entry, txn, "title") != title;
            let new_preview = string_field(&entry, txn, "preview") != preview;
            let recent = now - number_field(&entry, txn, "updated") < TOUCH_INTERVAL_MS;
            if !new_title && !new_preview && recent {
                return Ok(false);
            }
            if new_title {
                entry.insert(txn, "title", title);
            }
            if new_preview {
                entry.insert(txn, "preview", preview.as_str());
            }
            entry.insert(txn, "updated", now as f64);
            Ok(true)
        })?
        .0
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
            let entry = note_entry(txn, id)?;
            edit(&entry, txn);
            entry.insert(txn, "updated", updated as f64);
            Ok(())
        })?
        .0
    }
}

fn note_entry(txn: &mut TransactionMut, id: &str) -> Result<MapRef> {
    let notes = txn.get_or_insert_map(NOTES);
    match notes.get(txn, id) {
        Some(Out::YMap(entry)) => Ok(entry),
        _ => Err(CoreError::NoSuchNote(id.to_owned())),
    }
}

/// A folder's entry, if it exists and hasn't been deleted.
fn live_folder(txn: &mut TransactionMut, id: &str) -> Result<MapRef> {
    let folders = txn.get_or_insert_map(FOLDERS);
    match folders.get(txn, id) {
        Some(Out::YMap(entry)) if !bool_field(&entry, txn, "deleted") => Ok(entry),
        _ => Err(CoreError::Invalid(format!("no folder {id}"))),
    }
}

fn folder_entries<T: ReadTxn>(txn: &T) -> Vec<FolderMeta> {
    let Some(folders) = txn.get_map(FOLDERS) else {
        return Vec::new();
    };
    let mut list: Vec<FolderMeta> = folders
        .iter(txn)
        .filter_map(|(id, value)| match value {
            Out::YMap(entry) if !bool_field(&entry, txn, "deleted") => Some(FolderMeta {
                id: id.to_owned(),
                name: string_field(&entry, txn, "name"),
                created_ms: number_field(&entry, txn, "created"),
            }),
            _ => None,
        })
        .collect();
    list.sort_by(|a, b| {
        (a.name.to_lowercase(), a.created_ms, &a.id).cmp(&(
            b.name.to_lowercase(),
            b.created_ms,
            &b.id,
        ))
    });
    list
}

fn folder_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CoreError::Invalid("a folder needs a name".into()));
    }
    Ok(name)
}

fn string_field<T: ReadTxn>(entry: &MapRef, txn: &T, key: &str) -> String {
    match entry.get(txn, key) {
        Some(Out::Any(Any::String(s))) => s.to_string(),
        _ => String::new(),
    }
}

fn bool_field<T: ReadTxn>(entry: &MapRef, txn: &T, key: &str) -> bool {
    matches!(entry.get(txn, key), Some(Out::Any(Any::Bool(true))))
}

fn number_field<T: ReadTxn>(entry: &MapRef, txn: &T, key: &str) -> i64 {
    match entry.get(txn, key) {
        Some(Out::Any(Any::Number(n))) => n.as_f64().unwrap_or(0.0) as i64,
        _ => 0,
    }
}

/// An element's text, without formatting, with its child blocks' text joined
/// by spaces.
fn element_text<T: ReadTxn>(txn: &T, element: &XmlElementRef) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut inline = String::new();
    for child in element.children(txn) {
        match child {
            XmlOut::Text(t) => inline.push_str(&plain_text(txn, &t)),
            XmlOut::Element(e) => {
                let text = element_text(txn, &e);
                if !text.is_empty() {
                    parts.push(text);
                }
            }
            XmlOut::Fragment(_) => {}
        }
    }
    if !inline.is_empty() {
        parts.insert(0, inline);
    }
    parts.join(" ")
}

/// A text node's characters without its formatting. (`get_string` would wrap
/// formatted runs in tags such as `<bold>`.)
fn plain_text<T: ReadTxn>(txn: &T, text: &XmlTextRef) -> String {
    text.diff(txn, YChange::identity)
        .into_iter()
        .filter_map(|d| match d.insert {
            Out::Any(Any::String(s)) => Some(s.to_string()),
            _ => None,
        })
        .collect()
}

fn paragraph_text(txn: &mut TransactionMut, index: u32) -> Result<XmlTextRef> {
    let content = txn.get_or_insert_xml_fragment(CONTENT_FRAGMENT);
    let Some(XmlOut::Element(paragraph)) = content.get(txn, index) else {
        return Err(CoreError::Invalid(format!("no paragraph {index}")));
    };
    if paragraph.tag().as_ref() != PARAGRAPH {
        return Err(CoreError::Invalid(format!(
            "block {index} isn't a paragraph"
        )));
    }
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
    use std::sync::Arc;

    use mirrorleaf_proto::{BlobHash, DeviceId, VersionVector};
    use yrs::types::Attrs;
    use yrs::updates::decoder::Decode;

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
    fn photos_sit_between_paragraphs_as_blocks() {
        let mut replica = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
        let id = replica.create_note("Trip").unwrap();
        replica.append_paragraph(&id, "before").unwrap();
        let photo = PhotoRef {
            full: BlobHash([1; 32]),
            thumbnail: BlobHash([2; 32]),
        };
        replica.add_photo(&id, &photo, 2048, 1536).unwrap();
        replica.append_paragraph(&id, "after").unwrap();
        assert_eq!(
            replica.note_blocks(&id).unwrap(),
            [
                Block::Paragraph("before".into()),
                Block::Photo {
                    photo,
                    width: 2048,
                    height: 1536
                },
                Block::Paragraph("after".into()),
            ]
        );
        assert!(replica.insert_text(&id, 1, 0, "x").is_err());
        replica.insert_text(&id, 2, 5, "wards").unwrap();
    }

    #[test]
    fn folders_hold_notes_and_deleting_one_trashes_them() {
        let mut replica = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
        let work = replica.create_folder(" Work ").unwrap();
        let home = replica.create_folder("home").unwrap();
        assert!(replica.create_folder("  ").is_err());
        let names: Vec<_> = replica
            .list_folders()
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(names, ["home", "Work"]);

        let plan = replica.create_note_in("Plan", Some(&work)).unwrap();
        let loose = replica.create_note("Loose").unwrap();
        replica.move_note(&loose, Some(&home)).unwrap();
        replica.move_note(&loose, None).unwrap();
        assert!(replica.move_note(&loose, Some("no-such-folder")).is_err());
        replica.rename_folder(&work, "Office").unwrap();

        replica.delete_folder(&work).unwrap();
        assert!(replica.create_note_in("Late", Some(&work)).is_err());
        let names: Vec<_> = replica
            .list_folders()
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(names, ["home"]);
        let notes = replica.list_notes().unwrap();
        let plan_meta = notes.iter().find(|n| n.id == plan).unwrap();
        assert!(plan_meta.trashed);
        assert_eq!(plan_meta.folder, None, "a deleted folder no longer shows");

        replica.restore_note(&plan).unwrap();
        let notes = replica.list_notes().unwrap();
        let plan_meta = notes.iter().find(|n| n.id == plan).unwrap();
        assert!(!plan_meta.trashed);
        assert_eq!(plan_meta.folder, None);
    }

    #[test]
    fn pins_and_meta_write_only_what_changed() {
        let mut replica = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
        let id = replica.create_note("").unwrap();
        let before = replica.change_count().unwrap();
        assert!(
            replica
                .update_note_meta(&id, "Groceries", "milk, eggs")
                .unwrap()
        );
        assert!(
            !replica
                .update_note_meta(&id, "Groceries", "milk, eggs")
                .unwrap()
        );
        assert_eq!(replica.change_count().unwrap(), before + 1);

        let long = "x".repeat(PREVIEW_CHARS + 50);
        replica.update_note_meta(&id, "Groceries", &long).unwrap();
        replica.set_pinned(&id, true).unwrap();
        let note = replica.find_note(&id).unwrap();
        assert!(note.pinned);
        assert_eq!(note.title, "Groceries");
        assert_eq!(note.preview.chars().count(), PREVIEW_CHARS);
    }

    #[test]
    fn formatted_and_nested_text_reads_as_plain_text() {
        let mut replica = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
        let id = replica.create_note("Rich").unwrap();
        replica
            .edit_doc(&id, |txn| {
                let content = txn.get_or_insert_xml_fragment(CONTENT_FRAGMENT);
                let p = content.insert(txn, 0, XmlElementPrelim::empty(PARAGRAPH));
                let text = p.insert(txn, 0, XmlTextPrelim::new("bold and plain"));
                let bold = Attrs::from([(Arc::from("bold"), Any::Bool(true))]);
                text.format(txn, 0, 4, bold);
                let list = content.insert(txn, 1, XmlElementPrelim::empty("bulletList"));
                for (i, item) in ["one", "two"].into_iter().enumerate() {
                    let li = list.insert(txn, i as u32, XmlElementPrelim::empty("listItem"));
                    let p = li.insert(txn, 0, XmlElementPrelim::empty(PARAGRAPH));
                    p.insert(txn, 0, XmlTextPrelim::new(item));
                }
            })
            .unwrap();
        assert_eq!(
            replica.note_blocks(&id).unwrap(),
            [
                Block::Paragraph("bold and plain".into()),
                Block::Other {
                    kind: "bulletList".into(),
                    text: "one two".into()
                },
            ]
        );
    }

    #[test]
    fn an_editor_follows_other_devices_changes_only() {
        let mut replica = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
        let mut other = Replica::open_in_memory(DeviceId([2; 32])).unwrap();
        let id = replica.create_note("Shared").unwrap();
        replica.append_paragraph(&id, "mine").unwrap();
        let state = replica.doc_state(&id).unwrap();
        let cursor = replica.last_row().unwrap();

        // The other device starts from the same state and adds a paragraph.
        let (theirs, _) = replica
            .changes_since(&VersionVector::default(), 0, usize::MAX)
            .unwrap();
        other.apply_remote(&theirs).unwrap();
        other.append_paragraph(&id, "theirs").unwrap();
        replica.append_paragraph(&id, "mine again").unwrap();
        let (new, _) = other
            .changes_since(&replica.version_vector().unwrap(), 0, usize::MAX)
            .unwrap();
        replica.apply_remote(&new).unwrap();

        let (updates, next) = replica.doc_changes_after(&id, cursor).unwrap();
        assert!(next > cursor);
        // The other device's paragraph, not this device's own.
        let editor = yrs::Doc::new();
        {
            let mut txn = editor.transact_mut();
            txn.apply_update(yrs::Update::decode_v1(&state).unwrap())
                .unwrap();
            for update in &updates {
                txn.apply_update(yrs::Update::decode_v1(update).unwrap())
                    .unwrap();
            }
        }
        let txn = editor.transact();
        let fragment = txn.get_xml_fragment(CONTENT_FRAGMENT).unwrap();
        let texts: Vec<String> = fragment
            .children(&txn)
            .filter_map(|n| match n {
                XmlOut::Element(e) => Some(element_text(&txn, &e)),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["mine", "theirs"]);
        assert_eq!(replica.doc_changes_after(&id, next).unwrap().0.len(), 0);
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
