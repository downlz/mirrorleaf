use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use iroh::endpoint::{Connection, Incoming, presets};
use iroh::{Endpoint, EndpointAddr, PublicKey, RelayMode, SecretKey};
use mirrorleaf_core::{BlobKind, Replica};
use mirrorleaf_proto::ledger::Approval;
use mirrorleaf_proto::pair::PAIR_ALPN;
use mirrorleaf_proto::{ALPN, AccountId, BlobHash, DeviceId, VersionVector};
use tokio::sync::mpsc::UnboundedSender;

use crate::account::Account;
use crate::blobs::{BlobStorage, Blobs, PhotoStore};
use crate::frame::Traffic;
use crate::kit::RecoverySecret;
use crate::ledger::{HEARTBEAT, LedgerClient, MIN_REPORT_INTERVAL, ONLINE_FOR_SECS};
use crate::pair::{self, Invitation, OfferSlot, PairingEvent, WordCode};
use crate::session::{Role, RoundReport, handshake, round};
use crate::{AuthError, describe};

/// QUIC close code sent when a peer is refused, with the reason as text.
const REFUSED: u32 = 1;

/// How often `watch` checks for new local changes to push.
const TICK: Duration = Duration::from_secs(1);
/// How long `watch` waits before dialling an unreachable device again.
const REDIAL: Duration = Duration::from_secs(30);
const DIAL_TIMEOUT: Duration = Duration::from_secs(20);
/// Longest wait for one photo blob before trying it elsewhere.
const BLOB_TIMEOUT: Duration = Duration::from_secs(60);

/// Everything a sync needs from this device.
pub struct State {
    pub replica: Replica,
    pub account: Account,
}

pub(crate) type Shared = Arc<Mutex<State>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Network {
    /// n0's public relays and address lookup (Phase 0; our own from Phase 1, E10).
    Internet,
    /// No relays and no address lookup: peers are dialled at known addresses.
    /// For tests on one machine.
    Local,
}

/// Whether a connection's packets go straight between the devices or through a
/// relay (recorded in the cross-network test, plan §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    Direct,
    Relay,
    Unknown,
}

impl fmt::Display for PathKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PathKind::Direct => "direct",
            PathKind::Relay => "relay",
            PathKind::Unknown => "unknown path",
        })
    }
}

pub fn path_kind(conn: &Connection) -> PathKind {
    conn.paths()
        .iter()
        .find(|path| path.is_selected())
        .map(|path| {
            if path.remote_addr().is_relay() {
                PathKind::Relay
            } else {
                PathKind::Direct
            }
        })
        .unwrap_or(PathKind::Unknown)
}

/// One photo blob that arrived.
#[derive(Clone, Debug)]
pub struct BlobReport {
    pub hash: BlobHash,
    /// Bytes read from the network, including iroh-blobs' hash data.
    pub bytes_read: u64,
    /// The blob's size.
    pub len: u64,
    pub took: Duration,
}

/// A photo blob a peer couldn't send.
#[derive(Clone, Debug)]
pub struct BlobFailure {
    pub hash: BlobHash,
    pub kind: BlobKind,
    pub error: String,
}

/// What one round of fetching photos from a peer did.
#[derive(Clone, Debug, Default)]
pub struct FetchRound {
    /// How the peer was reached; `None` if nothing needed fetching.
    pub path: Option<PathKind>,
    pub fetched: Vec<(BlobKind, BlobReport)>,
    pub failed: Vec<BlobFailure>,
}

/// What `watch` reports as it happens.
#[derive(Debug)]
pub enum Event {
    Connected {
        peer: DeviceId,
        path: PathKind,
        incoming: bool,
    },
    Synced {
        peer: DeviceId,
        path: PathKind,
        report: RoundReport,
        took: Duration,
    },
    Failed {
        peer: Option<DeviceId>,
        error: String,
    },
    Disconnected {
        peer: DeviceId,
    },
    /// A photo blob arrived from a peer.
    Fetched {
        peer: DeviceId,
        path: PathKind,
        kind: BlobKind,
        report: BlobReport,
    },
    /// A photo added on this device entered the blob store.
    Stored {
        hash: BlobHash,
        len: u64,
    },
    /// No connected device could send a photo blob; it is tried again later.
    FetchFailed {
        hash: BlobHash,
        kind: BlobKind,
        error: String,
    },
    /// The ledger took a status report and answered with the account's devices.
    LedgerReported {
        response: mirrorleaf_proto::ledger::StatusResponse,
    },
    LedgerFailed {
        error: String,
    },
    /// The device list changed, or the account key was replaced (E3), and
    /// the connections started again.
    DevicesChanged {
        account_replaced: bool,
    },
}

/// This device on the network: an Iroh endpoint whose key is the device ID.
#[derive(Clone)]
pub struct SyncNode {
    inner: Arc<Inner>,
}

