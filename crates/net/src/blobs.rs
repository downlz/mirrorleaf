use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use iroh::endpoint::Connection;
use iroh::protocol::ProtocolHandler;
use iroh_blobs::api::Store;
use iroh_blobs::api::blobs::BlobStatus;
use iroh_blobs::api::remote::GetProgressItem;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::store::mem::MemStore;
use iroh_blobs::{BlobsProtocol, Hash};
use mirrorleaf_proto::BlobHash;
use n0_future::StreamExt;

use crate::node::Shared;

/// Where a device keeps its blobs.
#[derive(Clone, Debug)]
pub enum BlobStorage {
    /// On disk in this folder. Only one process can have it open at a time.
    Disk(PathBuf),
    /// In memory, for tests and for commands that don't move photos.
    Memory,
}

/// The blob store and the protocol that serves it (plan §5.3 step 4, §5.5).
/// iroh-blobs checks every chunk against the blob's BLAKE3 hash as it arrives,
/// and a fetch only asks for the chunks the store doesn't hold yet, so an
/// interrupted transfer carries on where it stopped. Cloning shares the store.
#[derive(Clone)]
pub(crate) struct Blobs {
    store: StoreKind,
    protocol: BlobsProtocol,
}

/// An open photo store, shared by the nodes one process binds one after
/// another (ADR-005). The engine opens it once per process: iroh-blobs' file
/// store runs its own thread pool, and closing the store leaves one of those
/// threads behind each time, so opening it for every push-woken job leaked a
/// thread per job.
#[derive(Clone)]
pub struct PhotoStore(pub(crate) Blobs);

impl PhotoStore {
    pub async fn open(storage: &BlobStorage) -> Result<Self> {
        Ok(Self(Blobs::open(storage).await?))
    }

    /// Closes the store. Nodes bound with it can no longer move photos.
    pub async fn shutdown(&self) {
        self.0.shutdown().await;
    }
}

#[derive(Clone)]
enum StoreKind {
    Disk(FsStore),
    Memory(MemStore),
}

/// What one fetch moved.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fetched {
    /// Bytes read from the network, including iroh-blobs' hash data.
    pub bytes_read: u64,
    /// The blob's size.
    pub len: u64,
}

impl Blobs {
    pub(crate) async fn open(storage: &BlobStorage) -> Result<Self> {
        let store = match storage {
            BlobStorage::Disk(dir) => StoreKind::Disk(
                FsStore::load(dir)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))
                    .with_context(|| {
                        format!(
                            "can't open the photo store in {} (is another Mirrorleaf process using it?)",
                            dir.display()
                        )
                    })?,
            ),
            BlobStorage::Memory => StoreKind::Memory(MemStore::new()),
        };
        let protocol = BlobsProtocol::new(api(&store), None);
        Ok(Self { store, protocol })
    }

    fn api(&self) -> &Store {
        api(&self.store)
    }

    /// Moves the blobs staged in the database into the store. Returns them.
    pub(crate) async fn store_staged(&self, state: &Shared) -> Result<Vec<(BlobHash, u64)>> {
        let mut stored = Vec::new();
        loop {
            let staged = state.lock().unwrap().replica.staged_blobs(8)?;
            if staged.is_empty() {
                return Ok(stored);
            }
            for (hash, data) in staged {
                let len = data.len() as u64;
                let added = self
                    .api()
                    .blobs()
                    .add_bytes(data)
                    .with_named_tag(tag(hash))
                    .await?;
                ensure!(
                    added.hash.as_bytes() == &hash.0,
                    "staged blob {} doesn't match its hash",
                    hash.short()
                );
                state.lock().unwrap().replica.blob_stored(hash, len)?;
                stored.push((hash, len));
            }
        }
    }

    /// Fetches a blob over `conn`. With `stop_after`, gives up once that many
    /// payload bytes have arrived, leaving the blob part-stored (for tests of
    /// resuming).
    pub(crate) async fn fetch(
        &self,
        conn: &Connection,
        hash: BlobHash,
        stop_after: Option<u64>,
    ) -> Result<Fetched> {
        let progress = self
            .api()
            .remote()
            .fetch(conn.clone(), Hash::from_bytes(hash.0))
            .stream();
        let mut progress = std::pin::pin!(progress);
        let stats = loop {
            match progress.next().await {
                Some(GetProgressItem::Progress(read)) => {
                    if stop_after.is_some_and(|limit| read >= limit) {
                        bail!("stopped after {read} bytes");
                    }
                }
                Some(GetProgressItem::Done(stats)) => break stats,
                Some(GetProgressItem::Error(e)) => return Err(anyhow::Error::new(e)),
                None => bail!("the transfer ended without a result"),
            }
        };
        let BlobStatus::Complete { size } = self.status(hash).await? else {
            bail!("the peer doesn't hold the whole blob");
        };
        self.api()
            .tags()
            .set(tag(hash), Hash::from_bytes(hash.0))
            .await?;
        Ok(Fetched {
            bytes_read: stats.total_bytes_read(),
            len: size,
        })
    }

    pub(crate) async fn status(&self, hash: BlobHash) -> Result<BlobStatus> {
        Ok(self.api().blobs().status(Hash::from_bytes(hash.0)).await?)
    }

    /// The blob's content, if the store holds all of it.
    pub(crate) async fn get(&self, hash: BlobHash) -> Result<Option<Vec<u8>>> {
        if !matches!(self.status(hash).await?, BlobStatus::Complete { .. }) {
            return Ok(None);
        }
        let bytes = self
            .api()
            .blobs()
            .get_bytes(Hash::from_bytes(hash.0))
            .await?;
        Ok(Some(bytes.to_vec()))
    }

    /// Serves blob requests on a connection from a device already checked
    /// against the account's device list.
    pub(crate) async fn serve(&self, conn: Connection) {
        let _ = self.protocol.accept(conn).await;
    }

    pub(crate) async fn shutdown(&self) {
        let _ = self.api().shutdown().await;
    }
}

/// Reads one blob from a device's store without going online. Fails while
/// another process (`sync --watch`) has the store open.
pub async fn read_blob(storage: &BlobStorage, hash: BlobHash) -> Result<Option<Vec<u8>>> {
    let blobs = Blobs::open(storage).await?;
    let bytes = blobs.get(hash).await;
    blobs.shutdown().await;
    bytes
}

fn api(store: &StoreKind) -> &Store {
    match store {
        StoreKind::Disk(store) => store,
        StoreKind::Memory(store) => store,
    }
}

/// A named tag keeps a blob safe from iroh-blobs' garbage collection, which
/// Mirrorleaf doesn't switch on yet (removing unused photos comes with E11).
fn tag(hash: BlobHash) -> String {
    format!("photo/{hash}")
}
