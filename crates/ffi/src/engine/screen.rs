//! What the app's screen drives through Tauri commands (M5): accounts, the
//! note list, the editor bridge (plan §5.6) and photos. Not exported to Kotlin:
//! on Android the screen uses Tauri commands too (M6).

use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use mirrorleaf_core::photos;
use mirrorleaf_core::yrs::Update;
use mirrorleaf_core::yrs::updates::decoder::Decode;
use mirrorleaf_net::device;
use mirrorleaf_net::ledger::LedgerClient;
use mirrorleaf_net::{Account, DevTicket, PhotoStore, describe};
use mirrorleaf_proto::BlobHash;
use tracing::warn;

use super::{DIAL_TIMEOUT, Engine, EngineError, lock, profile};

/// How long a ticket waits for this device to reach a relay, so the ticket
/// carries an address that works beyond the local network.
const TICKET_WAIT: Duration = Duration::from_secs(10);
/// How often a waiting edit batch is checked.
const FLUSH_TICK: Duration = Duration::from_millis(250);

/// Something the screen should refresh for.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UiEvent {
    /// Changes arrived from another device: the note list may differ.
    NotesChanged,
    /// A photo blob arrived; images showing it can load the better copy.
    PhotoArrived { hash: String },
    /// Connections or the ledger's answer changed.
    StatusChanged,
}

pub(super) type Listener = Arc<dyn Fn(UiEvent) + Send + Sync>;
type Sink = Box<dyn Fn(Vec<u8>) + Send + Sync>;

/// The note open in the editor.
pub(super) struct OpenNote {
    doc_id: String,
    /// The last log row the editor has.
    cursor: i64,
    /// Sends other devices' updates to the editor.
    sink: Sink,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct NoteItem {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub updated_ms: i64,
    pub folder: Option<String>,
    pub pinned: bool,
    pub trashed: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct FolderItem {
    pub id: String,
    pub name: String,
}

/// Everything the sidebar and the note list show.
#[derive(Clone, Debug, serde::Serialize)]
pub struct NoteList {
    pub notes: Vec<NoteItem>,
    pub folders: Vec<FolderItem>,
}

/// A photo ready for the editor to place.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PhotoInfo {
    /// `mirrorleaf-blob://<full>?thumb=<thumbnail>` (plan §5.1).
    pub src: String,
    pub width: u32,
    pub height: u32,
}

impl Engine {
    /// The engine, if this process has opened it.
    pub fn get() -> Option<Arc<Self>> {
        lock(&super::ENGINE).clone()
    }

    /// Tells `listener` whenever the screen should refresh.
    pub fn set_ui_listener(&self, listener: impl Fn(UiEvent) + Send + Sync + 'static) {
        *lock(&self.ui) = Some(Arc::new(listener));
    }

    pub(super) fn notify(&self, event: UiEvent) {
        let listener = lock(&self.ui).clone();
        if let Some(listener) = listener {
            listener(event);
        }
    }

    // --- Accounts (Phase 0 developer accounts, plan §5.4) ---

    /// Starts a developer account with this device as its first member.
    pub fn create_account(&self) -> Result<String, EngineError> {
        let mut replica = lock(&self.replica);
        if Account::load(&replica)?.is_some() {
            return Err(EngineError::Failed(
                "this device already belongs to an account".into(),
            ));
        }
        let account = Account::create(device::me(&replica, &self.secret)?);
        account.save(&mut replica)?;
        Ok(format!("Created account {}.", account.id().short()))
    }

    /// A ticket another device can join with. It carries the account key.
    pub async fn ticket(self: Arc<Self>) -> Result<String, EngineError> {
        let this = self.clone();
        self.on_runtime(async move {
            let node = this.begin_job().await?;
            let _ = tokio::time::timeout(TICKET_WAIT, node.online()).await;
            let ticket = {
                let state = node.state().lock().unwrap();
                let key = state
                    .account
                    .key
                    .as_ref()
                    .context("this device doesn't hold the account key")?;
                DevTicket {
                    account_secret: key.to_bytes(),
                    devices: state.account.devices.clone(),
                    inviter: node.addr(),
                }
            };
            this.end_job().await;
            Ok(ticket.to_string())
        })
        .await
    }

