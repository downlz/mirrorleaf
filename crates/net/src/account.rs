use anyhow::{Context, Result};
use mirrorleaf_core::Replica;
use mirrorleaf_proto::{AccountId, AccountKey, DeviceEntry, SignedDeviceList};

const ACCOUNT_SECRET: &str = "account_secret";
const DEVICE_LIST: &str = "device_list";

/// This device's account. In Phase 0 a developer account's key is held by every
/// device (plan §5.4), and is stored in plain text in the database until app
/// lock and encryption at rest arrive (E4).
pub struct Account {
    pub key: Option<AccountKey>,
    pub devices: SignedDeviceList,
}

impl Account {
    /// Starts a developer account with `device` as its first member.
    pub fn create(device: DeviceEntry) -> Self {
        let key = AccountKey::generate();
        let devices = key.first_list(device);
        Self {
            key: Some(key),
            devices,
        }
    }

    /// Joins the account behind `key`, adding `device` to its current list.
    pub fn join(key: AccountKey, current: &SignedDeviceList, device: DeviceEntry) -> Result<Self> {
        current
            .verify(&key.id())
            .context("the ticket's device list isn't signed by its account key")?;
        let devices = key.add_device(current, device);
        Ok(Self {
            key: Some(key),
            devices,
        })
    }

    pub fn id(&self) -> AccountId {
        self.devices.list.account
    }

    pub fn load(replica: &Replica) -> Result<Option<Self>> {
        let Some(list) = replica.meta(DEVICE_LIST)? else {
            return Ok(None);
        };
        let devices: SignedDeviceList = postcard::from_bytes(&list)?;
        let key = match replica.meta(ACCOUNT_SECRET)? {
            Some(bytes) => {
                Some(AccountKey::from_bytes(bytes.try_into().map_err(|_| {
                    anyhow::anyhow!("stored account key is corrupt")
                })?))
            }
            None => None,
        };
        Ok(Some(Self { key, devices }))
    }

    pub fn save(&self, replica: &mut Replica) -> Result<()> {
        if let Some(key) = &self.key {
            replica.set_meta(ACCOUNT_SECRET, &key.to_bytes())?;
        }
        replica.set_meta(DEVICE_LIST, &postcard::to_stdvec(&self.devices)?)?;
        Ok(())
    }
}
