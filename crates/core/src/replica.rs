use std::path::Path;
use std::time::Duration;

use mirrorleaf_proto::{Change, ContentType, DeviceId, SyncClass, VersionVector};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

use crate::DatabaseKey;
use crate::blobs::record_refs;
use crate::{CoreError, Result};

/// Schema version 1 (plan §5.1). `docs` holds snapshots, which compaction writes
/// once it arrives (plan §5.2, E11); until then documents rebuild from the log.
const SCHEMA_V1: &str = "
    CREATE TABLE meta (
        key   TEXT PRIMARY KEY,
        value BLOB NOT NULL
    );
    -- rowid is the order this device received each change. Sending in that order
    -- keeps every change after the changes it depends on.
    CREATE TABLE changes (
        rowid     INTEGER PRIMARY KEY,
        origin    BLOB NOT NULL,
        seq       INTEGER NOT NULL,
        doc_id    TEXT NOT NULL,
        update_v1 BLOB NOT NULL,
        UNIQUE (origin, seq)
    );
    CREATE INDEX changes_by_doc ON changes (doc_id);
    CREATE TABLE version_vector (
        origin BLOB PRIMARY KEY,
        seq    INTEGER NOT NULL
    );
    CREATE TABLE docs (
        doc_id TEXT PRIMARY KEY,
        state  BLOB NOT NULL
    );
";

