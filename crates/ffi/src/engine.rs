use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use mirrorleaf_core::Replica;
use mirrorleaf_net::device::{self, DeviceDir, device_of};
use mirrorleaf_net::ledger::{DeviceProfile, Fallback, LedgerClient, ONLINE_FOR_SECS};
use mirrorleaf_net::{
    Account, DevTicket, Event, Network, PhotoStore, SecretKey, State, SyncNode, describe,
    endpoint_addr,
};
use mirrorleaf_proto::DeviceId;
use mirrorleaf_proto::ledger::{Platform, StatusResponse};
use tokio::runtime::Runtime;
use tracing::{info, warn};

/// How long one dial may take before the device counts as unreachable.
const DIAL_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a push-woken job counts as online, so the devices it dials can
/// answer it; it reports going offline again when it finishes.
const JOB_ONLINE_SECS: u32 = 60;
/// When the app goes to the background, live sync keeps running this long so
/// the last edits reach connected devices (flush-on-close, plan §5.7).
const FLUSH: Duration = Duration::from_millis(1500);

/// The only engine in this process (plan §5.7).
static ENGINE: Mutex<Option<Arc<Engine>>> = Mutex::new(None);

/// An error as the apps show it: one readable line.
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum EngineError {
    #[error("{0}")]
    Failed(String),
}

impl From<anyhow::Error> for EngineError {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed(describe(&error))
    }
}

impl From<mirrorleaf_core::CoreError> for EngineError {
    fn from(error: mirrorleaf_core::CoreError) -> Self {
        Self::Failed(error.to_string())
    }
}

/// Told during a sync which devices this one is sending changes to, so a
/// high-priority job can say so in its notification (plan §5.9).
#[uniffi::export(callback_interface)]
pub trait SyncListener: Send + Sync {
    fn sending(&self, device_name: String, changes: u64);
}

/// One device of the account, for the status screen.
#[derive(Clone, Debug, serde::Serialize, uniffi::Record)]
pub struct DeviceSummary {
    pub id: String,
    pub name: String,
    pub this_device: bool,
    /// How many changes this device holds that were made on it.
    pub changes_held: u64,
    /// From the ledger's last answer, if there was one.
    pub online: Option<bool>,
    pub behind: Option<u64>,
}

#[derive(Clone, Debug, serde::Serialize, uniffi::Record)]
pub struct EngineStatus {
    pub device_id: String,
    pub device_name: String,
    pub account: Option<String>,
    pub devices: Vec<DeviceSummary>,
    pub notes: u32,
    pub changes: u64,
    pub ledger_url: Option<String>,
    pub has_push_token: bool,
    pub notifications: bool,
    /// Whether live sync is running (the app is in the foreground).
    pub live: bool,
}

/// What one push-woken sync did.
#[derive(Clone, Debug, Default, serde::Serialize, uniffi::Record)]
pub struct SyncSummary {
    /// The app was open, so live sync was already running and the job did nothing.
    pub live: bool,
    /// The devices reached, each with how ("direct" or "relay").
    pub reached: Vec<String>,
    pub failed: Vec<String>,
    pub sent: u64,
    pub received: u64,
    pub took_ms: u64,
}

/// A note for the Phase 0 screen (throwaway, like the screen itself).
#[derive(Clone, Debug, serde::Serialize)]
pub struct NoteSummary {
    pub id: String,
    pub title: String,
    pub paragraphs: Vec<String>,
}

/// Whoever is using the network node. The photo store can be opened once per
/// process, so live sync and push-woken jobs share one node.
#[derive(Default)]
struct Slot {
    node: Option<SyncNode>,
    live: Option<tokio::task::JoinHandle<()>>,
    jobs: u32,
}