    /// Joins a developer account from a ticket, then syncs with the inviting
    /// device.
    pub async fn join(self: Arc<Self>, ticket: String) -> Result<String, EngineError> {
        let this = self.clone();
        self.on_runtime(async move {
            let ticket: DevTicket = ticket.trim().parse()?;
            {
                let mut replica = lock(&this.replica);
                if Account::load(&replica)?.is_some() {
                    bail!("this device already belongs to an account");
                }
                let me = device::me(&replica, &this.secret)?;
                Account::join(ticket.account_key(), &ticket.devices, me)?.save(&mut replica)?;
            }
            let node = this.begin_job().await?;
            let outcome =
                match tokio::time::timeout(DIAL_TIMEOUT, node.sync_once(ticket.inviter)).await {
                    Ok(Ok((path, report))) => format!(
                        "Joined. Synced with the inviting device via {path}: received {} changes.",
                        report.received_changes
                    ),
                    Ok(Err(e)) => format!("Joined, but couldn't sync yet: {}", describe(&e)),
                    Err(_) => "Joined, but the inviting device didn't answer yet.".to_owned(),
                };
            this.end_job().await;
            this.notify(UiEvent::NotesChanged);
            Ok(outcome)
        })
        .await
    }

    /// Registers this device with the ledger at `url` and reports to it from now on.
    pub async fn use_ledger(self: Arc<Self>, url: String) -> Result<String, EngineError> {
        let this = self.clone();
        self.on_runtime(async move {
            let url = url.trim().trim_end_matches('/').to_owned();
            let (account, profile) = {
                let replica = lock(&this.replica);
                let account = Account::load(&replica)?.context("join an account first")?;
                (account, profile(&replica)?)
            };
            let client = LedgerClient::new(&url, &this.secret, &account, profile.clone())?;
            client.register().await?;
            device::save_ledger(&mut lock(&this.replica), &url, &profile)?;
            this.refresh_ledger();
            Ok(format!("Registered with the ledger at {url}."))
        })
        .await
    }

    // --- The note list ---

    pub fn note_list(&self) -> Result<NoteList, EngineError> {
        let (notes, folders) = lock(&self.replica).list_index()?;
        let notes = notes
            .into_iter()
            .map(|n| NoteItem {
                id: n.id,
                title: n.title,
                preview: n.preview,
                updated_ms: n.updated_ms,
                folder: n.folder,
                pinned: n.pinned,
                trashed: n.trashed,
            })
            .collect();
        let folders = folders
            .into_iter()
            .map(|f| FolderItem {
                id: f.id,
                name: f.name,
            })
            .collect();
        Ok(NoteList { notes, folders })
    }

    /// A new note, in `folder` if given. With a `title` (typed in the search
    /// field, direction D), the note starts with that line, which is also its
    /// title; without one it is empty and its first line becomes its title as
    /// the user types.
    pub fn create_note(
        &self,
        folder: Option<String>,
        title: Option<String>,
    ) -> Result<String, EngineError> {
        let title = title.map(|t| t.trim().to_owned()).unwrap_or_default();
        let mut replica = lock(&self.replica);
        let id = replica.create_note_in(&title, folder.as_deref())?;
        if !title.is_empty() {
            replica.append_paragraph(&id, &title)?;
        }
        Ok(id)
    }

    pub fn trash_note(&self, id: &str) -> Result<(), EngineError> {
        Ok(lock(&self.replica).trash_note(id)?)
    }

    pub fn restore_note(&self, id: &str) -> Result<(), EngineError> {
        Ok(lock(&self.replica).restore_note(id)?)
    }

    pub fn set_pinned(&self, id: &str, pinned: bool) -> Result<(), EngineError> {
        Ok(lock(&self.replica).set_pinned(id, pinned)?)
    }

    pub fn move_note(&self, id: &str, folder: Option<String>) -> Result<(), EngineError> {
        Ok(lock(&self.replica).move_note(id, folder.as_deref())?)
    }

    pub fn create_folder(&self, name: &str) -> Result<String, EngineError> {
        Ok(lock(&self.replica).create_folder(name)?)
    }

    pub fn rename_folder(&self, id: &str, name: &str) -> Result<(), EngineError> {
        Ok(lock(&self.replica).rename_folder(id, name)?)
    }

    pub fn delete_folder(&self, id: &str) -> Result<(), EngineError> {
        Ok(lock(&self.replica).delete_folder(id)?)
    }