/// Schema version 2 (M2): photos (plan §5.5). The blob store itself belongs to
/// the network layer; these tables say which photos notes refer to and which
/// blobs this device holds, so sync can tell what to fetch without loading a
/// document.
const SCHEMA_V2: &str = "
    -- Every photo any change refers to, found in the change's bytes.
    CREATE TABLE photo_refs (
        full   BLOB PRIMARY KEY,
        thumb  BLOB NOT NULL,
        doc_id TEXT NOT NULL
    );
    -- Blobs the blob store holds in full.
    CREATE TABLE blobs_held (
        hash BLOB PRIMARY KEY,
        len  INTEGER NOT NULL
    );
    -- Full images asked for, until they arrive.
    CREATE TABLE blob_wants (
        hash BLOB PRIMARY KEY
    );
    -- Blobs waiting to enter the blob store, from a process that doesn't have
    -- the store open (the CLI's `photo add`).
    CREATE TABLE staged_blobs (
        hash BLOB PRIMARY KEY,
        data BLOB NOT NULL
    );
";

/// Schema version 3 (Phase 1, M9): each change records what its bytes are, how
/// soon it should travel, and when it was made (sync protocol 2). Changes
/// stored before are Yjs updates of the normal class, made at an unknown time.
const SCHEMA_V3: &str = "
    ALTER TABLE changes ADD COLUMN content INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE changes ADD COLUMN class   INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE changes ADD COLUMN made_at INTEGER NOT NULL DEFAULT 0;
";

/// Schema version 4 (Phase 1, M10): what each other device was last seen to
/// hold, from the sync rounds with it. Sign-out uses it to warn about changes
/// only this device holds (E3); E6 shows it, E11 compacts by it.
const SCHEMA_V4: &str = "
    CREATE TABLE peer_holds (
        device BLOB NOT NULL,
        origin BLOB NOT NULL,
        seq    INTEGER NOT NULL,
        PRIMARY KEY (device, origin)
    );
    CREATE TABLE peer_seen (
        device BLOB PRIMARY KEY,
        at     INTEGER NOT NULL
    );
";

/// One device's copy of the account's notes.
pub struct Replica {
    pub(crate) conn: Connection,
    device: DeviceId,
}

impl Replica {
    /// Opens or creates the database at `path` for `device`. Refuses a database
    /// created for another device, which would corrupt the sequence numbers.
    pub fn open(path: impl AsRef<Path>, device: DeviceId) -> Result<Self> {
        Self::open_with_key(path, Some(device), None)
    }

    /// Opens the database at `path`, encrypted with `key` if one is given
    /// (M11, E4). With no `device`, the one the database was created for: for
    /// a device whose key is kept inside its encrypted database.
    pub fn open_with_key(
        path: impl AsRef<Path>,
        device: Option<DeviceId>,
        key: Option<&DatabaseKey>,
    ) -> Result<Self> {
        let conn = Connection::open(path)?;
        if let Some(key) = key {
            crate::encryption::unlock(&conn, key)?;
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let device = match device {
            Some(device) => device,
            None => {
                let stored: Option<Vec<u8>> = conn
                    .query_row(
                        "SELECT value FROM meta WHERE key = 'device_id'",
                        [],
                        |row| row.get(0),
                    )
                    .optional()?;
                let stored = stored.ok_or_else(|| {
                    CoreError::Invalid("the database doesn't say which device it belongs to".into())
                })?;
                device_id(stored)?
            }
        };
        Self::init(conn, device)
    }

    pub fn open_in_memory(device: DeviceId) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, device)
    }

    fn init(conn: Connection, device: DeviceId) -> Result<Self> {
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version == 0 {
            conn.execute_batch(SCHEMA_V1)?;
            conn.pragma_update(None, "user_version", 1)?;
        }
        if version < 2 {
            migrate_to_v2(&conn)?;
        }
        if version < 3 {
            migrate_to_v3(&conn)?;
        }
        if version < 4 {
            migrate(&conn, SCHEMA_V4, 4)?;
        }
        let mut replica = Self { conn, device };
        match replica.meta("device_id")? {
            None => replica.set_meta("device_id", &device.0)?,
            Some(stored) if stored == device.0 => {}
            Some(stored) => {
                return Err(CoreError::WrongDevice {
                    stored: data_hex(&stored),
                    given: device.to_string(),
                });
            }
        }
        Ok(replica)
    }

    pub fn device(&self) -> DeviceId {
        self.device
    }

    pub fn meta(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    pub fn set_meta(&mut self, key: &str, value: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Appends a local edit to the change log under the next sequence number.
    pub fn record_local(&mut self, doc_id: &str, update: Vec<u8>) -> Result<Change> {
        decode_update(&update)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let held = held_seq(&tx, &self.device)?;
        let change = Change::yjs(self.device, held + 1, doc_id, update, unix_now());
        insert_change(&tx, &change)?;
        tx.commit()?;
        Ok(change)
    }

    pub fn delete_meta(&mut self, key: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM meta WHERE key = ?1", [key])?;
        Ok(())
    }

    /// Remembers that `peer` held at least `holds` at Unix time `at` (after a
    /// sync round with it). Never lowers a number already recorded.
    pub fn record_peer_holds(
        &mut self,
        peer: DeviceId,
        holds: &VersionVector,
        at: u64,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO peer_holds (device, origin, seq) VALUES (?1, ?2, ?3)
                 ON CONFLICT (device, origin) DO UPDATE SET seq = max(seq, excluded.seq)",
            )?;
            for (origin, seq) in &holds.0 {
                stmt.execute(params![&peer.0[..], &origin.0[..], *seq as i64])?;
            }
            tx.execute(
                "INSERT INTO peer_seen (device, at) VALUES (?1, ?2)
                 ON CONFLICT (device) DO UPDATE SET at = max(at, excluded.at)",
                params![&peer.0[..], at as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// What `peer` was last seen to hold, and when (Unix seconds), if this
    /// device has synced with it.
    pub fn peer_holds(&self, peer: &DeviceId) -> Result<Option<(VersionVector, u64)>> {
        let at: Option<i64> = self
            .conn
            .query_row(
                "SELECT at FROM peer_seen WHERE device = ?1",
                [&peer.0[..]],
                |row| row.get(0),
            )
            .optional()?;
        let Some(at) = at else {
            return Ok(None);
        };
        let mut stmt = self
            .conn
            .prepare_cached("SELECT origin, seq FROM peer_holds WHERE device = ?1")?;
        let rows = stmt.query_map([&peer.0[..]], |row| {
            Ok((device_id(row.get::<_, Vec<u8>>(0)?)?, row.get::<_, i64>(1)?))
        })?;
        let mut vv = VersionVector::default();
        for row in rows {
            let (origin, seq) = row?;
            vv.0.insert(origin, seq as u64);
        }
        Ok(Some((vv, at as u64)))
    }

    /// How many changes this device holds that none of `others` is known to
    /// hold: what signing out would lose (E3). Each change counts as long as
    /// no other device was seen to hold it, wherever it was made.
    pub fn only_here(&self, others: &[DeviceId]) -> Result<u64> {
        let mut known = VersionVector::default();
        for peer in others {
            if let Some((holds, _)) = self.peer_holds(peer)? {
                known = known.merged(&holds);
            }
        }
        Ok(self.version_vector()?.ahead_of(&known))
    }

    pub fn version_vector(&self) -> Result<VersionVector> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT origin, seq FROM version_vector")?;
        let rows = stmt.query_map([], |row| {
            Ok((device_id(row.get::<_, Vec<u8>>(0)?)?, row.get::<_, i64>(1)?))
        })?;
        let mut vv = VersionVector::default();
        for row in rows {
            let (origin, seq) = row?;
            vv.0.insert(origin, seq as u64);
        }
        Ok(vv)
    }

    /// The next batch of changes a peer holding `theirs` lacks, in log order,
    /// starting after log row `after`. Returns the batch (about `max_bytes` of
    /// updates, at least one change) and the row to continue from. An empty batch
    /// means the peer is up to date.
    pub fn changes_since(
        &self,
        theirs: &VersionVector,
        after: i64,
        max_bytes: usize,
    ) -> Result<(Vec<Change>, i64)> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT rowid, origin, seq, doc_id, content, class, made_at, update_v1 FROM changes
             WHERE rowid > ?1 ORDER BY rowid",
        )?;
        let mut rows = stmt.query([after])?;
        let (mut batch, mut bytes, mut cursor) = (Vec::new(), 0, after);
        while let Some(row) = rows.next()? {
            cursor = row.get(0)?;
            let change = Change {
                origin: device_id(row.get(1)?)?,
                seq: row.get::<_, i64>(2)? as u64,
                doc_id: row.get(3)?,
                content: ContentType(row.get(4)?),
                class: SyncClass(row.get(5)?),
                made_at: row.get::<_, i64>(6)? as u64,
                update: row.get(7)?,
            };
            if theirs.contains(&change) {
                continue;
            }
            bytes += change.update.len();
            batch.push(change);
            if bytes >= max_bytes {
                break;
            }
        }
        Ok((batch, cursor))
    }

    /// Stores changes received from a peer, in one transaction. Changes already
    /// held are skipped. Returns how many were new. Yjs updates are checked
    /// before they are stored; other content types are stored as they come,
    /// so a newer device's changes can pass through this one.
    pub fn apply_remote(&mut self, changes: &[Change]) -> Result<usize> {
        for change in changes.iter().filter(|c| c.content == ContentType::YJS) {
            decode_update(&change.update)?;
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut added = 0;
        for change in changes {
            let held = held_seq(&tx, &change.origin)?;
            if change.seq <= held {
                continue;
            }
            if change.seq != held + 1 {
                return Err(CoreError::Gap {
                    origin: change.origin,
                    seq: change.seq,
                    held,
                });
            }
            insert_change(&tx, change)?;
            added += 1;
        }
        tx.commit()?;
        Ok(added)
    }

    /// Rebuilds a document from its snapshot (if any) and its log rows. Yjs
    /// updates are idempotent, so rows already in the snapshot are harmless.
    pub fn load_doc(&self, doc_id: &str) -> Result<Doc> {
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let snapshot: Option<Vec<u8>> = self
                .conn
                .query_row(
                    "SELECT state FROM docs WHERE doc_id = ?1",
                    [doc_id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(state) = snapshot {
                apply(&mut txn, &state)?;
            }
            let mut stmt = self.conn.prepare_cached(
                "SELECT update_v1 FROM changes WHERE doc_id = ?1 AND content = 0 ORDER BY rowid",
            )?;
            let mut rows = stmt.query([doc_id])?;
            while let Some(row) = rows.next()? {
                apply(&mut txn, &row.get::<_, Vec<u8>>(0)?)?;
            }
        }
        Ok(doc)
    }

    /// A document's whole state as one Yjs update, for an editor to start from
    /// (plan §5.6).
    pub fn doc_state(&self, doc_id: &str) -> Result<Vec<u8>> {
        let doc = self.load_doc(doc_id)?;
        let txn = doc.transact();
        Ok(txn.encode_state_as_update_v1(&StateVector::default()))
    }

    /// The last row of the change log (0 if it is empty). An editor that opens
    /// a document after reading this follows it with
    /// [`Replica::doc_changes_after`].
    pub fn last_row(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COALESCE(MAX(rowid), 0) FROM changes", [], |row| {
                row.get(0)
            })?)
    }

    /// Updates to `doc_id` that other devices made, stored after log row
    /// `after`, and the row to continue from. The editor's own changes are left
    /// out: it already has them.
    pub fn doc_changes_after(&self, doc_id: &str, after: i64) -> Result<(Vec<Vec<u8>>, i64)> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT rowid, origin, update_v1 FROM changes
             WHERE doc_id = ?1 AND rowid > ?2 AND content = 0 ORDER BY rowid",
        )?;
        let mut rows = stmt.query(params![doc_id, after])?;
        let (mut updates, mut cursor) = (Vec::new(), after);
        while let Some(row) = rows.next()? {
            cursor = row.get(0)?;
            if device_id(row.get(1)?)? != self.device {
                updates.push(row.get(2)?);
            }
        }
        Ok((updates, cursor))
    }

    /// Number of rows in the change log.
    pub fn change_count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM changes", [], |row| row.get(0))?;
        Ok(n as u64)
    }
}