struct Inner {
    endpoint: Endpoint,
    state: Shared,
    hints: Mutex<HashMap<DeviceId, EndpointAddr>>,
    blobs: Blobs,
    /// Whether this node opened its photo store, and so closes it.
    owns_store: bool,
    ledger: Mutex<Option<Arc<LedgerClient>>>,
    /// The invitation to join the account this node is showing, if any (E1).
    offer: OfferSlot,
    /// Wakes `watch` before its next tick, to push new changes at once.
    nudge: tokio::sync::Notify,
}

/// What `watch` last told the ledger.
#[derive(Default)]
struct Reporting {
    busy: bool,
    last_attempt: Option<Instant>,
    /// What the ledger last accepted.
    reported: Option<VersionVector>,
    last_success: Option<Instant>,
}

/// A connected peer, as `watch` tracks it.
struct Peer {
    conn: Connection,
    /// What the peer is known to hold, so `watch` only pushes when it has more.
    holds: VersionVector,
    busy: bool,
}

type Peers = Arc<Mutex<HashMap<DeviceId, Peer>>>;

/// When each blob that no connected device could send was last tried.
type Backoff = Arc<Mutex<HashMap<BlobHash, Instant>>>;

impl SyncNode {
    /// Binds this device's endpoint and opens its blob store. Staged photos
    /// stay staged until [`SyncNode::store_staged`] or `watch` moves them, so a
    /// node with a memory store (a ticket, a join) never takes them.
    pub async fn bind(
        state: State,
        secret: SecretKey,
        network: Network,
        storage: BlobStorage,
    ) -> Result<Self> {
        let blobs = Blobs::open(&storage).await?;
        Self::bind_inner(state, secret, network, blobs, true).await
    }

    /// Binds this device's endpoint with a photo store opened earlier and
    /// shared with other nodes. Closing the node leaves the store open.
    pub async fn bind_with_store(
        state: State,
        secret: SecretKey,
        network: Network,
        store: &PhotoStore,
    ) -> Result<Self> {
        Self::bind_inner(state, secret, network, store.0.clone(), false).await
    }