    /// Records the title and preview the editor shows for a note.
    pub fn set_note_meta(&self, id: &str, title: &str, preview: &str) -> Result<(), EngineError> {
        lock(&self.replica).update_note_meta(id, title, preview)?;
        Ok(())
    }

    // --- The editor bridge (plan §5.6) ---

    /// Opens a note in the editor: returns its whole state as one Yjs update,
    /// and from then on sends other devices' updates to it through `sink`.
    pub fn open_note(
        &self,
        id: &str,
        sink: impl Fn(Vec<u8>) + Send + Sync + 'static,
    ) -> Result<Vec<u8>, EngineError> {
        self.flush_edits()?;
        let (cursor, state) = {
            let replica = lock(&self.replica);
            replica.find_note(id)?;
            // The cursor is read first: an update that lands in between is in
            // both the state and the next forward, and Yjs ignores the repeat.
            (replica.last_row()?, replica.doc_state(id)?)
        };
        *lock(&self.editor) = Some(OpenNote {
            doc_id: id.to_owned(),
            cursor,
            sink: Box::new(sink),
        });
        self.forward_to_editor();
        Ok(state)
    }

    /// Stops sending updates to the editor, and stores its last edits.
    pub fn close_note(&self) -> Result<(), EngineError> {
        *lock(&self.editor) = None;
        self.flush_edits()
    }

    /// One update from the editor. Typing is merged into one change per note
    /// every 1.5 s (plan §5.2); [`Engine::flush_edits`] stores it sooner.
    pub fn edit_note(self: &Arc<Self>, id: &str, update: Vec<u8>) -> Result<(), EngineError> {
        Update::decode_v1(&update)
            .map_err(|e| EngineError::Failed(format!("the editor sent a bad update: {e}")))?;
        // The flag is set and cleared under the batcher's lock, so an update
        // can't arrive just as the flushing task stops and be left waiting.
        let start = {
            let mut batcher = lock(&self.batcher);
            batcher.push(id, update, Instant::now());
            !self.flushing.swap(true, Ordering::AcqRel)
        };
        if start {
            let this = self.clone();
            self.runtime.spawn(async move {
                loop {
                    tokio::time::sleep(FLUSH_TICK).await;
                    let due = lock(&this.batcher).is_due(Instant::now());
                    if due && let Err(e) = this.flush_edits() {
                        warn!("couldn't store edits: {e}");
                    }
                    let batcher = lock(&this.batcher);
                    if batcher.is_empty() {
                        this.flushing.store(false, Ordering::Release);
                        break;
                    }
                }
            });
        }
        Ok(())
    }

    /// Stores the editor's waiting updates as changes now: before switching
    /// notes, when the window closes and when the app quits (flush-on-close).
    pub fn flush_edits(&self) -> Result<(), EngineError> {
        let mut batcher = lock(&self.batcher);
        if batcher.is_empty() {
            return Ok(());
        }
        let mut replica = lock(&self.replica);
        batcher.flush(&mut replica)?;
        Ok(())
    }

    /// Sends the open note's new updates from other devices to the editor.
    pub(super) fn forward_to_editor(&self) {
        let mut editor = lock(&self.editor);
        let Some(open) = editor.as_mut() else {
            return;
        };
        let found = lock(&self.replica).doc_changes_after(&open.doc_id, open.cursor);
        match found {
            Ok((updates, cursor)) => {
                open.cursor = cursor;
                for update in updates {
                    (open.sink)(update);
                }
            }
            Err(e) => warn!("couldn't read changes for the editor: {e}"),
        }
    }

    // --- Photos (plan §5.5, §5.6) ---

    /// Prepares a photo (resized, at most 600 KB, plus a thumbnail) and stages
    /// it for the photo store. The editor places it where the cursor is.
    pub async fn import_photo(self: Arc<Self>, bytes: Vec<u8>) -> Result<PhotoInfo, EngineError> {
        let prepared = self
            .runtime
            .spawn_blocking(move || {
                let decoder = photos::system_decoder();
                photos::prepare(&bytes, decoder.as_deref())
            })
            .await
            .map_err(|e| EngineError::Failed(format!("photo import failed: {e}")))??;
        lock(&self.replica).stage_photo(&prepared)?;
        Ok(PhotoInfo {
            src: prepared.photo.to_uri(),
            width: prepared.width,
            height: prepared.height,
        })
    }

