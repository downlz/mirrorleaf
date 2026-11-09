//! Adding a device (E1, assessment §8.2): pairing codes, and both sides of
//! the pairing protocol (`mirrorleaf_proto::pair`).
//!
//! The device already on the account (the inviter) shows a QR code and, when
//! it has a ledger, a word code. The new device (the joiner) dials it over the
//! pairing ALPN. They run SPAKE2 with the code's secret as the password and
//! their device IDs as identities, prove to each other that they agree on the
//! key, and both show the same four-digit number. The inviter asks its person;
//! only if they allow it does the account key travel, over the connection
//! Iroh already encrypts between the two device keys. A code gets one attempt:
//! a wrong code, a refusal or the time running out ends it.

use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use data_encoding::BASE32_NOPAD;
use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey};
use mirrorleaf_proto::ledger::{Approval, MAX_DEVICES_PER_ACCOUNT, Platform};
use mirrorleaf_proto::pair::{
    self, CODE_LIFETIME_SECS, CodeKind, MAX_PAIR_MESSAGE, PAIR_ALPN, PairMessage, Refusal,
};
use mirrorleaf_proto::{DeviceEntry, DeviceId, SignedDeviceList};
use serde::{Deserialize, Serialize};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use tokio::sync::{mpsc, oneshot};

use crate::device;
use crate::ledger::LedgerClient;
use crate::node::{Network, Shared};

/// Longest wait for the other side's next message, except for the answer.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the inviter waits for its person to allow or refuse.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);
/// How long the joiner waits for that answer: a little longer.
const WELCOME_TIMEOUT: Duration = Duration::from_secs(150);
/// How long a side waits, after its last message, for the other to hang up.
const LINGER: Duration = Duration::from_secs(5);
const DIAL_TIMEOUT: Duration = Duration::from_secs(30);

/// The text a QR code holds starts with this. Upper case and base32 keep it in
/// the QR alphanumeric mode, which needs fewer modules.
const QR_PREFIX: &str = "MLP1";

// --- Codes ---

/// What a QR code holds: where the inviter is, a 128-bit secret, and the
/// ledger the account uses, so a new device can use it too.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QrCode {
    pub inviter: EndpointAddr,
    pub secret: [u8; 16],
    pub ledger: Option<String>,
}

impl fmt::Display for QrCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = postcard::to_stdvec(self).map_err(|_| fmt::Error)?;
        write!(f, "{QR_PREFIX}{}", BASE32_NOPAD.encode(&bytes))
    }
}

/// A code to type: the ledger's number for the inviter, then three words from
/// the BIP39 English list ("7-acoustic-planet-river"). The words are the PAKE
/// password: 33 bits, but a guess gets one try before the code ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordCode {
    pub nameplate: u32,
    pub words: [String; 3],
}

impl WordCode {
    pub fn generate(nameplate: u32) -> Self {
        let list = bip39::Language::English.word_list();
        let word = || list[usize::from(rand::random::<u16>() % 2048)].to_owned();
        Self {
            nameplate,
            words: [word(), word(), word()],
        }
    }

    fn password(&self) -> Vec<u8> {
        self.to_string().into_bytes()
    }
}

impl fmt::Display for WordCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c] = &self.words;
        write!(f, "{}-{a}-{b}-{c}", self.nameplate)
    }
}

impl FromStr for WordCode {
    type Err = anyhow::Error;

    /// Accepts spaces or hyphens, any case, and a word shortened to its first
    /// four letters (BIP39 words differ within four).
    fn from_str(s: &str) -> Result<Self> {
        let mut parts = s
            .split(|c: char| c == '-' || c.is_whitespace())
            .filter(|p| !p.is_empty());
        let nameplate = parts
            .next()
            .and_then(|n| n.parse::<u32>().ok())
            .context("a code starts with its number, for example 7-acoustic-planet-river")?;
        let list = bip39::Language::English.word_list();
        let mut words = Vec::new();
        for part in parts {
            let part = part.to_lowercase();
            let found = if list.contains(&part.as_str()) {
                Some(part.clone())
            } else if part.len() >= 4 {
                let mut matches = list.iter().filter(|w| w.starts_with(&part));
                match (matches.next(), matches.next()) {
                    (Some(w), None) => Some((*w).to_owned()),
                    _ => None,
                }
            } else {
                None
            };
            words.push(found.with_context(|| format!("“{part}” isn't one of the code's words"))?);
        }
        let words: [String; 3] = words
            .try_into()
            .map_err(|_| anyhow::anyhow!("a code has a number and three words"))?;
        Ok(Self { nameplate, words })
    }
}

