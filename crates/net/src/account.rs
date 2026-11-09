use anyhow::{Context, Result, bail};
use mirrorleaf_core::Replica;
use mirrorleaf_proto::ledger::Approval;
use mirrorleaf_proto::{
    AccountId, AccountKey, DeviceEntry, DeviceId, Handover, KIT_MADE_AT, SignedDeviceList,
};

use crate::kit::RecoverySecret;

const ACCOUNT_SECRET: &str = "account_secret";
/// A recovery kit not saved yet (E2): its secret and when it was made. Kept
/// until the person has saved the kit or written the words down, so closing
/// the app midway doesn't lose it; deleted then.
const PENDING_KIT: &str = "pending_kit";
/// Set after this device recovered the account: the kit was used, so the
/// screen asks for a new one (assessment §8.4).
const KIT_USED: &str = "kit_used";
/// Set when another device removed this one while replacing the account key:
/// the old account's list doesn't say so, the new one did (E3).
const REMOVED: &str = "removed_from_account";
/// The signed device list, version 2 (M9).
const DEVICE_LIST: &str = "device_list_v2";
/// Phase 0's device list, read once to convert it.
const LEGACY_DEVICE_LIST: &str = "device_list";
/// The account key's approval of this device for the ledger (E1), kept so the
/// device can register again after the ledger's database is lost.
const LEDGER_APPROVAL: &str = "ledger_approval";

/// This device's account. Every device holds the account key: pairing hands it
/// over (assessment §8.2). It is stored in plain text in the database until
/// app lock and encryption at rest arrive (E4).
pub struct Account {
    pub key: Option<AccountKey>,
    pub devices: SignedDeviceList,
    /// The account key's approval of this device, for the ledger.
    pub approval: Option<String>,
    /// Another device removed this one while replacing the key (E3).
    pub removed: bool,
}

impl Account {
    /// Starts an account with `device` as its first member.
    pub fn create(device: DeviceEntry) -> Self {
        let key = AccountKey::generate();
        let approval = Some(Approval::AddDevice.sign(&key, &device.id));
        let devices = key.first_list(device);
        Self {
            key: Some(key),
            devices,
            approval,
            removed: false,
        }
    }

    /// Starts an account whose key comes from a new recovery secret (E2): the
    /// secret is the kit the person must save before setup finishes.
    pub fn create_with_kit(device: DeviceEntry, now: u64) -> (Self, RecoverySecret) {
        let secret = RecoverySecret::generate();
        let key = secret.account_key();
        let approval = Some(Approval::AddDevice.sign(&key, &device.id));
        let devices = key.with_settings(&key.first_list(device), &[(KIT_MADE_AT, now.to_string())]);
        (
            Self {
                key: Some(key),
                devices,
                approval,
                removed: false,
            },
            secret,
        )
    }

    /// The account the recovery secret stands for (E2), as a recovered device
    /// starts it: the same key, and a list with only this device until it
    /// meets the account's other devices and merges theirs.
    pub fn recover(secret: &RecoverySecret, me: DeviceEntry) -> Self {
        let key = secret.account_key();
        let approval = Some(Approval::AddDevice.sign(&key, &me.id));
        let devices = key.recovery_list(me);
        Self {
            key: Some(key),
            devices,
            approval,
            removed: false,
        }
    }

    /// The account that replaces this one's key (E3), with its new recovery
    /// secret: every device on the list moves to it except `remove`.
    pub fn replaced(
        &self,
        me: &DeviceId,
        remove: &[DeviceId],
        now: u64,
    ) -> Result<(Self, RecoverySecret)> {
        let old = self
            .key
            .as_ref()
            .context("this device doesn't hold the account key")?;
        if remove.contains(me) {
            bail!("a device can't remove itself while replacing the key");
        }
        let secret = RecoverySecret::generate();
        let new = secret.account_key();
        let devices = old.replacement_list(&self.devices, &new, remove, now);
        let approval = Some(Approval::AddDevice.sign(&new, me));
        Ok((
            Self {
                key: Some(new),
                devices,
                approval,
                removed: false,
            },
            secret,
        ))
    }

    /// The new key for a device that missed its replacement (E3), if this
    /// device holds a replaced account's key.
    pub fn handover(&self) -> Option<Handover> {
        let key = self.key.as_ref()?;
        self.devices.list.rotated_from.as_ref()?;
        Some(Handover {
            account_secret: key.to_bytes(),
            devices: self.devices.clone(),
        })
    }

    /// Takes the account's new key from another device (E3). The new list
    /// must be signed by the new key, linked from this account by the old
    /// key's signature, and hold this device.
    pub fn adopt(&self, handover: Handover, me: &DeviceId) -> Result<Self> {
        let key = AccountKey::from_bytes(handover.account_secret);
        if key.id() != handover.devices.list.account {
            bail!("the new account key doesn't match its device list");
        }
        handover
            .devices
            .verify_replacement_of(&self.id())
            .context("the new device list doesn't come from this account's key")?;
        if !handover.devices.contains(me) {
            bail!("the account's new device list doesn't include this device");
        }
        let approval = Some(Approval::AddDevice.sign(&key, me));
        Ok(Self {
            key: Some(key),
            devices: handover.devices,
            approval,
            removed: false,
        })
    }

    /// Whether `me` was removed from the account by another device (E3):
    /// the list says so, or the list of the account that replaced this one's
    /// key said so (`mark_removed`).
    pub fn removed(&self, me: &DeviceId) -> bool {
        self.devices.was_removed(me) || self.removed
    }