    async fn bind_inner(
        state: State,
        secret: SecretKey,
        network: Network,
        blobs: Blobs,
        owns_store: bool,
    ) -> Result<Self> {
        ensure!(
            DeviceId(*secret.public().as_bytes()) == state.replica.device(),
            "the device key doesn't match this database"
        );
        let builder = match network {
            Network::Internet => Endpoint::builder(presets::N0),
            Network::Local => Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled),
        };
        let endpoint = builder
            .secret_key(secret)
            .alpns(vec![
                ALPN.to_vec(),
                PAIR_ALPN.to_vec(),
                iroh_blobs::ALPN.to_vec(),
            ])
            .bind()
            .await?;
        Ok(Self {
            inner: Arc::new(Inner {
                endpoint,
                state: Arc::new(Mutex::new(state)),
                hints: Mutex::default(),
                blobs,
                owns_store,
                ledger: Mutex::default(),
                offer: Arc::default(),
                nudge: tokio::sync::Notify::new(),
            }),
        })
    }

    /// Reports to this ledger while watching: at once, then whenever the
    /// change log grows (at most every 30 s), and every 5 minutes as the
    /// heartbeat (plan M3, ADR-004).
    pub fn use_ledger(&self, ledger: LedgerClient) {
        *self.inner.ledger.lock().unwrap() = Some(Arc::new(ledger));
    }

    /// The ledger client, unless this device was removed from the account:
    /// a removed device still holds the key, and mustn't register again (E3).
    pub fn ledger(&self) -> Option<Arc<LedgerClient>> {
        let me = self.device();
        if self.inner.state.lock().unwrap().account.removed(&me) {
            return None;
        }
        self.inner.ledger.lock().unwrap().clone()
    }

    /// Tells the ledger this device is going offline, so it stops counting on
    /// it straight away. Best effort: gives up after a few seconds.
    pub async fn go_offline(&self) -> Result<()> {
        let Some(ledger) = self.ledger() else {
            return Ok(());
        };
        let vv = self.inner.state.lock().unwrap().replica.version_vector()?;
        tokio::time::timeout(Duration::from_secs(5), ledger.report(&vv, Some(0))).await??;
        Ok(())
    }

    pub fn state(&self) -> &Arc<Mutex<State>> {
        &self.inner.state
    }

    /// Shows an invitation for a new device to join the account (E1): a QR
    /// code, and `words` if the ledger gave this device a number. It works
    /// once, for 10 minutes, and replaces any earlier invitation. What
    /// happens arrives on `events`. Incoming pairing connections are served
    /// by `accept` and `watch`, so one of them must be running.
    pub fn invite(
        &self,
        words: Option<WordCode>,
        events: UnboundedSender<PairingEvent>,
    ) -> Invitation {
        let ledger = self.ledger().map(|l| l.url().to_owned());
        pair::invite(&self.inner.offer, self.addr(), words, ledger, events)
    }

    /// Stops showing the invitation, and withdraws its number from the ledger.
    pub fn cancel_invitation(&self) {
        let Some(offer) = self.inner.offer.lock().unwrap().take() else {
            return;
        };
        if let (Some(ledger), Some(nameplate)) = (self.ledger(), offer.nameplate()) {
            tokio::spawn(async move {
                let _ = ledger.cancel_pairing(nameplate).await;
            });
        }
    }

    pub fn device(&self) -> DeviceId {
        DeviceId(*self.inner.endpoint.id().as_bytes())
    }

    /// Makes `watch` look for changes to push now rather than at its next
    /// tick (up to a second away): for the last edits before going offline.
    pub fn nudge(&self) {
        self.inner.nudge.notify_one();
    }

    /// Keeps the ledger client on this device's account after the account's
    /// key changed (E3): taken from another device, or replaced here.
    fn follow_account(&self) {
        let Some(ledger) = self.ledger() else {
            return;
        };
        let moved = {
            let state = self.inner.state.lock().unwrap();
            (ledger.account() != state.account.id()).then(|| ledger.for_account(&state.account))
        };
        match moved {
            Some(Ok(client)) => self.use_ledger(client),
            Some(Err(e)) => tracing::warn!("ledger: {}", describe(&e)),
            None => {}
        }
    }

    /// Replaces the account key (E3) and returns the new recovery kit's
    /// secret, which the caller shows once. Every device on the list moves to
    /// the new key except `remove`, which can't sync any more. The ledger moves
    /// the account first, so if another device replaced the key a moment
    /// earlier this one is refused and nothing changes here; other devices get
    /// the new key the next time they connect to one that has it.
    pub async fn replace_key(&self, remove: &[DeviceId]) -> Result<RecoverySecret> {
        let me = self.device();
        let now = unix_now();
        let (current, replaced, secret) = {
            let state = self.inner.state.lock().unwrap();
            let (replaced, secret) = state.account.replaced(&me, remove, now)?;
            (state.account.id(), replaced, secret)
        };
        if let Some(ledger) = self.ledger() {
            let link = replaced
                .devices
                .list
                .rotated_from
                .clone()
                .expect("a replaced account links to the old one");
            let devices = replaced.devices.list.devices.iter().map(|d| d.id).collect();
            ledger.rekey(&link, replaced.id(), devices).await?;
        }
        {
            let mut state = self.inner.state.lock().unwrap();
            ensure!(
                state.account.id() == current,
                "the account changed while its key was being replaced"
            );
            state.account = replaced;
            let state = &mut *state;
            state.account.save(&mut state.replica)?;
            // Kept until the person has saved the kit, so closing the app
            // midway doesn't lose it.
            Account::save_pending_kit(&mut state.replica, &secret, now)?;
        }
        self.follow_account();
        tracing::info!("replaced the account key");
        Ok(secret)
    }

    /// Removes another device from the account for good (E3): it is refused
    /// from now on, by this device at once and by the others as the new list
    /// reaches them. The ledger forgets it too, if it can be reached.
    pub async fn remove_device(&self, device: DeviceId) -> Result<()> {
        ensure!(
            device != self.device(),
            "to remove this device, sign out of it instead"
        );
        let approval = {
            let mut state = self.inner.state.lock().unwrap();
            let key = state
                .account
                .key
                .as_ref()
                .context("this device doesn't hold the account key")?;
            if !state.account.devices.contains(&device) {
                anyhow::bail!("that device isn't on the account");
            }
            let devices = key.remove_device(&state.account.devices, device);
            let approval = Approval::RemoveDevice.sign(key, &device);
            state.account.devices = devices;
            let state = &mut *state;
            state.account.save(&mut state.replica)?;
            approval
        };
        if let Some(ledger) = self.ledger()
            && let Err(e) = ledger.remove_device(device, approval).await
        {
            // Its ledger row goes unseen and is pruned after 90 days.
            tracing::warn!(
                "the ledger didn't remove {} yet: {}",
                device.short(),
                describe(&e)
            );
        }
        Ok(())
    }

    /// Takes this device off the account's list (signing out, E3) and tells
    /// the ledger. The others learn it the next time they meet this device
    /// or one that has met it: `announce` dials them once to make that soon.
    pub async fn leave_account(&self) -> Result<()> {
        let me = self.device();
        // Taken first: once off the list, this device no longer uses it.
        let ledger = self.ledger();
        {
            let mut state = self.inner.state.lock().unwrap();
            let key = state
                .account
                .key
                .as_ref()
                .context("this device doesn't hold the account key")?;
            let devices = key.remove_device(&state.account.devices, me);
            state.account.devices = devices;
            let state = &mut *state;
            state.account.save(&mut state.replica)?;
        }
        if let Some(ledger) = ledger
            && let Err(e) = ledger.leave().await
        {
            tracing::warn!("the ledger didn't remove this device: {}", describe(&e));
        }
        Ok(())
    }

    /// Dials every other device on the list once, so they learn of a change
    /// to the device list (their handshake merges ours even if they then
    /// refuse this device). Returns how many answered.
    pub async fn announce(&self) -> usize {
        let me = self.device();
        let peers: Vec<DeviceId> = {
            let state = self.inner.state.lock().unwrap();
            let list = &state.account.devices.list;
            list.devices
                .iter()
                .map(|d| d.id)
                .chain(list.removed.iter().copied())
                .filter(|d| *d != me)
                .collect()
        };
        let mut dials = tokio::task::JoinSet::new();
        for peer in peers {
            let node = self.clone();
            dials.spawn(async move {
                let addr = node.addr_for(peer).ok()?;
                match tokio::time::timeout(DIAL_TIMEOUT, node.tell(addr)).await {
                    Ok(Ok(true)) => Some(()),
                    _ => None,
                }
            });
        }
        let mut answered = 0;
        while let Some(result) = dials.join_next().await {
            if matches!(result, Ok(Some(()))) {
                answered += 1;
            }
        }
        answered
    }

    /// Our current address: relay and direct addresses as far as known.
    pub fn addr(&self) -> EndpointAddr {
        self.inner.endpoint.addr()
    }

    /// Waits until the endpoint has a relay connection and can be reached.
    pub async fn online(&self) {
        self.inner.endpoint.online().await
    }

    /// Our address on this machine's loopback interface, for tests.
    pub fn loopback_addr(&self) -> EndpointAddr {
        let port = self
            .inner
            .endpoint
            .bound_sockets()
            .into_iter()
            .find(SocketAddr::is_ipv4)
            .map(|s| s.port())
            .unwrap_or_default();
        EndpointAddr::new(self.inner.endpoint.id())
            .with_ip_addr(SocketAddr::from(([127, 0, 0, 1], port)))
    }

    /// Tells `watch` where a device can be reached, instead of looking it up.
    pub fn add_addr_hint(&self, device: DeviceId, addr: EndpointAddr) {
        self.inner.hints.lock().unwrap().insert(device, addr);
    }

    fn addr_for(&self, device: DeviceId) -> Result<EndpointAddr> {
        if let Some(addr) = self.inner.hints.lock().unwrap().get(&device) {
            return Ok(addr.clone());
        }
        endpoint_addr(device)
    }

    /// Dials a peer, authenticates it and runs a first sync round. The
    /// connection stays open for further rounds.
    pub async fn connect(
        &self,
        addr: impl Into<EndpointAddr>,
    ) -> Result<(Connection, RoundReport)> {
        let conn = self.inner.endpoint.connect(addr, ALPN).await?;
        let (mut send, mut recv) = conn.open_bi().await?;
        let mut traffic = Traffic::default();
        let shook = handshake(
            &self.inner.state,
            &mut send,
            &mut recv,
            device_of(&conn),
            &mut traffic,
        );
        refuse_on_error(&conn, shook.await)?;
        self.follow_account();
        let mut report = round(
            &self.inner.state,
            send,
            recv,
            Role::Opener,
            device_of(&conn),
        )
        .await?;
        report.traffic += traffic;
        Ok((conn, report))
    }

    /// Dials a peer to give it our device list: whether the handshake got
    /// through. A round follows (the peer may refuse it, having taken the
    /// list), so the peer has read our list before either side hangs up.
    async fn tell(&self, addr: EndpointAddr) -> Result<bool> {
        let conn = self.inner.endpoint.connect(addr, ALPN).await?;
        let (mut send, mut recv) = conn.open_bi().await?;
        let mut traffic = Traffic::default();
        let peer = device_of(&conn);
        let shook = handshake(&self.inner.state, &mut send, &mut recv, peer, &mut traffic).await;
        let told = match &shook {
            Ok(()) => {
                let _ = round(&self.inner.state, send, recv, Role::Opener, peer).await;
                true
            }
            // Hellos were exchanged, and the handshake waited for the peer
            // to read ours: it has our list, even if one side then refused.
            Err(e) => matches!(
                e.downcast_ref::<AuthError>(),
                Some(AuthError::Removed | AuthError::NotOnList)
            ),
        };
        conn.close(0u32.into(), b"done");
        Ok(told)
    }

    /// Dials a peer, syncs once and hangs up.
    pub async fn sync_once(
        &self,
        addr: impl Into<EndpointAddr>,
    ) -> Result<(PathKind, RoundReport)> {
        let (conn, report) = self.connect(addr).await?;
        let path = path_kind(&conn);
        conn.close(0u32.into(), b"done");
        Ok((path, report))
    }

    /// Runs another round on an open connection.
    pub async fn sync_round(&self, conn: &Connection) -> Result<RoundReport> {
        let (send, recv) = conn.open_bi().await?;
        round(&self.inner.state, send, recv, Role::Opener, device_of(conn)).await
    }

    /// Accepts the next incoming sync connection and serves its first round.
    /// Blob connections that arrive meanwhile are served in the background.
    /// Returns `None` once the endpoint is closed.
    pub async fn accept(&self) -> Option<Result<(Connection, RoundReport)>> {
        loop {
            let incoming = self.inner.endpoint.accept().await?;
            match self.accept_connection(incoming).await {
                Ok(None) => continue,
                Ok(Some(accepted)) => return Some(Ok(accepted)),
                Err(e) => return Some(Err(e)),
            }
        }
    }

    /// Serves an incoming connection: a blob connection in the background
    /// (`None`), or a sync connection's first round.
    async fn accept_connection(
        &self,
        incoming: Incoming,
    ) -> Result<Option<(Connection, RoundReport)>> {
        let conn = incoming.await?;
        if conn.alpn() == iroh_blobs::ALPN {
            self.serve_blobs(conn);
            return Ok(None);
        }
        if conn.alpn() == PAIR_ALPN {
            let (me, slot, state, ledger) = (
                self.device(),
                self.inner.offer.clone(),
                self.inner.state.clone(),
                self.ledger(),
            );
            tokio::spawn(async move {
                if let Err(e) = pair::serve(conn, me, slot, state, ledger).await {
                    tracing::warn!("pairing failed: {}", describe(&e));
                }
            });
            return Ok(None);
        }
        let (mut send, mut recv) = conn.accept_bi().await?;
        let mut traffic = Traffic::default();
        let shook = handshake(
            &self.inner.state,
            &mut send,
            &mut recv,
            device_of(&conn),
            &mut traffic,
        );
        refuse_on_error(&conn, shook.await)?;
        self.follow_account();
        let mut report = round(
            &self.inner.state,
            send,
            recv,
            Role::Responder,
            device_of(&conn),
        )
        .await?;
        report.traffic += traffic;
        Ok(Some((conn, report)))
    }

    /// Serves photo blobs to a device on the account's list, and refuses anyone
    /// else: iroh-blobs on its own would serve any peer (plan §5.3 step 2).
    fn serve_blobs(&self, conn: Connection) {
        let peer = device_of(&conn);
        if !self
            .inner
            .state
            .lock()
            .unwrap()
            .account
            .devices
            .contains(&peer)
        {
            let _ = refuse_on_error(&conn, Err(AuthError::NotOnList.into()));
            return;
        }
        let node = self.clone();
        tokio::spawn(async move { node.inner.blobs.serve(conn).await });
    }

    /// Moves photos staged in the database (by `photo add` in another process)
    /// into the blob store. Returns each blob stored and its size.
    pub async fn store_staged(&self) -> Result<Vec<(BlobHash, u64)>> {
        self.inner.blobs.store_staged(&self.inner.state).await
    }

    /// A blob's content, if this device holds all of it.
    pub async fn blob(&self, hash: BlobHash) -> Result<Option<Vec<u8>>> {
        self.inner.blobs.get(hash).await
    }

    /// Fetches one blob from `addr`. With `stop_after`, gives up once that many
    /// bytes have arrived and keeps what came, as an interrupted transfer would;
    /// the next fetch carries on from there.
    pub async fn fetch_blob(
        &self,
        addr: impl Into<EndpointAddr>,
        hash: BlobHash,
        stop_after: Option<u64>,
    ) -> Result<BlobReport> {
        let started = Instant::now();
        let conn = self.inner.endpoint.connect(addr, iroh_blobs::ALPN).await?;
        let fetched = self.fetch_on(&conn, hash, stop_after, started).await;
        conn.close(0u32.into(), b"done");
        fetched
    }

    /// Fetches every missing thumbnail and requested full image from `peer`
    /// (plan §5.3 step 4).
    pub async fn fetch_missing(&self, peer: DeviceId) -> Result<FetchRound> {
        let wanted = self.inner.state.lock().unwrap().replica.blobs_to_fetch()?;
        self.fetch_from(peer, wanted).await
    }

    /// Fetches `wanted` from `peer` over one blob connection.
    async fn fetch_from(
        &self,
        peer: DeviceId,
        wanted: Vec<(BlobHash, BlobKind)>,
    ) -> Result<FetchRound> {
        let mut fetch = FetchRound::default();
        if wanted.is_empty() {
            return Ok(fetch);
        }
        let addr = self.addr_for(peer)?;
        let conn = tokio::time::timeout(
            DIAL_TIMEOUT,
            self.inner.endpoint.connect(addr, iroh_blobs::ALPN),
        )
        .await??;
        for (hash, kind) in wanted {
            if self.holds(hash) {
                continue;
            }
            let started = Instant::now();
            let fetched =
                tokio::time::timeout(BLOB_TIMEOUT, self.fetch_on(&conn, hash, None, started)).await;
            match fetched {
                Ok(Ok(report)) => fetch.fetched.push((kind, report)),
                Ok(Err(e)) => fetch.failed.push(BlobFailure {
                    hash,
                    kind,
                    error: describe(&e),
                }),
                Err(_) => fetch.failed.push(BlobFailure {
                    hash,
                    kind,
                    error: "timed out".into(),
                }),
            }
        }
        fetch.path = Some(path_kind(&conn));
        conn.close(0u32.into(), b"done");
        Ok(fetch)
    }

    async fn fetch_on(
        &self,
        conn: &Connection,
        hash: BlobHash,
        stop_after: Option<u64>,
        started: Instant,
    ) -> Result<BlobReport> {
        let fetched = self.inner.blobs.fetch(conn, hash, stop_after).await?;
        self.inner
            .state
            .lock()
            .unwrap()
            .replica
            .blob_stored(hash, fetched.len)?;
        Ok(BlobReport {
            hash,
            bytes_read: fetched.bytes_read,
            len: fetched.len,
            took: started.elapsed(),
        })
    }

    fn holds(&self, hash: BlobHash) -> bool {
        let state = self.inner.state.lock().unwrap();
        state.replica.holds_blob(hash).unwrap_or(false)
    }

    /// Serves the next round the peer starts on `conn`. The time returned runs
    /// from when the peer opened the round, not from when we started waiting.
    pub async fn serve(&self, conn: &Connection) -> Result<(RoundReport, Duration)> {
        let (send, recv) = conn.accept_bi().await?;
        let started = Instant::now();
        let report = round(
            &self.inner.state,
            send,
            recv,
            Role::Responder,
            device_of(conn),
        )
        .await?;
        Ok((report, started.elapsed()))
    }

    pub async fn close(&self) {
        self.inner.endpoint.close().await;
        if self.inner.owns_store {
            self.inner.blobs.shutdown().await;
        }
    }

    /// Live mode (plan §5.3, step 5): stays connected to every device on the
    /// list that can be reached, and pushes new changes within about a second.
    /// Changes written by another process (the CLI's `note` commands) are
    /// picked up too. Runs until the endpoint closes or the database fails.
    pub async fn watch(&self, events: UnboundedSender<Event>) -> Result<()> {
        let peers: Peers = Arc::default();
        let acceptor = {
            let (node, peers, events) = (self.clone(), peers.clone(), events.clone());
            tokio::spawn(async move {
                while let Some(incoming) = node.inner.endpoint.accept().await {
                    let (node, peers, events) = (node.clone(), peers.clone(), events.clone());
                    tokio::spawn(async move {
                        let started = Instant::now();
                        match node.accept_connection(incoming).await {
                            Ok(Some((conn, report))) => {
                                node.track(&peers, conn, report, started, true, &events)
                            }
                            Ok(None) => {}
                            Err(e) => {
                                let error = describe(&e);
                                let _ = events.send(Event::Failed { peer: None, error });
                            }
                        }
                    });
                }
            })
        };

        let mut tick = tokio::time::interval(TICK);
        let mut last_dial: HashMap<DeviceId, Instant> = HashMap::new();
        let fetching = Arc::new(AtomicBool::new(false));
        let backoff: Backoff = Arc::default();
        let reporting: Arc<Mutex<Reporting>> = Arc::default();
        // Which account and device list the open connections were made under.
        let mut generation: Option<(AccountId, u64)> = None;
        let result = loop {
            tokio::select! {
                _ = tick.tick() => {}
                () = self.inner.nudge.notified() => {}
            }
            if self.inner.endpoint.is_closed() {
                break Ok(());
            }
            let (ours, devices, now) = {
                let state = self.inner.state.lock().unwrap();
                let devices: Vec<DeviceId> = state
                    .account
                    .devices
                    .list
                    .devices
                    .iter()
                    .map(|d| d.id)
                    .collect();
                let now = (state.account.id(), state.account.devices.list.version);
                match state.replica.version_vector() {
                    Ok(vv) => (vv, devices, now),
                    Err(e) => break Err(e.into()),
                }
            };
            // The device list travels only in the handshake, so when it
            // changes (a device added or removed, the key replaced) every
            // connection starts again: the others learn the change, and a
            // removed device is refused (E3).
            if let Some(before) = generation
                && before != now
            {
                let replaced = before.0 != now.0;
                self.follow_account();
                let dropped: Vec<(DeviceId, Peer)> = peers.lock().unwrap().drain().collect();
                for (id, peer) in dropped {
                    peer.conn.close(0u32.into(), b"device list changed");
                    let _ = events.send(Event::Disconnected { peer: id });
                }
                last_dial.clear();
                let _ = events.send(Event::DevicesChanged {
                    account_replaced: replaced,
                });
            }
            generation = Some(now);
            for peer in devices.into_iter().filter(|d| *d != self.device()) {
                let known = {
                    let mut map = peers.lock().unwrap();
                    match map.get_mut(&peer) {
                        Some(p) if !p.busy && ours.ahead_of(&p.holds) > 0 => {
                            p.busy = true;
                            Some(Some(p.conn.clone()))
                        }
                        Some(_) => Some(None),
                        None => None,
                    }
                };
                match known {
                    Some(Some(conn)) => self.spawn_push(&peers, peer, conn, &events),
                    Some(None) => {}
                    None if last_dial.get(&peer).is_some_and(|t| t.elapsed() < REDIAL) => {}
                    None => {
                        last_dial.insert(peer, Instant::now());
                        self.spawn_dial(&peers, peer, &events);
                    }
                }
            }

            match self.store_staged().await {
                Ok(stored) => {
                    for (hash, len) in stored {
                        let _ = events.send(Event::Stored { hash, len });
                    }
                }
                Err(e) => {
                    let error = describe(&e);
                    let _ = events.send(Event::Failed { peer: None, error });
                }
            }
            if !fetching.load(Ordering::Acquire) {
                self.spawn_fetch(&peers, &fetching, &backoff, &events);
            }
            self.maybe_report(&ours, &reporting, &events);
        };
        acceptor.abort();
        result
    }

    /// Sends a status report if one is due: the first, a changed log after
    /// `MIN_REPORT_INTERVAL`, or the heartbeat.
    fn maybe_report(
        &self,
        ours: &VersionVector,
        reporting: &Arc<Mutex<Reporting>>,
        events: &UnboundedSender<Event>,
    ) {
        let Some(ledger) = self.ledger() else {
            return;
        };
        {
            let mut r = reporting.lock().unwrap();
            let since = |t: Option<Instant>| t.map_or(Duration::MAX, |t| t.elapsed());
            // A changed log, or the heartbeat, but never more often than every
            // 30 s, so an unreachable ledger isn't asked every second.
            let changed = r.reported.as_ref() != Some(ours);
            let heartbeat = since(r.last_success) >= HEARTBEAT;
            let due = r.last_attempt.is_none()
                || (since(r.last_attempt) >= MIN_REPORT_INTERVAL && (changed || heartbeat));
            if r.busy || !due {
                return;
            }
            r.busy = true;
            r.last_attempt = Some(Instant::now());
        }
        let (vv, reporting, events) = (ours.clone(), reporting.clone(), events.clone());
        tokio::spawn(async move {
            let result = ledger.report(&vv, Some(ONLINE_FOR_SECS)).await;
            let mut r = reporting.lock().unwrap();
            r.busy = false;
            let _ = events.send(match result {
                Ok(response) => {
                    r.reported = Some(vv);
                    r.last_success = Some(Instant::now());
                    Event::LedgerReported { response }
                }
                Err(e) => Event::LedgerFailed {
                    error: describe(&e),
                },
            });
        });
    }

    /// Fetches missing thumbnails and requested full images from the connected
    /// devices, trying each in turn. A blob none of them could send waits
    /// `REDIAL` before it is tried again.
    fn spawn_fetch(
        &self,
        peers: &Peers,
        fetching: &Arc<AtomicBool>,
        backoff: &Backoff,
        events: &UnboundedSender<Event>,
    ) {
        let wanted = {
            let state = self.inner.state.lock().unwrap();
            match state.replica.blobs_to_fetch() {
                Ok(wanted) => wanted,
                Err(_) => return,
            }
        };
        let mut wanted: Vec<_> = {
            let backoff = backoff.lock().unwrap();
            wanted
                .into_iter()
                .filter(|(hash, _)| backoff.get(hash).is_none_or(|t| t.elapsed() >= REDIAL))
                .collect()
        };
        let connected: Vec<DeviceId> = peers.lock().unwrap().keys().copied().collect();
        if wanted.is_empty() || connected.is_empty() {
            return;
        }
        fetching.store(true, Ordering::Release);
        let (node, fetching, backoff, events) = (
            self.clone(),
            fetching.clone(),
            backoff.clone(),
            events.clone(),
        );
        tokio::spawn(async move {
            let mut last_error: HashMap<BlobHash, String> = HashMap::new();
            for peer in connected {
                wanted.retain(|(hash, _)| !node.holds(*hash));
                if wanted.is_empty() {
                    break;
                }
                match node.fetch_from(peer, wanted.clone()).await {
                    Ok(fetch) => {
                        let path = fetch.path.unwrap_or(PathKind::Unknown);
                        for (kind, report) in fetch.fetched {
                            let _ = events.send(Event::Fetched {
                                peer,
                                path,
                                kind,
                                report,
                            });
                        }
                        for failure in fetch.failed {
                            last_error.insert(failure.hash, failure.error);
                        }
                    }
                    Err(e) => {
                        let error = describe(&e);
                        for (hash, _) in &wanted {
                            last_error.insert(*hash, error.clone());
                        }
                    }
                }
            }
            wanted.retain(|(hash, _)| !node.holds(*hash));
            let mut backoff = backoff.lock().unwrap();
            for (hash, kind) in wanted {
                backoff.insert(hash, Instant::now());
                let error = last_error.remove(&hash).unwrap_or_default();
                let _ = events.send(Event::FetchFailed { hash, kind, error });
            }
            fetching.store(false, Ordering::Release);
        });
    }

    fn spawn_dial(&self, peers: &Peers, peer: DeviceId, events: &UnboundedSender<Event>) {
        let (node, peers, events) = (self.clone(), peers.clone(), events.clone());
        tokio::spawn(async move {
            let started = Instant::now();
            let dialled = async {
                let addr = node.addr_for(peer)?;
                tokio::time::timeout(DIAL_TIMEOUT, node.connect(addr)).await?
            };
            match dialled.await {
                Ok((conn, report)) => node.track(&peers, conn, report, started, false, &events),
                Err(e) => {
                    let error = describe(&e);
                    let _ = events.send(Event::Failed {
                        peer: Some(peer),
                        error,
                    });
                }
            }
        });
    }

    fn spawn_push(
        &self,
        peers: &Peers,
        peer: DeviceId,
        conn: Connection,
        events: &UnboundedSender<Event>,
    ) {
        let (node, peers, events) = (self.clone(), peers.clone(), events.clone());
        tokio::spawn(async move {
            let started = Instant::now();
            let result = node.sync_round(&conn).await;
            let mut map = peers.lock().unwrap();
            if let Some(p) = map.get_mut(&peer) {
                p.busy = false;
                if let Ok(report) = &result {
                    p.holds = p.holds.merged(&report.peer_holds);
                }
            }
            drop(map);
            let _ = events.send(match result {
                Ok(report) => Event::Synced {
                    peer,
                    path: path_kind(&conn),
                    report,
                    took: started.elapsed(),
                },
                Err(e) => Event::Failed {
                    peer: Some(peer),
                    error: describe(&e),
                },
            });
        });
    }

    /// Records a newly connected peer and serves the rounds it starts.
    fn track(
        &self,
        peers: &Peers,
        conn: Connection,
        report: RoundReport,
        started: Instant,
        incoming: bool,
        events: &UnboundedSender<Event>,
    ) {
        let peer = device_of(&conn);
        let path = path_kind(&conn);
        let _ = events.send(Event::Connected {
            peer,
            path,
            incoming,
        });
        peers.lock().unwrap().insert(
            peer,
            Peer {
                conn: conn.clone(),
                holds: report.peer_holds.clone(),
                busy: false,
            },
        );
        let _ = events.send(Event::Synced {
            peer,
            path,
            report,
            took: started.elapsed(),
        });

        let (node, peers, events) = (self.clone(), peers.clone(), events.clone());
        tokio::spawn(async move {
            loop {
                match node.serve(&conn).await {
                    Ok((report, took)) => {
                        if let Some(p) = peers.lock().unwrap().get_mut(&peer) {
                            p.holds = p.holds.merged(&report.peer_holds);
                        }
                        let path = path_kind(&conn);
                        let _ = events.send(Event::Synced {
                            peer,
                            path,
                            report,
                            took,
                        });
                    }
                    Err(_) if conn.close_reason().is_some() => break,
                    Err(e) => {
                        let error = describe(&e);
                        let _ = events.send(Event::Failed {
                            peer: Some(peer),
                            error,
                        });
                    }
                }
            }
            let mut map = peers.lock().unwrap();
            if map
                .get(&peer)
                .is_some_and(|p| p.conn.stable_id() == conn.stable_id())
            {
                map.remove(&peer);
                let _ = events.send(Event::Disconnected { peer });
            }
        });
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The address to dial a device by its ID alone; Iroh looks up where it is.
pub fn endpoint_addr(device: DeviceId) -> Result<EndpointAddr> {
    Ok(EndpointAddr::new(PublicKey::from_bytes(&device.0)?))
}

/// Closes `conn` with the reason if the handshake refused the peer, so the peer
/// sees why rather than a dropped connection.
fn refuse_on_error(conn: &Connection, result: Result<()>) -> Result<()> {
    if let Err(e) = &result {
        conn.close(REFUSED.into(), format!("refused: {e}").as_bytes());
    }
    result
}

fn device_of(conn: &Connection) -> DeviceId {
    DeviceId(*conn.remote_id().as_bytes())
}