/// A code as the new device receives it: scanned or typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingCode {
    Qr(QrCode),
    Words(WordCode),
}

impl FromStr for PairingCode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        if let Some(body) = s
            .get(..QR_PREFIX.len())
            .filter(|p| p.eq_ignore_ascii_case(QR_PREFIX))
            .map(|_| &s[QR_PREFIX.len()..])
        {
            let bytes = BASE32_NOPAD
                .decode(body.to_uppercase().as_bytes())
                .context("the QR code is damaged")?;
            return Ok(Self::Qr(
                postcard::from_bytes(&bytes).context("the QR code is damaged")?,
            ));
        }
        Ok(Self::Words(s.parse()?))
    }
}

impl PairingCode {
    /// Finds the inviting device. A QR code says where it is; a word code's
    /// number is looked up on `ledger` (or the ledger the code names).
    pub async fn resolve(&self, secret: &SecretKey, ledger: Option<&str>) -> Result<JoinTarget> {
        match self {
            Self::Qr(qr) => Ok(JoinTarget::qr(qr)),
            Self::Words(words) => {
                let url = ledger.context(
                    "a word code needs a ledger to find your other device: scan its QR code instead, or enter the ledger's address",
                )?;
                let device = crate::ledger::lookup_pairing(url, secret, words.nameplate).await?;
                Ok(JoinTarget::words(
                    crate::node::endpoint_addr(device)?,
                    words,
                    Some(url.to_owned()),
                ))
            }
        }
    }
}

/// Where the joiner dials, and with which code.
#[derive(Clone, Debug)]
pub struct JoinTarget {
    pub inviter: EndpointAddr,
    via: CodeKind,
    password: Vec<u8>,
    /// The ledger the code came with, if any.
    pub ledger: Option<String>,
}

impl JoinTarget {
    pub fn qr(qr: &QrCode) -> Self {
        Self {
            inviter: qr.inviter.clone(),
            via: CodeKind::Qr,
            password: qr.secret.to_vec(),
            ledger: qr.ledger.clone(),
        }
    }

    pub fn words(inviter: EndpointAddr, words: &WordCode, ledger: Option<String>) -> Self {
        Self {
            inviter,
            via: CodeKind::Words,
            password: words.password(),
            ledger,
        }
    }
}

// --- The inviter's side ---

/// What the inviting device shows.
#[derive(Clone, Debug)]
pub struct Invitation {
    pub qr: QrCode,
    /// Only with a ledger, which turns the number into the inviter's address.
    pub words: Option<WordCode>,
    pub expires: SystemTime,
}

/// What happens to an invitation, for the inviting device's screen.
#[derive(Debug)]
pub enum PairingEvent {
    /// A device proved it knows the code. Show `number` and ask whether to let
    /// it join.
    Request {
        name: String,
        platform: Platform,
        number: String,
        answer: Answer,
    },
    /// It joined: it is on the account's device list now.
    Joined { device: DeviceId, name: String },
    /// The code stopped working without a device joining.
    Ended(Refusal),
}

/// The person's answer to a [`PairingEvent::Request`]. Dropping it refuses.
#[derive(Debug)]
pub struct Answer(oneshot::Sender<bool>);

impl Answer {
    pub fn allow(self) {
        let _ = self.0.send(true);
    }

    pub fn deny(self) {
        let _ = self.0.send(false);
    }
}

/// The invitation the node is serving, if any.
pub(crate) struct Offer {
    qr_secret: [u8; 16],
    words: Option<WordCode>,
    expires: Instant,
    events: mpsc::UnboundedSender<PairingEvent>,
    used: bool,
}

pub(crate) type OfferSlot = Arc<Mutex<Option<Offer>>>;

impl Offer {
    /// The ledger's number for this invitation, if it has one.
    pub(crate) fn nameplate(&self) -> Option<u32> {
        self.words.as_ref().map(|w| w.nameplate)
    }
}