    /// Records that the account's new key was withheld from this device,
    /// which another device removed when it replaced the key (E3).
    pub fn mark_removed(replica: &mut Replica) -> Result<()> {
        replica.set_meta(REMOVED, &[1])?;
        Ok(())
    }

    /// Keeps a kit that isn't saved yet, so the screen can show it again.
    pub fn save_pending_kit(
        replica: &mut Replica,
        secret: &RecoverySecret,
        made_at: u64,
    ) -> Result<()> {
        let mut value = secret.to_bytes().to_vec();
        value.extend(made_at.to_le_bytes());
        replica.set_meta(PENDING_KIT, &value)?;
        Ok(())
    }

    /// The kit waiting to be saved, and when it was made.
    pub fn pending_kit(replica: &Replica) -> Result<Option<(RecoverySecret, u64)>> {
        let Some(value) = replica.meta(PENDING_KIT)? else {
            return Ok(None);
        };
        if value.len() != 24 {
            return Ok(None);
        }
        let secret: [u8; 16] = value[..16].try_into().expect("16 bytes");
        let made_at = u64::from_le_bytes(value[16..].try_into().expect("8 bytes"));
        Ok(Some((RecoverySecret::from_bytes(secret), made_at)))
    }

    /// The kit is saved: forget its secret.
    pub fn clear_pending_kit(replica: &mut Replica) -> Result<()> {
        replica.delete_meta(PENDING_KIT)?;
        replica.delete_meta(KIT_USED)?;
        Ok(())
    }

    /// Records that this device used the kit to recover the account.
    pub fn mark_kit_used(replica: &mut Replica) -> Result<()> {
        replica.set_meta(KIT_USED, &[1])?;
        Ok(())
    }

    pub fn kit_used(replica: &Replica) -> Result<bool> {
        Ok(replica.meta(KIT_USED)?.as_deref() == Some(&[1]))
    }

    /// Joins the account behind `key` by adding `device` to its current list
    /// (Phase 0 developer tickets).
    pub fn join(key: AccountKey, current: &SignedDeviceList, device: DeviceEntry) -> Result<Self> {
        current
            .verify(&key.id())
            .context("the ticket's device list isn't signed by its account key")?;
        let approval = Some(Approval::AddDevice.sign(&key, &device.id));
        let devices = key.add_device(current, device);
        Ok(Self {
            key: Some(key),
            devices,
            approval,
            removed: false,
        })
    }

    /// The account a pairing handed over (E1): checks that the list is signed
    /// by the key, that it holds `me`, and that the approval is the key's.
    pub fn from_pairing(
        secret: [u8; 32],
        devices: SignedDeviceList,
        approval: String,
        me: &DeviceId,
    ) -> Result<Self> {
        let key = AccountKey::from_bytes(secret);
        devices
            .verify(&key.id())
            .context("the device list isn't signed by the account key")?;
        if !devices.contains(me) {
            if devices.was_removed(me) {
                bail!("this device was removed from the account; reset it and add it again");
            }
            bail!("the device list doesn't include this device");
        }
        Approval::AddDevice
            .verify(&key.id(), me, &approval)
            .context("the approval isn't the account key's")?;
        Ok(Self {
            key: Some(key),
            devices,
            approval: Some(approval),
            removed: false,
        })
    }

    pub fn id(&self) -> AccountId {
        self.devices.list.account
    }

    pub fn load(replica: &Replica) -> Result<Option<Self>> {
        let key = match replica.meta(ACCOUNT_SECRET)? {
            Some(bytes) => {
                Some(AccountKey::from_bytes(bytes.try_into().map_err(|_| {
                    anyhow::anyhow!("stored account key is corrupt")
                })?))
            }
            None => None,
        };
        let devices = match replica.meta(DEVICE_LIST)? {
            Some(list) => postcard::from_bytes(&list)?,
            None => match replica.meta(LEGACY_DEVICE_LIST)? {
                // Not converted yet (see `upgrade`): convert in memory.
                Some(legacy) => {
                    let key = key.as_ref().context(
                        "a Phase 0 device list can't be converted without the account key",
                    )?;
                    SignedDeviceList::upgrade_legacy(&legacy, key)?
                }
                None => return Ok(None),
            },
        };
        let approval = replica
            .meta(LEDGER_APPROVAL)?
            .map(|a| String::from_utf8_lossy(&a).into_owned());
        Ok(Some(Self {
            key,
            devices,
            approval,
            removed: replica.meta(REMOVED)?.as_deref() == Some(&[1]),
        }))
    }

    /// Converts a device list stored by Phase 0 to version 2, once. Every
    /// device converts the same list to the same signed list, so they still
    /// agree afterwards. Returns whether there was anything to convert.
    pub fn upgrade(replica: &mut Replica) -> Result<bool> {
        if replica.meta(DEVICE_LIST)?.is_some() || replica.meta(LEGACY_DEVICE_LIST)?.is_none() {
            return Ok(false);
        }
        let Some(account) = Self::load(replica)? else {
            return Ok(false);
        };
        let mut account = account;
        if account.approval.is_none()
            && let Some(key) = &account.key
        {
            account.approval = Some(Approval::AddDevice.sign(key, &replica.device()));
        }
        account.save(replica)?;
        Ok(true)
    }

    pub fn save(&self, replica: &mut Replica) -> Result<()> {
        if let Some(key) = &self.key {
            replica.set_meta(ACCOUNT_SECRET, &key.to_bytes())?;
        }
        if let Some(approval) = &self.approval {
            replica.set_meta(LEDGER_APPROVAL, approval.as_bytes())?;
        }
        replica.set_meta(DEVICE_LIST, &postcard::to_stdvec(&self.devices)?)?;
        Ok(())
    }
}
