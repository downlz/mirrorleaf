//! Adding a device (E1, assessment §8.2), as the screen drives it: the
//! inviting device shows a QR code and a word code and asks before letting a
//! device in; the new device joins with one of them. The screen hears every
//! step as `UiEvent::PairingChanged` and reads it with `pairing_status`.

use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, bail};
use mirrorleaf_net::device;
use mirrorleaf_net::ledger::LedgerClient;
use mirrorleaf_net::pair::{self, Answer, JoinRequest, PairingCode, PairingEvent, WordCode};
use mirrorleaf_net::{Account, Network, describe};
use mirrorleaf_proto::ledger::Platform;
use mirrorleaf_proto::pair::Refusal;
use qrcode::render::svg;
use tracing::{info, warn};

use super::screen::UiEvent;
use super::{Engine, EngineError, lock, profile};

/// How long an invitation waits for a relay connection, so its QR code holds
/// an address that works beyond the local network.
const ONLINE_WAIT: Duration = Duration::from_secs(10);

/// The ledger a new device asks about a word code when it has none of its
/// own: set at build time (for development, the ledger on the local
/// network). Production builds will name Mirrorleaf's own ledger (E10).
const DEFAULT_LEDGER: Option<&str> = option_env!("MIRRORLEAF_DEFAULT_LEDGER");

/// What the inviting device shows.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Invitation {
    /// The QR code, dark on white, as an SVG document.
    pub qr_svg: String,
    /// The text the QR code holds, for a device without a camera to paste.
    pub qr_text: String,
    /// "7-acoustic-planet-river", when the device has a ledger to give it a
    /// number.
    pub words: Option<String>,
    /// When the codes stop working, in Unix milliseconds.
    pub expires_ms: i64,
}

/// Where adding a device has got to, on either side.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PairingStatus {
    #[default]
    Idle,
    /// Inviting: the codes are showing.
    Inviting { invitation: Invitation },
    /// Inviting: a device proved it knows the code; ask whether to let it in.
    Request {
        name: String,
        platform: String,
        number: String,
    },
    /// Joining: dialling the other device.
    Connecting,
    /// Joining: show this number and wait for the other device to allow it.
    Confirm { number: String },
    /// Either side: the new device is on the account.
    Joined { name: String },
    /// Either side: it ended without joining, and why.
    Ended { reason: String },
}

/// The pairing in progress, if any.
#[derive(Default)]
pub(super) struct Pairing {
    status: PairingStatus,
    /// The person's answer, while a request waits for it.
    answer: Option<Answer>,
}

impl Engine {
    pub fn pairing_status(&self) -> PairingStatus {
        lock(&self.pairing).status.clone()
    }

    fn set_pairing(&self, status: PairingStatus) {
        lock(&self.pairing).status = status;
        self.notify(UiEvent::PairingChanged);
    }

