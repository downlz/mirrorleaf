//! Backups (E9, assessment §8.4): a file holding every change in the log and
//! every photo this device holds, which any device can import (the changes
//! merge as they do in sync). With **Encrypt backups** on (off by default,
//! owner, 9 Nov 2026) the file is encrypted with a key derived from the
//! account key, which the recovery kit recreates: the kit and the latest
//! backup restore everything.
//!
//! The file: `MLBACKUP`, a format number, the header (postcard), then the
//! records, each a length and its postcard bytes, ending with `End`. When
//! encrypted, that record stream is cut into 64 KiB chunks, each sealed with
//! ChaCha20-Poly1305 under a nonce made of a random prefix, the chunk's number
//! and a last-chunk flag, with the header as associated data: a chunk can't be
//! moved, dropped or swapped, and a file cut short is noticed.

use std::io::{Read, Write};
use std::str::FromStr;

use anyhow::{Context, Result, bail, ensure};
use mirrorleaf_proto::{AccountId, AccountKey, BlobHash, Change, DeviceId};
use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use serde::{Deserialize, Serialize};

const MAGIC: &[u8; 8] = b"MLBACKUP";
const FORMAT: u8 = 1;
/// Plaintext per encrypted chunk.
const CHUNK: usize = 64 * 1024;
/// Largest header or record accepted (a photo is at most about 600 KB).
const MAX_FRAME: usize = 16 * 1024 * 1024;
/// BLAKE3 key-derivation context: the backup key from the account key. Fixed
/// for good, or older backups couldn't be opened.
const BACKUP_KEY_CONTEXT: &str = "mirrorleaf 2026-11-09 backup key from the account key";
/// The file name's ending.
pub const EXTENSION: &str = "mirrorleaf-backup";

/// What a backup says about itself, readable without its key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupHeader {
    /// The account it was made for: an encrypted backup opens with that
    /// account's key (or its recovery kit).
    pub account: AccountId,
    pub device: DeviceId,
    /// Unix seconds.
    pub made_at: u64,
    pub encrypted: bool,
    nonce_prefix: [u8; 7],
}

/// One thing a backup holds.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub enum Record {
    Change(Change),
    Blob { hash: BlobHash, data: Vec<u8> },
    End,
}

/// The same records, borrowed, for writing (the same encoding).
#[derive(Serialize)]
enum RecordRef<'a> {
    Change(&'a Change),
    Blob { hash: BlobHash, data: &'a [u8] },
    End,
}

/// The key that encrypts the account's backups.
pub fn backup_key(account: &AccountKey) -> [u8; 32] {
    blake3::derive_key(BACKUP_KEY_CONTEXT, &account.to_bytes())
}

/// The key to open a backup with: none for a plain one; the account's own if
/// it was made for this account; otherwise the backup's own recovery kit,
/// whose `words` recreate the account key it was made with.
pub fn key_for(
    header: &BackupHeader,
    current: Option<&AccountKey>,
    words: Option<&str>,
) -> Result<Option<[u8; 32]>> {
    if !header.encrypted {
        return Ok(None);
    }
    if let Some(key) = current
        && key.id() == header.account
    {
        return Ok(Some(backup_key(key)));
    }
    let Some(words) = words.filter(|w| !w.trim().is_empty()) else {
        bail!(NEEDS_KIT);
    };
    let secret = crate::kit::RecoveryCode::from_str(words)?.secret;
    let key = secret.account_key();
    ensure!(
        key.id() == header.account,
        "these words are for a different account than this backup"
    );
    Ok(Some(backup_key(&key)))
}

/// The error when an encrypted backup comes from an earlier account key: the
/// screen asks for that kit's words.
pub const NEEDS_KIT: &str =
    "this backup was encrypted with an earlier recovery kit: type that kit's 12 words";

fn aead(key: &[u8; 32]) -> LessSafeKey {
    LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key).expect("a 32-byte key"))
}

fn nonce(prefix: &[u8; 7], counter: u32, last: bool) -> Nonce {
    let mut bytes = [0u8; 12];
    bytes[..7].copy_from_slice(prefix);
    bytes[7..11].copy_from_slice(&counter.to_be_bytes());
    bytes[11] = u8::from(last);
    Nonce::assume_unique_for_key(bytes)
}

/// How much a backup holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BackupCounts {
    pub changes: u64,
    pub photos: u64,
    pub bytes: u64,
}

/// Writes a backup.
pub struct BackupWriter<W: Write> {
    out: W,
    header_bytes: Vec<u8>,
    prefix: [u8; 7],
    key: Option<LessSafeKey>,
    pending: Vec<u8>,
    counter: u32,
    counts: BackupCounts,
}

