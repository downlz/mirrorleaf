use anyhow::{Context, Result, bail};
use mirrorleaf_core::Replica;
use mirrorleaf_proto::ledger::Approval;
use mirrorleaf_proto::{AccountId, AccountKey, DeviceEntry, DeviceId, SignedDeviceList};

const ACCOUNT_SECRET: &str = "account_secret";
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
        }
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