    /// Names this device before it joins or starts an account. Afterwards the
    /// name is part of the account's signed device list.
    pub fn set_device_name(&self, name: String) -> Result<(), EngineError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(EngineError::Failed("give this device a name".into()));
        }
        let mut replica = lock(&self.replica);
        if Account::load(&replica)?.is_some() {
            return Err(EngineError::Failed(
                "this device already belongs to an account; its name is set".into(),
            ));
        }
        replica.set_meta(device::DEVICE_NAME, name.as_bytes())?;
        Ok(())
    }

    // --- The inviting device ---

    /// Shows an invitation for a new device: live sync starts if it isn't
    /// running, as it serves the pairing connection.
    pub async fn invite(self: Arc<Self>) -> Result<Invitation, EngineError> {
        let this = self.clone();
        self.on_runtime(async move {
            this.clone().start_live().await?;
            let node = lock(&this.node)
                .clone()
                .context("live sync isn't running")?;
            let _ = tokio::time::timeout(ONLINE_WAIT, node.online()).await;
            let words = match node.ledger() {
                Some(ledger) => match ledger.offer_pairing().await {
                    Ok(offer) => Some(WordCode::generate(offer.nameplate)),
                    Err(e) => {
                        warn!("no word code: {}", describe(&e));
                        None
                    }
                },
                None => None,
            };
            let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
            let invited = node.invite(words, events);
            let qr_text = invited.qr.to_string();
            let invitation = Invitation {
                qr_svg: qr_svg(&qr_text)?,
                qr_text,
                words: invited.words.map(|w| w.to_string()),
                expires_ms: invited
                    .expires
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
            };
            this.set_pairing(PairingStatus::Inviting {
                invitation: invitation.clone(),
            });
            let listener = this.clone();
            tokio::spawn(async move {
                while let Some(event) = received.recv().await {
                    listener.on_pairing(event);
                }
            });
            Ok(invitation)
        })
        .await
    }

    fn on_pairing(&self, event: PairingEvent) {
        let status = match event {
            PairingEvent::Request {
                name,
                platform,
                number,
                answer,
            } => {
                lock(&self.pairing).answer = Some(answer);
                PairingStatus::Request {
                    name,
                    platform: platform.to_string(),
                    number,
                }
            }
            PairingEvent::Joined { name, .. } => {
                info!("{name} joined the account");
                self.notify(UiEvent::StatusChanged);
                PairingStatus::Joined { name }
            }
            PairingEvent::Ended(reason) => PairingStatus::Ended {
                reason: inviter_reason(reason).to_owned(),
            },
        };
        self.set_pairing(status);
    }

    /// The person's answer to "Allow … to join?".
    pub fn answer_pairing(&self, allow: bool) -> Result<(), EngineError> {
        let answer = lock(&self.pairing)
            .answer
            .take()
            .ok_or_else(|| EngineError::Failed("no device is waiting to join".into()))?;
        if allow {
            answer.allow();
        } else {
            answer.deny();
        }
        Ok(())
    }

    /// Stops showing the invitation, or refuses a device still waiting.
    pub fn cancel_pairing(&self) {
        if let Some(answer) = lock(&self.pairing).answer.take() {
            answer.deny();
        }
        if let Some(node) = lock(&self.node).clone() {
            node.cancel_invitation();
        }
        let mut pairing = lock(&self.pairing);
        if matches!(pairing.status, PairingStatus::Inviting { .. }) {
            pairing.status = PairingStatus::Idle;
        }
        drop(pairing);
        self.notify(UiEvent::PairingChanged);
    }

    // --- The new device ---

    /// Joins an account with a code from one of its devices: the QR code's
    /// text or the word code. `ledger` is where to look a word code up when
    /// this device has no ledger yet. Live sync starts once it has joined.
    pub async fn join_with_code(
        self: Arc<Self>,
        code: String,
        ledger: Option<String>,
    ) -> Result<String, EngineError> {
        let this = self.clone();
        self.on_runtime(async move {
            let joined = this.clone().pair_in(&code, ledger).await;
            match &joined {
                Ok(name) => this.set_pairing(PairingStatus::Joined { name: name.clone() }),
                Err(e) => this.set_pairing(PairingStatus::Ended {
                    reason: sentence(&describe(e)),
                }),
            }
            let name = joined?;
            this.notify(UiEvent::NotesChanged);
            this.notify(UiEvent::StatusChanged);
            Ok(format!("Joined. Syncing with {name}."))
        })
        .await
    }

    async fn pair_in(
        self: Arc<Self>,
        code: &str,
        ledger: Option<String>,
    ) -> anyhow::Result<String> {
        let (request, lookup) = {
            let replica = lock(&self.replica);
            if Account::load(&replica)?.is_some() {
                bail!("this device already belongs to an account");
            }
            let own = device::ledger_settings(&replica)?.map(|(url, _)| url);
            let lookup = ledger
                .filter(|l| !l.trim().is_empty())
                .or(own)
                .or(DEFAULT_LEDGER.map(str::to_owned));
            let request = JoinRequest {
                name: device::device_name(&replica)?,
                platform: Platform::this(),
                app_version: mirrorleaf_net::app_version().to_owned(),
            };
            (request, lookup)
        };
        self.set_pairing(PairingStatus::Connecting);
        let code: PairingCode = code.parse()?;
        let target = code.resolve(&self.secret, lookup.as_deref()).await?;
        let shows = self.clone();
        let welcome = pair::join(
            self.secret.clone(),
            Network::Internet,
            &target,
            request,
            move |number| shows.set_pairing(PairingStatus::Confirm { number }),
        )
        .await?;

        let me = device::device_of(&self.secret);
        let account = Account::from_pairing(
            welcome.account_secret,
            welcome.devices,
            welcome.approval,
            &me,
        )?;
        let inviter = device::device_names(&account)
            .remove(&mirrorleaf_proto::DeviceId(*target.inviter.id.as_bytes()))
            .unwrap_or_else(|| "your other device".to_owned());
        // The account's ledger, unless this device already has one.
        let ledger = welcome.ledger.or(target.ledger).or(lookup);
        let register = {
            let mut replica = lock(&self.replica);
            account.save(&mut replica)?;
            match ledger {
                Some(url) if device::ledger_settings(&replica)?.is_none() => {
                    let profile = profile(&replica)?;
                    device::save_ledger(&mut replica, &url, &profile)?;
                    Some(LedgerClient::new(&url, &self.secret, &account, profile)?)
                }
                _ => None,
            }
        };
        if let Some(client) = register
            && let Err(e) = client.register().await
        {
            warn!("couldn't register with the ledger yet: {}", describe(&e));
        }
        info!("joined account {}", account.id().short());
        // Live sync dials the inviter, which has this device on its list now.
        if let Err(e) = self.clone().start_live().await {
            warn!("live sync didn't start: {}", describe(&e));
        }
        Ok(inviter)
    }
}

