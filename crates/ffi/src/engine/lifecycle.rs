//! The account's lifecycle as the screen drives it (M10): the recovery kit
//! (E2), recovering with it, removing a device and replacing the account key
//! (E3), and signing out.

use std::sync::Arc;

use anyhow::{Context, bail};
use mirrorleaf_net::device;
use mirrorleaf_net::kit::{Kit, RecoveryCode};
use mirrorleaf_net::{Account, SyncNode, describe, recovery};
use mirrorleaf_proto::DeviceId;
use tracing::{info, warn};

use super::pairing::DEFAULT_LEDGER;
use super::screen::UiEvent;
use super::{Engine, EngineError, lock, profile};

/// A recovery kit waiting to be saved, as the screen shows it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct KitView {
    pub words: Vec<String>,
    /// The account's short form ("48a02495").
    pub account: String,
    /// "9 November 2026".
    pub made: String,
}

impl Engine {
    /// The device's data folder.
    pub fn data_dir(&self) -> String {
        self.dir.path().to_string_lossy().into_owned()
    }

    /// The kit not saved yet, if any. Shown until the person has saved it or
    /// written the words down (assessment §8.1).
    pub fn pending_kit(&self) -> Result<Option<KitView>, EngineError> {
        Ok(self.kit()?.map(|kit| KitView {
            words: kit.secret.words().iter().map(|w| (*w).to_owned()).collect(),
            account: kit.fingerprint(),
            made: kit.date(),
        }))
    }

    fn kit(&self) -> anyhow::Result<Option<Kit>> {
        let replica = lock(&self.replica);
        let Some((secret, made_at)) = Account::pending_kit(&replica)? else {
            return Ok(None);
        };
        let ledger = device::ledger_settings(&replica)?.map(|(url, _)| url);
        Ok(Some(Kit::new(secret, made_at, ledger)))
    }

    /// The file name for the kit as `kind` ("pdf" or "text").
    pub fn kit_file_name(&self, kind: &str) -> Result<String, EngineError> {
        let kit = self.kit()?.context("there's no recovery kit to save")?;
        Ok(kit.file_name(if kind == "pdf" { "pdf" } else { "txt" }))
    }

    /// Writes the kit waiting to be saved to `path`, as "pdf" or "text".
    pub fn write_kit(&self, kind: &str, path: &str) -> Result<(), EngineError> {
        let kit = self.kit()?.context("there's no recovery kit to save")?;
        let bytes = match kind {
            "pdf" => kit.pdf()?,
            "text" => kit.text().into_bytes(),
            other => return Err(EngineError::Failed(format!("no kit as {other}"))),
        };
        std::fs::write(path, bytes).map_err(|e| EngineError::Failed(e.to_string()))?;
        info!("saved the recovery kit as {kind}");
        Ok(())
    }

    /// The kit is saved, or its words written down: forget its secret.
    pub fn finish_kit(&self) -> Result<(), EngineError> {
        Account::clear_pending_kit(&mut lock(&self.replica))?;
        self.notify(UiEvent::StatusChanged);
        Ok(())
    }

    /// The node, bound for one piece of work: live sync's if it is running.
    async fn borrow_node(&self) -> anyhow::Result<SyncNode> {
        let mut slot = self.slot.lock().await;
        let node = self.bind(&mut slot).await?;
        slot.jobs += 1;
        Ok(node)
    }

    async fn return_node(&self) {
        let mut slot = self.slot.lock().await;
        slot.jobs = slot.jobs.saturating_sub(1);
        self.release_if_idle(&mut slot).await;
    }