impl<W: Write> BackupWriter<W> {
    /// Starts a backup of `account`, encrypted with `key` if one is given
    /// (`backup_key`).
    pub fn new(
        mut out: W,
        account: AccountId,
        device: DeviceId,
        made_at: u64,
        key: Option<[u8; 32]>,
    ) -> Result<Self> {
        let header = BackupHeader {
            account,
            device,
            made_at,
            encrypted: key.is_some(),
            nonce_prefix: rand::random(),
        };
        let header_bytes = postcard::to_stdvec(&header)?;
        out.write_all(MAGIC)?;
        out.write_all(&[FORMAT])?;
        out.write_all(&(header_bytes.len() as u32).to_le_bytes())?;
        out.write_all(&header_bytes)?;
        Ok(Self {
            out,
            header_bytes,
            prefix: header.nonce_prefix,
            key: key.as_ref().map(aead),
            pending: Vec::new(),
            counter: 0,
            counts: BackupCounts::default(),
        })
    }

    pub fn change(&mut self, change: &Change) -> Result<()> {
        self.counts.changes += 1;
        self.record(&RecordRef::Change(change))
    }

    pub fn blob(&mut self, hash: BlobHash, data: &[u8]) -> Result<()> {
        self.counts.photos += 1;
        self.record(&RecordRef::Blob { hash, data })
    }

    /// Ends the backup and returns what it holds.
    pub fn finish(mut self) -> Result<(W, BackupCounts)> {
        self.record(&RecordRef::End)?;
        self.seal(true)?;
        self.out.flush()?;
        Ok((self.out, self.counts))
    }

    fn record(&mut self, record: &RecordRef<'_>) -> Result<()> {
        let bytes = postcard::to_stdvec(record)?;
        self.counts.bytes += bytes.len() as u64;
        self.pending
            .extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        self.pending.extend_from_slice(&bytes);
        if self.key.is_none() {
            self.out.write_all(&self.pending)?;
            self.pending.clear();
            return Ok(());
        }
        while self.pending.len() > CHUNK {
            self.seal(false)?;
        }
        Ok(())
    }

    /// Seals the next chunk (everything left, if `last`).
    fn seal(&mut self, last: bool) -> Result<()> {
        let Some(key) = &self.key else {
            return Ok(());
        };
        let take = if last { self.pending.len() } else { CHUNK };
        let mut chunk: Vec<u8> = self.pending.drain(..take).collect();
        key.seal_in_place_append_tag(
            nonce(&self.prefix, self.counter, last),
            Aad::from(&self.header_bytes),
            &mut chunk,
        )
        .map_err(|_| anyhow::anyhow!("couldn't encrypt the backup"))?;
        self.counter = self
            .counter
            .checked_add(1)
            .context("the backup is too large")?;
        self.out.write_all(&(chunk.len() as u32).to_le_bytes())?;
        self.out.write_all(&chunk)?;
        Ok(())
    }
}

/// Reads a backup.
pub struct BackupReader<R: Read> {
    input: R,
    header: BackupHeader,
    header_bytes: Vec<u8>,
    key: Option<LessSafeKey>,
    plain: Vec<u8>,
    at: usize,
    counter: u32,
    last_seen: bool,
}

impl<R: Read> BackupReader<R> {
    /// Opens a backup and reads its header.
    pub fn open(mut input: R) -> Result<Self> {
        let mut magic = [0u8; 9];
        input
            .read_exact(&mut magic)
            .context("this isn't a Mirrorleaf backup")?;
        ensure!(&magic[..8] == MAGIC, "this isn't a Mirrorleaf backup");
        ensure!(
            magic[8] == FORMAT,
            "this backup was made by a newer version of Mirrorleaf: update this one first"
        );
        let header_bytes = read_frame(&mut input)?.context("the backup is damaged")?;
        let header: BackupHeader =
            postcard::from_bytes(&header_bytes).context("the backup is damaged")?;
        Ok(Self {
            input,
            header,
            header_bytes,
            key: None,
            plain: Vec::new(),
            at: 0,
            counter: 0,
            last_seen: false,
        })
    }

    pub fn header(&self) -> &BackupHeader {
        &self.header
    }

    /// The key for an encrypted backup (`backup_key`).
    pub fn unlock(&mut self, key: [u8; 32]) {
        self.key = Some(aead(&key));
    }

    /// The next record, or `None` after `End`.
    pub fn next_record(&mut self) -> Result<Option<Record>> {
        let len = u32::from_le_bytes(self.take::<4>()?) as usize;
        ensure!(len <= MAX_FRAME, "the backup is damaged");
        let bytes = self.take_vec(len)?;
        match postcard::from_bytes(&bytes).context("the backup is damaged")? {
            Record::End => Ok(None),
            record => Ok(Some(record)),
        }
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let bytes = self.take_vec(N)?;
        Ok(bytes.try_into().expect("N bytes"))
    }

    fn take_vec(&mut self, n: usize) -> Result<Vec<u8>> {
        if !self.header.encrypted {
            let mut bytes = vec![0u8; n];
            self.input
                .read_exact(&mut bytes)
                .context("the backup ends early: it was cut short")?;
            return Ok(bytes);
        }
        while self.plain.len() - self.at < n {
            self.open_chunk()?;
        }
        let bytes = self.plain[self.at..self.at + n].to_vec();
        self.at += n;
        Ok(bytes)
    }