/// Why an invitation ended, as the inviting device says it. (`Refusal`'s own
/// text is written for the device that tried to join.)
fn inviter_reason(reason: Refusal) -> &'static str {
    match reason {
        Refusal::WrongCode => {
            "A device tried a wrong code, so this code stopped working. Make a new one."
        }
        Refusal::CodeExpired => "The code ran out before a device joined.",
        Refusal::Denied => "You didn't allow the device to join.",
        Refusal::NoAnswer => "Nobody answered in time, so the code stopped working.",
        Refusal::AccountFull => {
            "Your account already has the most devices it can hold. Remove one first."
        }
    }
}

/// An error as a sentence for the screen: a capital first letter and a full stop.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    let mut out: String = chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    if !out.is_empty() && !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// The QR code for `text`, dark on white with a quiet zone, as SVG.
fn qr_svg(text: &str) -> anyhow::Result<String> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M)?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(240, 240)
        .dark_color(svg::Color("#1c211e"))
        .light_color(svg::Color("#ffffff"))
        .quiet_zone(true)
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_qr_code_is_an_svg() {
        let svg = qr_svg("MLP1ABCDEFGHIJKLMNOPQRSTUVWXYZ234567").unwrap();
        assert!(
            svg.starts_with("<?xml") || svg.starts_with("<svg"),
            "{}",
            &svg[..40]
        );
        assert!(svg.contains("#1c211e"));
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            sentence("the code didn't match; make a new one"),
            "The code didn't match; make a new one."
        );
        assert_eq!(sentence("Done."), "Done.");
        assert_eq!(sentence(""), "");
    }

    #[test]
    fn statuses_reach_the_screen_tagged() {
        let json = serde_json::to_value(PairingStatus::Confirm {
            number: "4821".into(),
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "state": "confirm", "number": "4821" })
        );
    }
}