fn held_seq(conn: &Connection, origin: &DeviceId) -> Result<u64> {
    let seq: Option<i64> = conn
        .query_row(
            "SELECT seq FROM version_vector WHERE origin = ?1",
            [&origin.0[..]],
            |row| row.get(0),
        )
        .optional()?;
    Ok(seq.unwrap_or(0) as u64)
}

fn insert_change(conn: &Connection, change: &Change) -> Result<()> {
    conn.execute(
        "INSERT INTO changes (origin, seq, doc_id, content, class, made_at, update_v1)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            &change.origin.0[..],
            change.seq as i64,
            change.doc_id,
            change.content.0,
            change.class.0,
            change.made_at as i64,
            change.update
        ],
    )?;
    conn.execute(
        "INSERT INTO version_vector (origin, seq) VALUES (?1, ?2)
         ON CONFLICT (origin) DO UPDATE SET seq = excluded.seq",
        params![&change.origin.0[..], change.seq as i64],
    )?;
    if change.content == ContentType::YJS {
        record_refs(conn, &change.doc_id, &change.update)?;
    }
    Ok(())
}

/// The time now, in Unix seconds.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Adds each change's content type, sync class and time (M9).
fn migrate_to_v3(conn: &Connection) -> Result<()> {
    migrate(conn, SCHEMA_V3, 3)
}