    fn open_chunk(&mut self) -> Result<()> {
        ensure!(!self.last_seen, "the backup is damaged");
        let key = self
            .key
            .as_ref()
            .context("this backup is encrypted: it needs its key")?;
        let mut sealed =
            read_frame(&mut self.input)?.context("the backup ends early: it was cut short")?;
        // The last chunk is sealed with its flag set; try that if the other fails.
        let mut opened = None;
        for last in [false, true] {
            let mut attempt = sealed.clone();
            if let Ok(plain) = key.open_in_place(
                nonce(&self.header.nonce_prefix, self.counter, last),
                Aad::from(&self.header_bytes),
                &mut attempt,
            ) {
                opened = Some((plain.to_vec(), last));
                break;
            }
        }
        sealed.clear();
        let Some((plain, last)) = opened else {
            if self.counter == 0 {
                bail!(
                    "this backup was encrypted with a different key: use the recovery kit it was made with"
                );
            }
            bail!("the backup is damaged");
        };
        self.plain.drain(..self.at);
        self.at = 0;
        self.plain.extend_from_slice(&plain);
        self.counter += 1;
        self.last_seen = last;
        Ok(())
    }
}

/// A length-prefixed frame; `None` at a clean end of the input.
fn read_frame(input: &mut impl Read) -> Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match input.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(len) as usize;
    ensure!(len <= MAX_FRAME + 16, "the backup is damaged");
    let mut bytes = vec![0u8; len];
    input
        .read_exact(&mut bytes)
        .context("the backup ends early: it was cut short")?;
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(n: u64) -> Change {
        Change::yjs(
            DeviceId([1; 32]),
            n,
            "note",
            vec![n as u8; 300],
            1_794_200_000,
        )
    }

    fn write(key: Option<[u8; 32]>, photo: &[u8]) -> (Vec<u8>, AccountKey) {
        let account = AccountKey::generate();
        let mut writer = BackupWriter::new(
            Vec::new(),
            account.id(),
            DeviceId([1; 32]),
            1_794_200_000,
            key,
        )
        .unwrap();
        for n in 1..=500 {
            writer.change(&change(n)).unwrap();
        }
        writer.blob(BlobHash([9; 32]), photo).unwrap();
        let (bytes, counts) = writer.finish().unwrap();
        assert_eq!((counts.changes, counts.photos), (500, 1));
        (bytes, account)
    }

    fn read_all(bytes: &[u8], key: Option<[u8; 32]>) -> Result<Vec<Record>> {
        let mut reader = BackupReader::open(bytes)?;
        if let Some(key) = key {
            reader.unlock(key);
        }
        let mut records = Vec::new();
        while let Some(record) = reader.next_record()? {
            records.push(record);
        }
        Ok(records)
    }

    #[test]
    fn a_plain_backup_reads_back_without_a_key() {
        let photo = vec![7u8; 250_000];
        let (bytes, _) = write(None, &photo);
        let records = read_all(&bytes, None).unwrap();
        assert_eq!(records.len(), 501);
        assert_eq!(records[0], Record::Change(change(1)));
        assert_eq!(
            records[500],
            Record::Blob {
                hash: BlobHash([9; 32]),
                data: photo
            }
        );
        // Cut short: noticed.
        assert!(read_all(&bytes[..bytes.len() - 10], None).is_err());
    }

    #[test]
    fn an_encrypted_backup_needs_its_key_and_resists_tampering() {
        let photo = vec![7u8; 250_000];
        let key = [5u8; 32];
        let (bytes, _) = write(Some(key), &photo);
        assert!(!bytes.windows(300).any(|w| w == vec![3u8; 300].as_slice()));
        let records = read_all(&bytes, Some(key)).unwrap();
        assert_eq!(records.len(), 501);
        assert!(
            BackupReader::open(bytes.as_slice())
                .unwrap()
                .header()
                .encrypted
        );

        let wrong = read_all(&bytes, Some([6u8; 32])).unwrap_err();
        assert!(wrong.to_string().contains("different key"), "{wrong}");
        assert!(read_all(&bytes, None).is_err(), "no key");
        // A changed byte, or the last chunk missing, is noticed.
        let mut tampered = bytes.clone();
        let middle = tampered.len() / 2;
        tampered[middle] ^= 1;
        assert!(read_all(&tampered, Some(key)).is_err());
        let header_end = 9 + 4 + {
            let reader = BackupReader::open(bytes.as_slice()).unwrap();
            reader.header_bytes.len()
        };
        let first_chunk =
            u32::from_le_bytes(bytes[header_end..header_end + 4].try_into().unwrap()) as usize;
        let cut = &bytes[..header_end + 4 + first_chunk];
        assert!(read_all(cut, Some(key)).is_err());
    }

    #[test]
    fn the_backup_key_comes_from_the_account_key() {
        let account = AccountKey::from_recovery(&[4u8; 16]);
        assert_eq!(
            backup_key(&account),
            backup_key(&AccountKey::from_recovery(&[4u8; 16]))
        );
        assert_ne!(backup_key(&account), backup_key(&AccountKey::generate()));
        assert_ne!(backup_key(&account), account.to_bytes());
    }
}