    /// Replaces the account key (E3), leaving out the devices in `remove`
    /// (IDs), and makes a new recovery kit to save: the old kit stops
    /// working. The other devices get the new key as they next connect to a
    /// device that has it.
    pub async fn replace_key(self: Arc<Self>, remove: Vec<String>) -> Result<(), EngineError> {
        let this = self.clone();
        self.on_runtime(async move {
            let remove = remove
                .iter()
                .map(|id| {
                    id.parse::<DeviceId>()
                        .map_err(|_| anyhow::anyhow!("bad device ID"))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let node = this.borrow_node().await?;
            let replaced = node.replace_key(&remove).await;
            if replaced.is_ok() && !this.live.load(std::sync::atomic::Ordering::Acquire) {
                // Not live: hand the key to whoever answers now.
                let told = node.announce().await;
                info!("handed the new key to {told} device(s)");
            }
            this.return_node().await;
            replaced?;
            this.notify(UiEvent::StatusChanged);
            Ok(())
        })
        .await
    }

    /// Removes another device from the account for good (E3).
    pub async fn remove_device(self: Arc<Self>, id: String) -> Result<(), EngineError> {
        let this = self.clone();
        self.on_runtime(async move {
            let device: DeviceId = id.parse().map_err(|_| anyhow::anyhow!("bad device ID"))?;
            let node = this.borrow_node().await?;
            let removed = node.remove_device(device).await;
            if removed.is_ok() && !this.live.load(std::sync::atomic::Ordering::Acquire) {
                node.announce().await;
            }
            this.return_node().await;
            removed?;
            info!("removed {}", device.short());
            this.notify(UiEvent::StatusChanged);
            Ok(())
        })
        .await
    }

    /// How many changes exist only on this device: what signing out would
    /// lose (E3).
    pub fn only_here(&self) -> Result<u64, EngineError> {
        let replica = lock(&self.replica);
        let me = device::device_of(&self.secret);
        let Some(account) = Account::load(&replica)? else {
            return Ok(0);
        };
        let others: Vec<DeviceId> = account
            .devices
            .list
            .devices
            .iter()
            .map(|d| d.id)
            .filter(|d| *d != me)
            .collect();
        Ok(replica.only_here(&others)?)
    }

    /// Signs this device out (E3): off the account's list, the others told,
    /// and everything erased when the app next starts, which the app does at
    /// once. Refuses while changes exist only here, unless `force`.
    pub async fn sign_out(self: Arc<Self>, force: bool) -> Result<(), EngineError> {
        let only_here = self.only_here()?;
        if only_here > 0 && !force {
            return Err(EngineError::Failed(format!(
                "{only_here} changes exist only on this device"
            )));
        }
        let this = self.clone();
        self.on_runtime(async move {
            let me = device::device_of(&this.secret);
            let removed = Account::load(&lock(&this.replica))?.is_some_and(|a| a.removed(&me));
            if !removed {
                let node = this.borrow_node().await?;
                let left = node.leave_account().await;
                if left.is_ok() {
                    let told = node.announce().await;
                    info!("signed out; told {told} device(s)");
                }
                this.return_node().await;
                left?;
            }
            this.stop_live().await?;
            this.dir.erase_on_next_start()?;
            Ok(())
        })
        .await
    }

    /// Recovers the account with its kit (E2): the 12 words or the QR code's
    /// text. `ledger` is where to find the account when the code doesn't say;
    /// this device's own, or the build's default, otherwise. Live sync starts
    /// and dials the account's other devices.
    pub async fn recover(
        self: Arc<Self>,
        code: String,
        ledger: Option<String>,
    ) -> Result<String, EngineError> {
        let this = self.clone();
        self.on_runtime(async move {
            let (me, lookup, profile) = {
                let replica = lock(&this.replica);
                if Account::load(&replica)?.is_some() {
                    bail!("this device already belongs to an account");
                }
                let own = device::ledger_settings(&replica)?.map(|(url, _)| url);
                let lookup = ledger
                    .filter(|l| !l.trim().is_empty())
                    .or(own)
                    .or(DEFAULT_LEDGER.map(str::to_owned));
                (
                    device::me(&replica, &this.secret)?,
                    lookup,
                    profile(&replica)?,
                )
            };
            let code: RecoveryCode = code.parse()?;
            let (account, client) =
                recovery::recover(&code, me, &this.secret, lookup, profile.clone()).await?;
            let others = account.devices.list.devices.len().saturating_sub(1);
            {
                let mut replica = lock(&this.replica);
                account.save(&mut replica)?;
                device::save_ledger(&mut replica, client.url(), &profile)?;
                Account::mark_kit_used(&mut replica)?;
            }
            info!(
                "recovered account {} ({others} other devices on the ledger)",
                account.id().short()
            );
            if let Err(e) = this.clone().start_live().await {
                warn!("live sync didn't start: {}", describe(&e));
            }
            this.notify(UiEvent::StatusChanged);
            this.notify(UiEvent::NotesChanged);
            Ok(match others {
                0 => "Recovered, but the ledger knows none of your other devices.".to_owned(),
                1 => "Recovered. Fetching your notes from your other device…".to_owned(),
                n => format!("Recovered. Fetching your notes from your {n} other devices…"),
            })
        })
        .await
    }
}