/// Starts serving a new invitation, replacing any earlier one.
pub(crate) fn invite(
    slot: &OfferSlot,
    inviter: EndpointAddr,
    words: Option<WordCode>,
    ledger: Option<String>,
    events: mpsc::UnboundedSender<PairingEvent>,
) -> Invitation {
    let secret: [u8; 16] = rand::random();
    let lifetime = Duration::from_secs(CODE_LIFETIME_SECS);
    *slot.lock().unwrap() = Some(Offer {
        qr_secret: secret,
        words: words.clone(),
        expires: Instant::now() + lifetime,
        events,
        used: false,
    });
    Invitation {
        qr: QrCode {
            inviter,
            secret,
            ledger,
        },
        words,
        expires: SystemTime::now() + lifetime,
    }
}

/// Serves one pairing connection (the inviter's side).
pub(crate) async fn serve(
    conn: Connection,
    inviter: DeviceId,
    slot: OfferSlot,
    state: Shared,
    ledger: Option<Arc<LedgerClient>>,
) -> Result<()> {
    let joiner = DeviceId(*conn.remote_id().as_bytes());
    let (mut send, mut recv) = conn.accept_bi().await?;
    let PairMessage::Start { via, pake } = read(&mut recv).await? else {
        bail!("expected the joiner's first message");
    };

    // This connection takes the code's one attempt, if it is still open.
    let taken = {
        let mut slot = slot.lock().unwrap();
        match slot.as_mut() {
            Some(offer) if !offer.used && Instant::now() < offer.expires => {
                let password = match via {
                    CodeKind::Qr => Some(offer.qr_secret.to_vec()),
                    CodeKind::Words => offer.words.as_ref().map(WordCode::password),
                };
                password.map(|password| {
                    offer.used = true;
                    (password, offer.events.clone(), offer.words.clone())
                })
            }
            _ => None,
        }
    };
    let Some((password, events, words)) = taken else {
        refuse(&conn, &mut send, Refusal::CodeExpired).await;
        return Ok(());
    };
    let outcome = run_inviter(
        &mut send, &mut recv, inviter, joiner, &password, &pake, &events, &state,
    )
    .await;
    // Whatever happened, the code is used up.
    finish(&slot, ledger, words.as_ref());
    match outcome {
        Ok(name) => {
            let _ = events.send(PairingEvent::Joined {
                device: joiner,
                name,
            });
            linger(&conn).await;
            Ok(())
        }
        Err(Ended::Refused(reason)) => {
            let _ = events.send(PairingEvent::Ended(reason));
            refuse(&conn, &mut send, reason).await;
            Ok(())
        }
        Err(Ended::Failed(e)) => {
            let _ = events.send(PairingEvent::Ended(Refusal::CodeExpired));
            Err(e)
        }
    }
}

