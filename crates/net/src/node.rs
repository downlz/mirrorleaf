use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, ensure};
use iroh::endpoint::{Connection, Incoming, presets};
use iroh::{Endpoint, EndpointAddr, PublicKey, RelayMode, SecretKey};
use mirrorleaf_core::Replica;
use mirrorleaf_proto::{ALPN, DeviceId, VersionVector};
use tokio::sync::mpsc::UnboundedSender;

use crate::account::Account;
use crate::describe;
use crate::frame::Traffic;
use crate::session::{Role, RoundReport, handshake, round};

/// QUIC close code sent when a peer is refused, with the reason as text.
const REFUSED: u32 = 1;

/// How often `watch` checks for new local changes to push.
const TICK: Duration = Duration::from_secs(1);
/// How long `watch` waits before dialling an unreachable device again.
const REDIAL: Duration = Duration::from_secs(30);
const DIAL_TIMEOUT: Duration = Duration::from_secs(20);

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
}

/// A connected peer, as `watch` tracks it.
struct Peer {
    conn: Connection,
    /// What the peer is known to hold, so `watch` only pushes when it has more.
    holds: VersionVector,
    busy: bool,
}

type Peers = Arc<Mutex<HashMap<DeviceId, Peer>>>;

impl SyncNode {
    pub async fn bind(state: State, secret: SecretKey, network: Network) -> Result<Self> {
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
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await?;
        Ok(Self {
            inner: Arc::new(Inner {
                endpoint,
                state: Arc::new(Mutex::new(state)),
                hints: Mutex::default(),
            }),
        })
    }

    pub fn state(&self) -> &Arc<Mutex<State>> {
        &self.inner.state
    }

    pub fn device(&self) -> DeviceId {
        DeviceId(*self.inner.endpoint.id().as_bytes())
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
        let mut report = round(&self.inner.state, send, recv, Role::Opener).await?;
        report.traffic += traffic;
        Ok((conn, report))
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
        round(&self.inner.state, send, recv, Role::Opener).await
    }

    /// Accepts the next incoming connection and serves its first round. Returns
    /// `None` once the endpoint is closed.
    pub async fn accept(&self) -> Option<Result<(Connection, RoundReport)>> {
        let incoming = self.inner.endpoint.accept().await?;
        Some(self.accept_connection(incoming).await)
    }

    async fn accept_connection(&self, incoming: Incoming) -> Result<(Connection, RoundReport)> {
        let conn = incoming.await?;
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
        let mut report = round(&self.inner.state, send, recv, Role::Responder).await?;
        report.traffic += traffic;
        Ok((conn, report))
    }

    /// Serves the next round the peer starts on `conn`.
    pub async fn serve(&self, conn: &Connection) -> Result<RoundReport> {
        let (send, recv) = conn.accept_bi().await?;
        round(&self.inner.state, send, recv, Role::Responder).await
    }

    pub async fn close(&self) {
        self.inner.endpoint.close().await
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
                            Ok((conn, report)) => {
                                node.track(&peers, conn, report, started, true, &events)
                            }
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
        let result = loop {
            tick.tick().await;
            if self.inner.endpoint.is_closed() {
                break Ok(());
            }
            let (ours, devices) = {
                let state = self.inner.state.lock().unwrap();
                let devices: Vec<DeviceId> = state
                    .account
                    .devices
                    .list
                    .devices
                    .iter()
                    .map(|d| d.id)
                    .collect();
                match state.replica.version_vector() {
                    Ok(vv) => (vv, devices),
                    Err(e) => break Err(e.into()),
                }
            };
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
        };
        acceptor.abort();
        result
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
                let started = Instant::now();
                match node.serve(&conn).await {
                    Ok(report) => {
                        if let Some(p) = peers.lock().unwrap().get_mut(&peer) {
                            p.holds = p.holds.merged(&report.peer_holds);
                        }
                        let path = path_kind(&conn);
                        let took = started.elapsed();
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