/// The sync engine for one device.
#[derive(uniffi::Object)]
pub struct Engine {
    dir: DeviceDir,
    secret: SecretKey,
    /// The screen's connection to the database. Sync uses its own, as the CLI's
    /// `sync --watch` does; SQLite's WAL mode lets both work at once.
    replica: Mutex<Replica>,
    runtime: Runtime,
    slot: tokio::sync::Mutex<Slot>,
    /// The photo store, opened the first time a node is bound and kept open for
    /// the life of the process (ADR-005): reopening it for every job leaked a
    /// thread each time.
    photos: tokio::sync::OnceCell<PhotoStore>,
    /// The bound node, for the few places that can't wait for `slot`.
    node: Mutex<Option<SyncNode>>,
    live: AtomicBool,
    /// The ledger's last answer: who is online and how far behind.
    last_status: Mutex<Option<StatusResponse>>,
    /// Armed when the app is opened while behind (plan §5.9).
    fallback: Mutex<Option<Fallback>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[uniffi::export]
impl Engine {
    /// The process's engine, opened in `data_dir`. On a device's first start it
    /// creates the device's key and database, named `device_name`.
    #[uniffi::constructor]
    pub fn open(data_dir: String, device_name: String) -> Result<Arc<Self>, EngineError> {
        let mut engine = lock(&ENGINE);
        if let Some(open) = engine.as_ref() {
            if open.dir.path() == Path::new(&data_dir) {
                return Ok(open.clone());
            }
            return Err(EngineError::Failed(format!(
                "the engine is already open in {}",
                open.dir.path().display()
            )));
        }
        let opened = Arc::new(Self::start(&data_dir, &device_name)?);
        *engine = Some(opened.clone());
        Ok(opened)
    }