enum Ended {
    Refused(Refusal),
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for Ended {
    fn from(e: anyhow::Error) -> Self {
        Self::Failed(e)
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_inviter(
    send: &mut SendStream,
    recv: &mut RecvStream,
    inviter: DeviceId,
    joiner: DeviceId,
    password: &[u8],
    joiner_pake: &[u8],
    events: &mpsc::UnboundedSender<PairingEvent>,
    state: &Shared,
) -> Result<String, Ended> {
    let (spake, reply) = Spake2::<Ed25519Group>::start_b(
        &Password::new(password),
        &Identity::new(&joiner.0),
        &Identity::new(&inviter.0),
    );
    let key = spake
        .finish(joiner_pake)
        .map_err(|_| anyhow::anyhow!("the joining device sent a malformed message"))?;
    write(send, &PairMessage::Reply { pake: reply }).await?;
    match step(recv, STEP_TIMEOUT).await? {
        PairMessage::Confirm { proof } if pair::check_confirmation(&key, true, &proof) => {}
        // A wrong code, or a guess: either way the code ends here.
        PairMessage::Confirm { .. } => return Err(Ended::Refused(Refusal::WrongCode)),
        other => return Err(anyhow::anyhow!("unexpected pairing message: {other:?}").into()),
    }
    write(
        send,
        &PairMessage::Confirm {
            proof: pair::confirmation(&key, false),
        },
    )
    .await?;
    let (name, platform) = match step(recv, STEP_TIMEOUT).await? {
        PairMessage::Request { name, platform, .. } => (name, platform),
        other => return Err(anyhow::anyhow!("unexpected pairing message: {other:?}").into()),
    };

    // Ask the person, showing the number both screens show.
    let (answer, answered) = oneshot::channel();
    let _ = events.send(PairingEvent::Request {
        name: name.clone(),
        platform,
        number: pair::confirmation_number(&key),
        answer: Answer(answer),
    });
    match tokio::time::timeout(ANSWER_TIMEOUT, answered).await {
        Ok(Ok(true)) => {}
        Ok(_) => return Err(Ended::Refused(Refusal::Denied)),
        Err(_) => return Err(Ended::Refused(Refusal::NoAnswer)),
    }

    // Add it to the device list, then hand over the account.
    let welcome = {
        let mut state = state.lock().unwrap();
        let account = &state.account;
        let key = account
            .key
            .as_ref()
            .context("this device doesn't hold the account key, so it can't add devices")?;
        if account.devices.list.devices.len() as i64 >= MAX_DEVICES_PER_ACCOUNT {
            return Err(Ended::Refused(Refusal::AccountFull));
        }
        let devices = key.add_device(&account.devices, entry(joiner, name.clone(), platform));
        let approval = Approval::AddDevice.sign(key, &joiner);
        let account_secret = key.to_bytes();
        state.account.devices = devices.clone();
        let state = &mut *state;
        state.account.save(&mut state.replica)?;
        let ledger = device::ledger_settings(&state.replica)?.map(|(url, _)| url);
        PairMessage::Welcome {
            account_secret,
            devices: Box::new(devices),
            approval,
            ledger,
        }
    };
    write(send, &welcome).await?;
    // The device is on the list now; if `Done` never comes, it can simply
    // pair again.
    match step(recv, STEP_TIMEOUT).await {
        Ok(PairMessage::Done) => {}
        other => tracing::warn!("the joining device didn't confirm: {other:?}"),
    }
    let _ = send.finish();
    Ok(name)
}

/// Ends the invitation and withdraws its number from the ledger.
fn finish(slot: &OfferSlot, ledger: Option<Arc<LedgerClient>>, words: Option<&WordCode>) {
    slot.lock().unwrap().take();
    if let (Some(ledger), Some(words)) = (ledger, words) {
        let nameplate = words.nameplate;
        tokio::spawn(async move {
            if let Err(e) = ledger.cancel_pairing(nameplate).await {
                tracing::warn!("couldn't withdraw pairing number {nameplate}: {e:#}");
            }
        });
    }
}

/// Tells the other side why, then waits briefly for it to hang up so the
/// message isn't lost with the connection.
async fn refuse(conn: &Connection, send: &mut SendStream, reason: Refusal) {
    let _ = write(send, &PairMessage::Refused { reason }).await;
    let _ = send.finish();
    linger(conn).await;
}

async fn linger(conn: &Connection) {
    let _ = tokio::time::timeout(LINGER, conn.closed()).await;
}

// --- The joiner's side ---

/// What the new device sends about itself.
#[derive(Clone, Debug)]
pub struct JoinRequest {
    pub name: String,
    pub platform: Platform,
    pub app_version: String,
}

/// What the new device receives once allowed in.
#[derive(Clone, Debug)]
pub struct Welcome {
    pub account_secret: [u8; 32],
    pub devices: SignedDeviceList,
    pub approval: String,
    pub ledger: Option<String>,
}

/// The pairing ended without joining, for a reason the person can act on.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct JoinRefused(pub Refusal);

/// The joiner's side: dials the inviter, proves it knows the code, calls
/// `on_number` with the four-digit number to show, and waits to be let in.
pub async fn join(
    secret: SecretKey,
    network: Network,
    target: &JoinTarget,
    request: JoinRequest,
    on_number: impl FnOnce(String) + Send,
) -> Result<Welcome> {
    let builder = match network {
        Network::Internet => Endpoint::builder(presets::N0),
        Network::Local => Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled),
    };
    let joiner = crate::device::device_of(&secret);
    let endpoint = builder.secret_key(secret).bind().await?;
    let result = async {
        let conn = tokio::time::timeout(
            DIAL_TIMEOUT,
            endpoint.connect(target.inviter.clone(), PAIR_ALPN),
        )
        .await
        .context("your other device didn't answer: is Mirrorleaf open on it?")??;
        let welcome = run_joiner(&conn, joiner, target, request, on_number).await;
        conn.close(0u32.into(), b"done");
        welcome
    }
    .await;
    endpoint.close().await;
    result
}

async fn run_joiner(
    conn: &Connection,
    joiner: DeviceId,
    target: &JoinTarget,
    request: JoinRequest,
    on_number: impl FnOnce(String) + Send,
) -> Result<Welcome> {
    let inviter = DeviceId(*conn.remote_id().as_bytes());
    let (mut send, mut recv) = conn.open_bi().await?;
    let (spake, start) = Spake2::<Ed25519Group>::start_a(
        &Password::new(&target.password),
        &Identity::new(&joiner.0),
        &Identity::new(&inviter.0),
    );
    write(
        &mut send,
        &PairMessage::Start {
            via: target.via,
            pake: start,
        },
    )
    .await?;
    let reply = match step(&mut recv, STEP_TIMEOUT).await? {
        PairMessage::Reply { pake } => pake,
        other => return Err(unexpected(other)),
    };
    let key = spake
        .finish(&reply)
        .map_err(|_| anyhow::anyhow!("your other device sent a malformed reply"))?;
    write(
        &mut send,
        &PairMessage::Confirm {
            proof: pair::confirmation(&key, true),
        },
    )
    .await?;
    match step(&mut recv, STEP_TIMEOUT).await? {
        PairMessage::Confirm { proof } if pair::check_confirmation(&key, false, &proof) => {}
        PairMessage::Confirm { .. } => {
            bail!("the device you reached couldn't prove it made this code; make a new one")
        }
        other => return Err(unexpected(other)),
    }
    on_number(pair::confirmation_number(&key));
    write(
        &mut send,
        &PairMessage::Request {
            name: request.name,
            platform: request.platform,
            app_version: request.app_version,
        },
    )
    .await?;
    let welcome = match step(&mut recv, WELCOME_TIMEOUT).await? {
        PairMessage::Welcome {
            account_secret,
            devices,
            approval,
            ledger,
        } => Welcome {
            account_secret,
            devices: *devices,
            approval,
            ledger,
        },
        other => return Err(unexpected(other)),
    };
    write(&mut send, &PairMessage::Done).await?;
    let _ = send.finish();
    // Wait for the inviter to read `Done` and hang up.
    let _ = tokio::time::timeout(LINGER, conn.closed()).await;
    Ok(welcome)
}

/// The next message, within `wait`.
async fn step(recv: &mut RecvStream, wait: Duration) -> Result<PairMessage> {
    tokio::time::timeout(wait, read(recv))
        .await
        .context("your other device stopped answering")?
}

/// A refusal becomes its reason; anything else out of order is a protocol error.
fn unexpected(message: PairMessage) -> anyhow::Error {
    match message {
        PairMessage::Refused { reason } => JoinRefused(reason).into(),
        other => anyhow::anyhow!("unexpected pairing message: {other:?}"),
    }
}

// --- Framing: a 4-byte big-endian length and a postcard body, as for sync ---

async fn write(send: &mut SendStream, message: &PairMessage) -> Result<()> {
    let body = pair::encode(message);
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    send.write_all(&frame).await?;
    Ok(())
}

async fn read(recv: &mut RecvStream) -> Result<PairMessage> {
    let mut len = [0; 4];
    recv.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_PAIR_MESSAGE {
        bail!("pairing message too large");
    }
    let mut body = vec![0; len];
    recv.read_exact(&mut body).await?;
    Ok(pair::decode(&body)?)
}

/// The joiner's entry for the device list.
fn entry(joiner: DeviceId, name: String, platform: Platform) -> DeviceEntry {
    DeviceEntry {
        id: joiner,
        name,
        platform: Some(platform),
        added_at: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_codes_round_trip_and_forgive_typing() {
        let code = WordCode::generate(7);
        assert_eq!(code.to_string().parse::<WordCode>().unwrap(), code);
        let typed: WordCode = "7 Acoustic  plan-RIVER".parse().unwrap();
        assert_eq!(typed.to_string(), "7-acoustic-planet-river");
        assert!("7-acoustic-planet".parse::<WordCode>().is_err());
        assert!("acoustic-planet-river-ozone".parse::<WordCode>().is_err());
        assert!("7-acoustic-planet-zzzz".parse::<WordCode>().is_err());
    }

    #[test]
    fn qr_codes_round_trip() {
        let secret = SecretKey::generate();
        let qr = QrCode {
            inviter: EndpointAddr::new(secret.public()),
            secret: [9; 16],
            ledger: Some("https://ledger.example".into()),
        };
        let text = qr.to_string();
        assert!(text.starts_with("MLP1"));
        assert!(
            text.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        );
        assert_eq!(
            text.parse::<PairingCode>().unwrap(),
            PairingCode::Qr(qr.clone())
        );
        assert_eq!(
            text.to_lowercase().parse::<PairingCode>().unwrap(),
            PairingCode::Qr(qr)
        );
    }
}