    /// The best copy of a photo this device holds: the full image, else the
    /// thumbnail, in which case the full image is asked for and live sync
    /// fetches it (plan §5.5: "the full image follows when the note is
    /// opened"). Returns the bytes and whether they are the full image.
    pub async fn photo(
        self: Arc<Self>,
        full: String,
        thumbnail: String,
    ) -> Result<Option<(Vec<u8>, bool)>, EngineError> {
        let this = self.clone();
        self.on_runtime(async move {
            let full = BlobHash::from_str(&full).context("not a photo address")?;
            let thumbnail = BlobHash::from_str(&thumbnail).context("not a photo address")?;
            if let Some(bytes) = this.blob(full).await? {
                return Ok(Some((bytes, true)));
            }
            let thumb = this.blob(thumbnail).await?;
            if thumb.is_some() {
                lock(&this.replica).want_blob(full)?;
            }
            Ok(thumb.map(|bytes| (bytes, false)))
        })
        .await
    }

    /// A blob from the database's staging table or the photo store.
    async fn blob(&self, hash: BlobHash) -> anyhow::Result<Option<Vec<u8>>> {
        if let Some(staged) = lock(&self.replica).staged_blob(hash)? {
            return Ok(Some(staged));
        }
        let storage = self.dir.blob_storage();
        let store = self
            .photos
            .get_or_try_init(|| async move { PhotoStore::open(&storage).await })
            .await?;
        store.get(hash).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use mirrorleaf_core::yrs::{
        Doc, GetString, ReadTxn, Transact, WriteTxn, XmlElementPrelim, XmlFragment, XmlOut,
        XmlTextPrelim,
    };
    use mirrorleaf_core::{CONTENT_FRAGMENT, DeviceId, Replica, VersionVector};
    use mirrorleaf_net::device::{DB_FILE, DeviceDir};

    use super::*;

    /// A data folder of its own for each test.
    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mirrorleaf-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn paragraphs(doc: &Doc) -> Vec<String> {
        let txn = doc.transact();
        let Some(content) = txn.get_xml_fragment(CONTENT_FRAGMENT) else {
            return Vec::new();
        };
        content
            .children(&txn)
            .filter_map(|node| match node {
                XmlOut::Element(p) => p.children(&txn).find_map(|c| match c {
                    XmlOut::Text(t) => Some(t.get_string(&txn)),
                    _ => None,
                }),
                _ => None,
            })
            .collect()
    }

    /// Types a paragraph in the "editor" and returns that edit's update.
    fn type_paragraph(editor: &Doc, text: &str) -> Vec<u8> {
        let mut txn = editor.transact_mut();
        let content = txn.get_or_insert_xml_fragment(CONTENT_FRAGMENT);
        let end = content.len(&txn);
        let p = content.insert(&mut txn, end, XmlElementPrelim::empty("paragraph"));
        p.insert(&mut txn, 0, XmlTextPrelim::new(text));
        txn.encode_update_v1()
    }

    #[test]
    fn the_editor_is_batched_and_hears_only_other_devices() {
        let dir = temp_dir("editor");
        let engine = Arc::new(Engine::start(dir.to_str().unwrap(), "Test").unwrap());
        let id = engine.create_note(None, None).unwrap();
        let before = lock(&engine.replica).change_count().unwrap();

        let (sent, heard) = mpsc::channel();
        let state = engine
            .open_note(&id, move |update| sent.send(update).unwrap())
            .unwrap();
        let editor = Doc::new();
        editor
            .transact_mut()
            .apply_update(Update::decode_v1(&state).unwrap())
            .unwrap();

        // Typing: several updates, one change once flushed (plan §5.2).
        for word in ["one", "two", "three"] {
            let update = type_paragraph(&editor, word);
            engine.edit_note(&id, update).unwrap();
        }
        assert!(
            engine.edit_note(&id, vec![1, 2, 3]).is_err(),
            "a bad update is refused"
        );
        engine.flush_edits().unwrap();
        assert_eq!(lock(&engine.replica).change_count().unwrap(), before + 1);
        engine.forward_to_editor();
        assert!(
            heard.try_recv().is_err(),
            "the editor's own edits aren't echoed"
        );

        // Another device adds a paragraph; its change arrives through sync,
        // which uses its own database connection.
        let mut other = Replica::open_in_memory(DeviceId([9; 32])).unwrap();
        let (mine, _) = lock(&engine.replica)
            .changes_since(&VersionVector::default(), 0, usize::MAX)
            .unwrap();
        other.apply_remote(&mine).unwrap();
        other.append_paragraph(&id, "theirs").unwrap();
        let (theirs, _) = other
            .changes_since(
                &lock(&engine.replica).version_vector().unwrap(),
                0,
                usize::MAX,
            )
            .unwrap();
        let (secret, _) = DeviceDir::new(&dir).open().unwrap();
        let mut sync_side = Replica::open(
            dir.join(DB_FILE),
            mirrorleaf_net::device::device_of(&secret),
        )
        .unwrap();
        sync_side.apply_remote(&theirs).unwrap();

        engine.forward_to_editor();
        let update = heard.recv_timeout(Duration::from_secs(1)).unwrap();
        editor
            .transact_mut()
            .apply_update(Update::decode_v1(&update).unwrap())
            .unwrap();
        assert_eq!(paragraphs(&editor), ["one", "two", "three", "theirs"]);

        // Closing stops the updates.
        engine.close_note().unwrap();
        other.append_paragraph(&id, "later").unwrap();
        engine.forward_to_editor();
        assert!(heard.try_recv().is_err());
        drop(engine);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_note_created_from_search_starts_with_its_title() {
        let dir = temp_dir("create");
        let engine = Arc::new(Engine::start(dir.to_str().unwrap(), "Test").unwrap());
        let id = engine
            .create_note(None, Some("  Packing list ".into()))
            .unwrap();
        let replica = lock(&engine.replica);
        assert_eq!(replica.note_paragraphs(&id).unwrap(), ["Packing list"]);
        assert_eq!(replica.find_note(&id).unwrap().title, "Packing list");
        drop(replica);
        drop(engine);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn photos_show_the_best_copy_held() {
        let dir = temp_dir("photos");
        let engine = Arc::new(Engine::start(dir.to_str().unwrap(), "Test").unwrap());
        let mut png = Vec::new();
        image::RgbImage::from_fn(640, 480, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        })
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();

        let info = engine
            .runtime
            .block_on(engine.clone().import_photo(png))
            .unwrap();
        assert_eq!((info.width, info.height), (640, 480));
        let photo = mirrorleaf_core::PhotoRef::parse(&info.src).unwrap();
        let (full, thumb) = (photo.full.to_string(), photo.thumbnail.to_string());

        // Staged for the photo store, and shown from there before any sync.
        let (bytes, is_full) = engine
            .runtime
            .block_on(engine.clone().photo(full.clone(), thumb.clone()))
            .unwrap()
            .unwrap();
        assert!(is_full);
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "a JPEG");

        // With only the thumbnail here, the thumbnail stands in.
        let elsewhere = "e".repeat(64);
        let (_, is_full) = engine
            .runtime
            .block_on(engine.clone().photo(elsewhere.clone(), thumb))
            .unwrap()
            .unwrap();
        assert!(!is_full);
        // Nothing held at all, or not a photo address.
        let nothing = engine
            .runtime
            .block_on(engine.clone().photo(elsewhere.clone(), elsewhere))
            .unwrap();
        assert!(nothing.is_none());
        assert!(
            engine
                .runtime
                .block_on(engine.clone().photo("x".into(), full))
                .is_err()
        );
        drop(engine);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn typing_is_stored_by_the_flushing_task() {
        let dir = temp_dir("flush");
        let engine = Arc::new(Engine::start(dir.to_str().unwrap(), "Test").unwrap());
        let id = engine.create_note(None, None).unwrap();
        let before = lock(&engine.replica).change_count().unwrap();
        engine.open_note(&id, |_| {}).unwrap();
        let editor = Doc::new();
        engine
            .edit_note(&id, type_paragraph(&editor, "hello"))
            .unwrap();
        // Nothing is stored until the batch window has passed.
        assert_eq!(lock(&engine.replica).change_count().unwrap(), before);
        std::thread::sleep(mirrorleaf_core::BATCH_WINDOW + FLUSH_TICK * 3);
        assert_eq!(lock(&engine.replica).change_count().unwrap(), before + 1);
        assert!(
            !engine.flushing.load(Ordering::Acquire),
            "the task stops when idle"
        );
        drop(engine);
        let _ = std::fs::remove_dir_all(dir);
    }
}