/// Runs `schema` and sets the schema version to `version`, in one transaction.
fn migrate(conn: &Connection, schema: &str, version: i64) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let migrated = (|| {
        conn.execute_batch(schema)?;
        conn.pragma_update(None, "user_version", version)?;
        Ok(())
    })();
    conn.execute_batch(if migrated.is_ok() {
        "COMMIT"
    } else {
        "ROLLBACK"
    })?;
    migrated
}

/// Adds the photo tables, and finds the photos changes already in the log refer to.
fn migrate_to_v2(conn: &Connection) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let migrated = (|| {
        conn.execute_batch(SCHEMA_V2)?;
        let mut stmt = conn.prepare("SELECT doc_id, update_v1 FROM changes ORDER BY rowid")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            record_refs(conn, &row.get::<_, String>(0)?, &row.get::<_, Vec<u8>>(1)?)?;
        }
        conn.pragma_update(None, "user_version", 2)?;
        Ok(())
    })();
    conn.execute_batch(if migrated.is_ok() {
        "COMMIT"
    } else {
        "ROLLBACK"
    })?;
    migrated
}

fn device_id(bytes: Vec<u8>) -> rusqlite::Result<DeviceId> {
    let len = bytes.len();
    bytes.try_into().map(DeviceId).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            len,
            rusqlite::types::Type::Blob,
            "device ID must be 32 bytes".into(),
        )
    })
}

