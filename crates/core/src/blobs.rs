use std::fmt;
use std::str::FromStr;

use mirrorleaf_proto::BlobHash;
use rusqlite::{Connection, OptionalExtension, params};

use crate::{CoreError, Replica, Result};

/// How a note refers to a photo: `mirrorleaf-blob://<full>?thumb=<thumbnail>`,
/// both BLAKE3 hashes in hex (plan §5.1). The editor's custom protocol (M5) shows
/// the full image if this device holds it and the thumbnail otherwise.
pub const BLOB_SCHEME: &str = "mirrorleaf-blob://";
const THUMB_PARAM: &str = "?thumb=";
const URI_LEN: usize = BLOB_SCHEME.len() + 64 + THUMB_PARAM.len() + 64;

/// A photo in a note: its full image and its thumbnail, both blobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PhotoRef {
    pub full: BlobHash,
    pub thumbnail: BlobHash,
}

impl PhotoRef {
    pub fn to_uri(&self) -> String {
        format!("{BLOB_SCHEME}{}{THUMB_PARAM}{}", self.full, self.thumbnail)
    }

    pub fn parse(uri: &str) -> Option<Self> {
        let (full, thumbnail) = uri.strip_prefix(BLOB_SCHEME)?.split_once(THUMB_PARAM)?;
        Some(Self {
            full: BlobHash::from_str(full).ok()?,
            thumbnail: BlobHash::from_str(thumbnail).ok()?,
        })
    }
}

/// Every photo a Yjs update refers to. The address is one attribute string,
/// which a v1 update stores whole, so it can be found in the update's bytes
/// without loading the document (plan §5.2). Text that merely looks like an
/// address only makes a fetch that finds nothing.
pub(crate) fn refs_in(update: &[u8]) -> Vec<PhotoRef> {
    let scheme = BLOB_SCHEME.as_bytes();
    let mut refs = Vec::new();
    let mut from = 0;
    while let Some(at) = update[from..]
        .windows(scheme.len())
        .position(|w| w == scheme)
    {
        let start = from + at;
        let found = update
            .get(start..start + URI_LEN)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(PhotoRef::parse);
        refs.extend(found);
        from = start + scheme.len();
    }
    refs
}

/// Records the photos a change refers to. Called for every change stored, local
/// or remote.
pub(crate) fn record_refs(conn: &Connection, doc_id: &str, update: &[u8]) -> Result<()> {
    for photo in refs_in(update) {
        conn.execute(
            "INSERT INTO photo_refs (full, thumb, doc_id) VALUES (?1, ?2, ?3)
             ON CONFLICT (full) DO NOTHING",
            params![&photo.full.0[..], &photo.thumbnail.0[..], doc_id],
        )?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlobKind {
    Thumbnail,
    Full,
}

impl fmt::Display for BlobKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BlobKind::Thumbnail => "thumbnail",
            BlobKind::Full => "full image",
        })
    }
}

/// A photo some note refers to, and what this device holds of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhotoStatus {
    pub photo: PhotoRef,
    /// The note that first referred to it.
    pub note: String,
    pub has_thumbnail: bool,
    pub has_full: bool,
    /// The full image has been asked for and hasn't arrived yet.
    pub wanted: bool,
}

/// A blob counts as held once it is in the store or staged for it.
const HELD: &str = "(SELECT hash FROM blobs_held UNION SELECT hash FROM staged_blobs)";

