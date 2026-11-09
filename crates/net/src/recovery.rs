//! Recovering an account with its recovery kit (E2, assessment §8.4).
//!
//! The kit's secret recreates the account key. The ledger knows the account's
//! devices by ID (never by name), so the recovered device registers there,
//! learns their IDs and dials them; the first one it reaches merges its full
//! device list (with names) into this device's, and the notes sync back. Until
//! then the others appear without names, and the person can remove the ones
//! they lost.

use anyhow::{Context, Result};
use iroh::SecretKey;
use mirrorleaf_proto::{DeviceEntry, DeviceId, VersionVector};

use crate::Account;
use crate::kit::RecoveryCode;
use crate::ledger::{DeviceProfile, LedgerClient};

/// The recovered account and a client for its ledger. The ledger is the one
/// in the kit's QR code, else `ledger`. The caller saves both.
pub async fn recover(
    code: &RecoveryCode,
    me: DeviceEntry,
    device_secret: &SecretKey,
    ledger: Option<String>,
    profile: DeviceProfile,
) -> Result<(Account, LedgerClient)> {
    let url = code
        .ledger
        .clone()
        .or(ledger)
        .filter(|url| !url.trim().is_empty())
        .context("recovering needs your ledger's address, to find your other devices")?;
    let mut account = Account::recover(&code.secret, me.clone());
    let client = LedgerClient::new(&url, device_secret, &account, profile)?;
    client.join_existing().await?;
    let status = client.report(&VersionVector::default(), None).await?;
    let others: Vec<DeviceId> = status
        .devices
        .iter()
        .map(|d| d.device)
        .filter(|d| *d != me.id)
        .collect();
    if !others.is_empty() {
        let key = account
            .key
            .as_ref()
            .expect("a recovered account holds its key");
        account.devices = key.add_unnamed(&account.devices, &others);
    }
    Ok((account, client))
}