pub(crate) fn decode_update(bytes: &[u8]) -> Result<Update> {
    Update::decode_v1(bytes).map_err(|e| CoreError::Update(e.to_string()))
}

fn apply(txn: &mut yrs::TransactionMut, bytes: &[u8]) -> Result<()> {
    txn.apply_update(decode_update(bytes)?)
        .map_err(|e| CoreError::Update(e.to_string()))
}

fn data_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::{GetString, ReadTxn, Text, WriteTxn};

    fn text_update(text: &str) -> Vec<u8> {
        let doc = Doc::new();
        let mut txn = doc.transact_mut();
        txn.get_or_insert_text("t").push(&mut txn, text);
        txn.encode_update_v1()
    }

    #[test]
    fn local_changes_get_consecutive_sequence_numbers() {
        let mut replica = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
        let a = replica.record_local("doc", text_update("a")).unwrap();
        let b = replica.record_local("doc", text_update("b")).unwrap();
        assert_eq!((a.seq, b.seq), (1, 2));
        assert_eq!(replica.version_vector().unwrap().get(&DeviceId([1; 32])), 2);
    }

    #[test]
    fn remote_changes_skip_duplicates_and_reject_gaps() {
        let mut source = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
        let first = source.record_local("doc", text_update("a")).unwrap();
        let second = source.record_local("doc", text_update("b")).unwrap();
        let third = source.record_local("doc", text_update("c")).unwrap();

        let mut sink = Replica::open_in_memory(DeviceId([2; 32])).unwrap();
        assert_eq!(
            sink.apply_remote(&[first.clone(), second.clone()]).unwrap(),
            2
        );
        assert_eq!(sink.apply_remote(&[first, second]).unwrap(), 0);
        let mut skipped = third.clone();
        skipped.seq = 4;
        assert!(matches!(
            sink.apply_remote(&[skipped]),
            Err(CoreError::Gap { .. })
        ));
        assert_eq!(sink.apply_remote(&[third]).unwrap(), 1);
    }

    #[test]
    fn changes_since_pages_through_what_the_peer_lacks() {
        let mut replica = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
        for word in ["one", "two", "three"] {
            replica.record_local("doc", text_update(word)).unwrap();
        }
        let theirs = VersionVector([(DeviceId([1; 32]), 1)].into());
        let (page, cursor) = replica.changes_since(&theirs, 0, 1).unwrap();
        assert_eq!(page.iter().map(|c| c.seq).collect::<Vec<_>>(), [2]);
        let (page, cursor) = replica.changes_since(&theirs, cursor, 1).unwrap();
        assert_eq!(page.iter().map(|c| c.seq).collect::<Vec<_>>(), [3]);
        assert!(
            replica
                .changes_since(&theirs, cursor, 1)
                .unwrap()
                .0
                .is_empty()
        );
    }

    #[test]
    fn local_changes_carry_their_time_and_type() {
        let mut replica = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
        let before = unix_now();
        let change = replica.record_local("doc", text_update("a")).unwrap();
        assert!(change.made_at >= before && change.made_at <= unix_now());
        assert_eq!(
            (change.content, change.class),
            (ContentType::YJS, SyncClass::NORMAL)
        );
        let (page, _) = replica
            .changes_since(&VersionVector::default(), 0, 1 << 20)
            .unwrap();
        assert_eq!(page, [change]);
    }

    #[test]
    fn unknown_content_passes_through_without_touching_documents() {
        let mut sink = Replica::open_in_memory(DeviceId([2; 32])).unwrap();
        let mut future = Change::yjs(DeviceId([1; 32]), 1, "doc", b"not yjs at all".to_vec(), 7);
        future.content = ContentType(9);
        future.class = SyncClass::LAZY;
        assert_eq!(sink.apply_remote(std::slice::from_ref(&future)).unwrap(), 1);
        // Stored and passed on as it came...
        let (page, _) = sink
            .changes_since(&VersionVector::default(), 0, 1 << 20)
            .unwrap();
        assert_eq!(page, [future]);
        // ...but never applied to a Yjs document.
        let doc = sink.load_doc("doc").unwrap();
        assert_eq!(doc.transact().get_text("t"), None);
    }

    #[test]
    fn documents_rebuild_from_the_log() {
        let mut replica = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
        replica.record_local("doc", text_update("hello")).unwrap();
        let doc = replica.load_doc("doc").unwrap();
        let txn = doc.transact();
        assert_eq!(txn.get_text("t").unwrap().get_string(&txn), "hello");
    }

    #[test]
    fn an_m1_database_gains_the_photo_tables() {
        use crate::{BlobHash, PhotoRef};

        let dir = std::env::temp_dir().join(format!("mirrorleaf-v1-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m1.db");
        let _ = std::fs::remove_file(&path);
        let photo = PhotoRef {
            full: BlobHash([1; 32]),
            thumbnail: BlobHash([2; 32]),
        };
        {
            // A database as M1 left it, holding a change that refers to a photo.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA_V1).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            let change = Change::yjs(DeviceId([1; 32]), 1, "note", photo.to_uri().into_bytes(), 0);
            conn.execute(
                "INSERT INTO changes (origin, seq, doc_id, update_v1) VALUES (?1, ?2, ?3, ?4)",
                params![&change.origin.0[..], 1, change.doc_id, change.update],
            )
            .unwrap();
        }
        let replica = Replica::open(&path, DeviceId([1; 32])).unwrap();
        let version: i64 = replica
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 4);
        assert_eq!(replica.photos().unwrap()[0].photo, photo);
        // M9's columns: the old change reads as a Yjs update, normal class,
        // made at an unknown time.
        let (page, _) = replica
            .changes_since(&VersionVector::default(), 0, 1 << 20)
            .unwrap();
        assert_eq!(
            (page[0].content, page[0].class, page[0].made_at),
            (ContentType::YJS, SyncClass::NORMAL, 0)
        );
        drop(replica);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn changes_no_other_device_holds_are_counted() {
        let me = DeviceId([1; 32]);
        let (phone, laptop, lost) = (DeviceId([2; 32]), DeviceId([3; 32]), DeviceId([4; 32]));
        let mut replica = Replica::open_in_memory(me).unwrap();
        for text in ["a", "b", "c"] {
            replica.record_local("note", text_update(text)).unwrap();
        }
        // A change from the lost device that only this one received.
        let theirs = Change::yjs(lost, 1, "note", text_update("d"), 0);
        replica.apply_remote(&[theirs]).unwrap();
        assert_eq!(replica.only_here(&[phone, laptop]).unwrap(), 4);

        // The phone took two of ours; the laptop took all three, then less
        // in an older report that mustn't lower what is known.
        let vv = |n| VersionVector([(me, n)].into());
        replica.record_peer_holds(phone, &vv(2), 100).unwrap();
        replica.record_peer_holds(laptop, &vv(3), 200).unwrap();
        replica.record_peer_holds(laptop, &vv(1), 150).unwrap();
        assert_eq!(replica.peer_holds(&laptop).unwrap(), Some((vv(3), 200)));
        assert_eq!(replica.peer_holds(&lost).unwrap(), None);
        assert_eq!(
            replica.only_here(&[phone, laptop]).unwrap(),
            1,
            "the lost device's"
        );
        assert_eq!(replica.only_here(&[phone]).unwrap(), 2);
    }

    #[test]
    fn a_database_belongs_to_one_device() {
        let dir = std::env::temp_dir().join(format!("mirrorleaf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wrong-device.db");
        let _ = std::fs::remove_file(&path);
        Replica::open(&path, DeviceId([1; 32])).unwrap();
        assert!(matches!(
            Replica::open(&path, DeviceId([2; 32])),
            Err(CoreError::WrongDevice { .. })
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