impl Replica {
    /// Leaves a blob for the process that has the blob store open, which moves
    /// it in with [`Replica::blob_stored`].
    pub fn stage_blob(&mut self, hash: BlobHash, data: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO staged_blobs (hash, data) VALUES (?1, ?2) ON CONFLICT (hash) DO NOTHING",
            params![&hash.0[..], data],
        )?;
        Ok(())
    }

    /// Up to `limit` staged blobs.
    pub fn staged_blobs(&self, limit: usize) -> Result<Vec<(BlobHash, Vec<u8>)>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT hash, data FROM staged_blobs ORDER BY rowid LIMIT ?1")?;
        let rows = stmt.query_map([limit as i64], |row| {
            Ok((blob_hash(row.get(0)?)?, row.get(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// A staged blob's content.
    pub fn staged_blob(&self, hash: BlobHash) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row(
                "SELECT data FROM staged_blobs WHERE hash = ?1",
                [&hash.0[..]],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Records that the blob store now holds `hash` in full, and drops any staged
    /// copy of it and any request for it.
    pub fn blob_stored(&mut self, hash: BlobHash, len: u64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO blobs_held (hash, len) VALUES (?1, ?2) ON CONFLICT (hash) DO NOTHING",
            params![&hash.0[..], len as i64],
        )?;
        tx.execute("DELETE FROM staged_blobs WHERE hash = ?1", [&hash.0[..]])?;
        tx.execute("DELETE FROM blob_wants WHERE hash = ?1", [&hash.0[..]])?;
        tx.commit()?;
        Ok(())
    }

    pub fn holds_blob(&self, hash: BlobHash) -> Result<bool> {
        Ok(self
            .conn
            .query_row(&format!("SELECT ?1 IN {HELD}"), [&hash.0[..]], |row| {
                row.get(0)
            })?)
    }

    /// Asks for a photo's full image. Thumbnails are always fetched; full images
    /// only when a note is opened, or on Wi-Fi (plan §5.5).
    pub fn want_blob(&mut self, hash: BlobHash) -> Result<()> {
        if !self.holds_blob(hash)? {
            self.conn.execute(
                "INSERT INTO blob_wants (hash) VALUES (?1) ON CONFLICT (hash) DO NOTHING",
                [&hash.0[..]],
            )?;
        }
        Ok(())
    }

    /// What to fetch from peers: every missing thumbnail, then the full images
    /// asked for.
    pub fn blobs_to_fetch(&self) -> Result<Vec<(BlobHash, BlobKind)>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT thumb, 0 FROM photo_refs WHERE thumb NOT IN {HELD}
             UNION ALL
             SELECT hash, 1 FROM blob_wants WHERE hash NOT IN {HELD}"
        ))?;
        let rows = stmt.query_map([], |row| {
            let kind = match row.get::<_, i64>(1)? {
                0 => BlobKind::Thumbnail,
                _ => BlobKind::Full,
            };
            Ok((blob_hash(row.get(0)?)?, kind))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every photo the notes refer to, in the order they were first seen.
    pub fn photos(&self) -> Result<Vec<PhotoStatus>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT full, thumb, doc_id, thumb IN {HELD}, full IN {HELD},
                    full IN (SELECT hash FROM blob_wants)
             FROM photo_refs ORDER BY rowid"
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok(PhotoStatus {
                photo: PhotoRef {
                    full: blob_hash(row.get(0)?)?,
                    thumbnail: blob_hash(row.get(1)?)?,
                },
                note: row.get(2)?,
                has_thumbnail: row.get(3)?,
                has_full: row.get(4)?,
                wanted: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Finds a photo by its full image's hash or a unique prefix of it.
    pub fn find_photo(&self, hash_prefix: &str) -> Result<PhotoStatus> {
        let mut matches = self
            .photos()?
            .into_iter()
            .filter(|p| p.photo.full.to_string().starts_with(hash_prefix));
        match (matches.next(), matches.next()) {
            (Some(photo), None) if !hash_prefix.is_empty() => Ok(photo),
            (Some(_), Some(_)) => Err(CoreError::Invalid(format!(
                "{hash_prefix} matches more than one photo"
            ))),
            _ => Err(CoreError::Invalid(format!(
                "no photo matches {hash_prefix}"
            ))),
        }
    }
}

fn blob_hash(bytes: Vec<u8>) -> rusqlite::Result<BlobHash> {
    let len = bytes.len();
    bytes.try_into().map(BlobHash).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            len,
            rusqlite::types::Type::Blob,
            "blob hash must be 32 bytes".into(),
        )
    })
}

#[cfg(test)]
mod tests {
    use mirrorleaf_proto::DeviceId;

    use super::*;

    fn photo(n: u8) -> PhotoRef {
        PhotoRef {
            full: BlobHash([n; 32]),
            thumbnail: BlobHash([n + 100; 32]),
        }
    }

    #[test]
    fn photo_addresses_round_trip_and_are_found_in_bytes() {
        let a = photo(1);
        assert_eq!(PhotoRef::parse(&a.to_uri()), Some(a));
        assert_eq!(PhotoRef::parse("mirrorleaf-blob://abc?thumb=def"), None);

        let b = photo(2);
        let mut bytes = b"\x01\x77junk".to_vec();
        bytes.extend(a.to_uri().as_bytes());
        bytes.extend(b"mirrorleaf-blob://not-a-hash");
        bytes.extend(b.to_uri().as_bytes());
        assert_eq!(refs_in(&bytes), [a, b]);
        assert!(refs_in(&a.to_uri().as_bytes()[..URI_LEN - 1]).is_empty());
    }

    #[test]
    fn missing_thumbnails_and_wanted_images_are_fetched() {
        let mut replica = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
        let note = replica.create_note("Holiday").unwrap();
        let (a, b) = (photo(1), photo(2));
        replica.add_photo(&note, &a, 4, 3).unwrap();
        replica.add_photo(&note, &b, 4, 3).unwrap();

        let thumbs = |r: &Replica| r.blobs_to_fetch().unwrap();
        assert_eq!(
            thumbs(&replica),
            [
                (a.thumbnail, BlobKind::Thumbnail),
                (b.thumbnail, BlobKind::Thumbnail)
            ]
        );

        replica.stage_blob(a.thumbnail, b"thumb a").unwrap();
        replica.blob_stored(b.thumbnail, 7).unwrap();
        replica.want_blob(a.full).unwrap();
        assert_eq!(thumbs(&replica), [(a.full, BlobKind::Full)]);
        assert_eq!(replica.staged_blobs(10).unwrap().len(), 1);

        replica.blob_stored(a.full, 9).unwrap();
        replica.blob_stored(a.thumbnail, 7).unwrap();
        assert!(thumbs(&replica).is_empty());
        assert!(replica.staged_blobs(10).unwrap().is_empty());

        let status = replica.find_photo(&a.full.to_string()[..6]).unwrap();
        assert!(status.has_thumbnail && status.has_full && !status.wanted);
        assert_eq!(status.note, note);
    }

    #[test]
    fn remote_changes_bring_their_photo_references() {
        let mut source = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
        let note = source.create_note("Holiday").unwrap();
        source.add_photo(&note, &photo(1), 4, 3).unwrap();
        let (changes, _) = source
            .changes_since(&Default::default(), 0, usize::MAX)
            .unwrap();

        let mut sink = Replica::open_in_memory(DeviceId([2; 32])).unwrap();
        sink.apply_remote(&changes).unwrap();
        assert_eq!(sink.photos().unwrap()[0].photo, photo(1));
    }
}