    pub fn status(&self) -> Result<EngineStatus, EngineError> {
        let replica = lock(&self.replica);
        let me = device_of(&self.secret);
        let account = Account::load(&replica)?;
        let held = replica.version_vector()?;
        let last = lock(&self.last_status).clone();
        let row = |id: DeviceId| {
            last.as_ref()
                .and_then(|s| s.devices.iter().find(|d| d.device == id).cloned())
        };
        let devices = account
            .as_ref()
            .map(|account| {
                account
                    .devices
                    .list
                    .devices
                    .iter()
                    .map(|d| DeviceSummary {
                        id: d.id.to_string(),
                        name: d.name.clone(),
                        this_device: d.id == me,
                        changes_held: held.get(&d.id),
                        online: row(d.id).map(|r| r.online),
                        behind: row(d.id).map(|r| r.behind),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let notes = replica.list_notes()?.iter().filter(|n| !n.trashed).count();
        let profile = profile(&replica)?;
        Ok(EngineStatus {
            device_id: me.to_string(),
            device_name: device::device_name(&replica)?,
            account: account.map(|a| a.id().to_string()),
            devices,
            notes: notes as u32,
            changes: replica.change_count()?,
            ledger_url: device::ledger_settings(&replica)?.map(|(url, _)| url),
            has_push_token: profile.push_token.is_some(),
            notifications: profile.notifications,
            live: self.live.load(Ordering::Acquire),
        })
    }

    /// The FCM token, or none when no push can reach this device (no Google
    /// Play services, or registration failed): it then syncs when opened.
    pub fn set_push_token(&self, token: Option<String>) -> Result<(), EngineError> {
        let token = token.filter(|t| !t.is_empty());
        lock(&self.replica).set_meta(
            device::LEDGER_PUSH_TOKEN,
            token.as_deref().unwrap_or("").as_bytes(),
        )?;
        self.refresh_ledger();
        Ok(())
    }

    /// Whether the app may show notifications. Without it the ledger sends only
    /// normal-priority pushes, because FCM downgrades high-priority ones that
    /// show nothing (plan §5.9).
    pub fn set_notifications(&self, allowed: bool) -> Result<(), EngineError> {
        lock(&self.replica).set_meta(device::LEDGER_NOTIFICATIONS, &[u8::from(allowed)])?;
        self.refresh_ledger();
        Ok(())
    }

    /// The push-woken job (plan §5.9): reports to the ledger, dials the devices
    /// its answer points to, fetches missing thumbnails, and reports going
    /// offline. Does nothing if the app is open, as live sync is running.
    pub async fn sync_now(
        self: Arc<Self>,
        listener: Box<dyn SyncListener>,
    ) -> Result<SyncSummary, EngineError> {
        let this = self.clone();
        self.on_runtime(async move { this.sync_job(listener).await })
            .await
    }

    /// The app came to the foreground: stay connected to every device that can
    /// be reached and keep syncing, and if this device is behind, ask the
    /// ledger to wake the device holding its changes (plan §5.9).
    pub async fn go_live(self: Arc<Self>) -> Result<(), EngineError> {
        let this = self.clone();
        self.on_runtime(async move { this.start_live().await })
            .await
    }

    /// The app went to the background: let the last edits go out, tell the
    /// ledger this device is going offline, and stop syncing.
    pub async fn go_offline(self: Arc<Self>) -> Result<(), EngineError> {
        let this = self.clone();
        self.on_runtime(async move { this.stop_live().await }).await
    }

    /// The name of the device to show in "Open Mirrorleaf on <name> to sync",
    /// once that prompt is due: the device holding this one's missing changes
    /// couldn't be woken, or hasn't sent them in time (plan §5.9).
    pub fn fallback_prompt(&self) -> Option<String> {
        let fallback = lock(&self.fallback).clone()?;
        let replica = lock(&self.replica);
        let mine = replica.version_vector().ok()?;
        if fallback.arrived(&mine) {
            *lock(&self.fallback) = None;
            return None;
        }
        if !fallback.due(&mine, Instant::now()) {
            return None;
        }
        let account = Account::load(&replica).ok().flatten()?;
        Some(
            device::device_names(&account)
                .remove(&fallback.holder)
                .unwrap_or_else(|| fallback.holder.short()),
        )
    }
}

/// For the Tauri screen (Phase 0's throwaway UI), not exported to Kotlin.
impl Engine {
    /// The engine, if this process has opened it.
    pub fn get() -> Option<Arc<Self>> {
        lock(&ENGINE).clone()
    }

    /// Joins a developer account from a ticket (plan §5.4), then syncs with the
    /// inviting device.
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

    pub fn notes(&self) -> Result<Vec<NoteSummary>, EngineError> {
        let replica = lock(&self.replica);
        let mut notes = Vec::new();
        for note in replica.list_notes()? {
            if note.trashed {
                continue;
            }
            notes.push(NoteSummary {
                paragraphs: replica.note_paragraphs(&note.id)?,
                id: note.id,
                title: note.title,
            });
        }
        Ok(notes)
    }

    pub fn create_note(&self, title: &str, text: &str) -> Result<String, EngineError> {
        let mut replica = lock(&self.replica);
        let id = replica.create_note(title)?;
        if !text.is_empty() {
            replica.append_paragraph(&id, text)?;
        }
        Ok(id)
    }

    pub fn append(&self, id: &str, text: &str) -> Result<(), EngineError> {
        lock(&self.replica).append_paragraph(id, text)?;
        Ok(())
    }
}

impl Engine {
    fn start(data_dir: &str, device_name: &str) -> anyhow::Result<Self> {
        let dir = DeviceDir::new(data_dir);
        let (secret, replica) = dir.open_or_init(device_name)?;
        // Two workers are plenty for one device's sync, and keep the push-woken
        // job small (plan §7.3: Android background job ≤ 50 MB).
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("mirrorleaf-engine")
            .enable_all()
            .build()?;
        info!("engine open for {}", device_of(&secret).short());
        Ok(Self {
            dir,
            secret,
            replica: Mutex::new(replica),
            runtime,
            slot: tokio::sync::Mutex::default(),
            photos: tokio::sync::OnceCell::new(),
            node: Mutex::default(),
            live: AtomicBool::new(false),
            last_status: Mutex::default(),
            fallback: Mutex::default(),
        })
    }

    /// Runs `work` on the engine's own runtime, whoever calls: Kotlin's
    /// coroutines or Tauri's runtime.
    async fn on_runtime<T, F>(&self, work: F) -> Result<T, EngineError>
    where
        T: Send + 'static,
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        match self.runtime.spawn(work).await {
            Ok(result) => result.map_err(EngineError::from),
            Err(e) => Err(EngineError::Failed(format!("engine task failed: {e}"))),
        }
    }

    /// The node, binding it if no one is using it yet.
    async fn bind(&self, slot: &mut Slot) -> anyhow::Result<SyncNode> {
        if let Some(node) = &slot.node {
            return Ok(node.clone());
        }
        let (secret, replica) = self.dir.open()?;
        let account =
            Account::load(&replica)?.context("this device hasn't joined an account yet")?;
        let ledger = device::ledger_client(&replica, &secret, &account)?;
        let storage = self.dir.blob_storage();
        let photos = self
            .photos
            .get_or_try_init(|| async move { PhotoStore::open(&storage).await })
            .await?;
        let node = SyncNode::bind_with_store(
            State { replica, account },
            secret,
            Network::Internet,
            photos,
        )
        .await?;
        if let Some(ledger) = ledger {
            node.use_ledger(ledger);
        }
        slot.node = Some(node.clone());
        *lock(&self.node) = Some(node.clone());
        Ok(node)
    }

    /// Closes the node once neither live sync nor a job is using it.
    async fn release_if_idle(&self, slot: &mut Slot) {
        if slot.live.is_some() || slot.jobs > 0 {
            return;
        }
        if let Some(node) = slot.node.take() {
            *lock(&self.node) = None;
            node.close().await;
        }
    }

    async fn begin_job(&self) -> anyhow::Result<SyncNode> {
        let mut slot = self.slot.lock().await;
        let node = self.bind(&mut slot).await?;
        slot.jobs += 1;
        Ok(node)
    }

    async fn end_job(&self) {
        let mut slot = self.slot.lock().await;
        slot.jobs = slot.jobs.saturating_sub(1);
        self.release_if_idle(&mut slot).await;
    }

    /// Gives the bound node a ledger client with the current settings (a new
    /// push token, notifications allowed or not).
    fn refresh_ledger(&self) {
        let Some(node) = lock(&self.node).clone() else {
            return;
        };
        let replica = lock(&self.replica);
        let client = Account::load(&replica)
            .ok()
            .flatten()
            .and_then(|account| device::ledger_client(&replica, &self.secret, &account).ok())
            .flatten();
        if let Some(client) = client {
            node.use_ledger(client);
        }
    }

    async fn sync_job(&self, listener: Box<dyn SyncListener>) -> anyhow::Result<SyncSummary> {
        let started = Instant::now();
        let node = {
            let mut slot = self.slot.lock().await;
            if slot.live.is_some() {
                return Ok(SyncSummary {
                    live: true,
                    ..SyncSummary::default()
                });
            }
            let node = self.bind(&mut slot).await?;
            slot.jobs += 1;
            node
        };
        let result = self.background_round(&node, listener.as_ref()).await;
        {
            let mut slot = self.slot.lock().await;
            slot.jobs = slot.jobs.saturating_sub(1);
            if slot.jobs == 0 && slot.live.is_none() {
                report_offline(&node).await;
            }
            self.release_if_idle(&mut slot).await;
        }
        let mut summary = result?;
        summary.took_ms = started.elapsed().as_millis() as u64;
        info!(
            "background sync: reached {:?}, failed {:?}, sent {}, received {} in {} ms",
            summary.reached, summary.failed, summary.sent, summary.received, summary.took_ms
        );
        Ok(summary)
    }

    async fn background_round(
        &self,
        node: &SyncNode,
        listener: &dyn SyncListener,
    ) -> anyhow::Result<SyncSummary> {
        node.store_staged().await?;
        let me = node.device();
        let (ours, names, others) = {
            let state = node.state().lock().unwrap();
            let others: Vec<DeviceId> = state
                .account
                .devices
                .list
                .devices
                .iter()
                .map(|d| d.id)
                .filter(|d| *d != me)
                .collect();
            (
                state.replica.version_vector()?,
                device::device_names(&state.account),
                others,
            )
        };
        let name = |d: &DeviceId| names.get(d).cloned().unwrap_or_else(|| d.short());

        // The ledger's answer says which devices are online and what each
        // holds, so the job dials only those it can help or learn from.
        let status = match node.ledger() {
            Some(ledger) => match ledger.report(&ours, Some(JOB_ONLINE_SECS)).await {
                Ok(status) => Some(status),
                Err(e) => {
                    warn!("ledger: {}", describe(&e));
                    None
                }
            },
            None => None,
        };
        let targets: Vec<(DeviceId, u64)> = match &status {
            Some(status) => {
                *lock(&self.last_status) = Some(status.clone());
                status
                    .devices
                    .iter()
                    .filter(|d| d.device != me && d.online)
                    .filter_map(|d| {
                        let to_send = ours.ahead_of(&d.version_vector);
                        let to_get = d.version_vector.ahead_of(&ours);
                        (to_send + to_get > 0).then_some((d.device, to_send))
                    })
                    .collect()
            }
            // Without the ledger, try every device on the account.
            None => others.into_iter().map(|d| (d, 0)).collect(),
        };
        for (peer, changes) in &targets {
            if *changes > 0 {
                listener.sending(name(peer), *changes);
            }
        }

        let mut summary = SyncSummary::default();
        for (peer, _) in targets {
            let dialled = tokio::time::timeout(DIAL_TIMEOUT, node.sync_once(endpoint_addr(peer)?));
            match dialled.await {
                Ok(Ok((path, report))) => {
                    summary.reached.push(format!("{} ({path})", name(&peer)));
                    summary.sent += report.sent_changes as u64;
                    summary.received += report.received_changes as u64;
                    if let Err(e) = node.fetch_missing(peer).await {
                        warn!("photos from {}: {}", name(&peer), describe(&e));
                    }
                }
                Ok(Err(e)) => summary
                    .failed
                    .push(format!("{}: {}", name(&peer), describe(&e))),
                Err(_) => summary
                    .failed
                    .push(format!("{}: not reachable", name(&peer))),
            }
        }
        Ok(summary)
    }

    async fn start_live(self: Arc<Self>) -> anyhow::Result<()> {
        let node = {
            let mut slot = self.slot.lock().await;
            if slot.live.is_some() {
                return Ok(());
            }
            let node = self.bind(&mut slot).await?;
            let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
            let (watcher, this) = (node.clone(), self.clone());
            slot.live = Some(tokio::spawn(async move {
                let log = async {
                    while let Some(event) = received.recv().await {
                        this.on_event(event);
                    }
                };
                tokio::select! {
                    watched = watcher.watch(events) => {
                        if let Err(e) = watched {
                            warn!("live sync stopped: {}", describe(&e));
                        }
                    }
                    () = log => {}
                }
            }));
            self.live.store(true, Ordering::Release);
            node
        };
        info!("live sync started");
        self.check_behind(&node).await;
        Ok(())
    }

    async fn stop_live(&self) -> anyhow::Result<()> {
        let mut slot = self.slot.lock().await;
        let Some(task) = slot.live.take() else {
            return Ok(());
        };
        tokio::time::sleep(FLUSH).await;
        task.abort();
        self.live.store(false, Ordering::Release);
        if let Some(node) = &slot.node {
            report_offline(node).await;
        }
        self.release_if_idle(&mut slot).await;
        info!("live sync stopped");
        Ok(())
    }

    /// Opening the app while behind asks the ledger to wake the device holding
    /// the changes, and arms the fallback prompt (plan §5.9, rule 3).
    async fn check_behind(&self, node: &SyncNode) {
        let Some(ledger) = node.ledger() else {
            return;
        };
        let ours = match node.state().lock().unwrap().replica.version_vector() {
            Ok(vv) => vv,
            Err(_) => return,
        };
        let status = match ledger.report(&ours, Some(ONLINE_FOR_SECS)).await {
            Ok(status) => status,
            Err(e) => {
                warn!("ledger: {}", describe(&e));
                return;
            }
        };
        *lock(&self.last_status) = Some(status.clone());
        let me = node.device();
        let behind = status
            .devices
            .iter()
            .find(|d| d.device == me)
            .map_or(0, |d| d.behind);
        if behind == 0 {
            *lock(&self.fallback) = None;
            return;
        }
        match ledger.wake().await {
            Ok(wake) => {
                info!("behind by {behind}: the ledger answered {wake:?}");
                *lock(&self.fallback) = Fallback::after_wake(&wake, &status, Instant::now());
            }
            Err(e) => warn!("ledger wake: {}", describe(&e)),
        }
    }

    fn on_event(&self, event: Event) {
        match event {
            Event::Connected { peer, path, .. } => {
                info!("connected to {} via {path}", peer.short())
            }
            Event::Synced {
                peer, path, report, ..
            } if report.sent_changes + report.received_changes > 0 => info!(
                "{} via {path}: sent {}, received {}",
                peer.short(),
                report.sent_changes,
                report.received_changes
            ),
            Event::Failed { peer, error } => {
                info!("{}: {error}", peer.map_or("incoming".into(), |p| p.short()))
            }
            Event::LedgerReported { response } => *lock(&self.last_status) = Some(response),
            Event::LedgerFailed { error } => warn!("ledger: {error}"),
            _ => {}
        }
    }
}

/// How this device presents itself to the ledger, from its saved settings.
fn profile(replica: &Replica) -> anyhow::Result<DeviceProfile> {
    let token = replica
        .meta(device::LEDGER_PUSH_TOKEN)?
        .map(|v| String::from_utf8_lossy(&v).into_owned())
        .filter(|t| !t.is_empty());
    Ok(DeviceProfile {
        platform: Platform::this(),
        push_token: token,
        notifications: replica.meta(device::LEDGER_NOTIFICATIONS)?.as_deref() == Some(&[1]),
    })
}

/// Tells the ledger this device is going offline. Best effort.
async fn report_offline(node: &SyncNode) {
    if node.ledger().is_some()
        && let Err(e) = node.go_offline().await
    {
        warn!("ledger: couldn't report going offline: {}", describe(&e));
    }
}
