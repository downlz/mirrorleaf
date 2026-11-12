//! Device lifecycle over loopback (M10: E2 recovery kit, E3 removal and key
//! replacement): real Iroh peers in one process, no ledger.

use mirrorleaf_core::Replica;
use mirrorleaf_net::kit::RecoverySecret;
use mirrorleaf_net::{Account, BlobStorage, Network, SecretKey, State, SyncNode};
use mirrorleaf_proto::{AccountKey, DeviceEntry, DeviceId};

async fn node(name: &str, account: impl FnOnce(DeviceEntry) -> Account) -> SyncNode {
    let secret = SecretKey::generate();
    let device = DeviceId(*secret.public().as_bytes());
    let replica = Replica::open_in_memory(device).unwrap();
    let account = account(DeviceEntry {
        id: device,
        name: name.into(),
        platform: None,
        added_at: 0,
    });
    SyncNode::bind(
        State { replica, account },
        secret,
        Network::Local,
        BlobStorage::Memory,
    )
    .await
    .unwrap()
}

/// A device joining `inviter`'s account from its current list.
async fn joiner(name: &str, inviter: &SyncNode) -> SyncNode {
    let (key, devices) = {
        let state = inviter.state().lock().unwrap();
        (
            state.account.key.as_ref().unwrap().to_bytes(),
            state.account.devices.clone(),
        )
    };
    node(name, |me| {
        Account::join(AccountKey::from_bytes(key), &devices, me).unwrap()
    })
    .await
}

/// A laptop, a phone and a tablet on one account made with a recovery kit,
/// all holding the full device list. Returns the kit's secret too.
async fn three() -> (SyncNode, SyncNode, SyncNode, RecoverySecret) {
    let mut kit = None;
    let a = node("laptop", |me| {
        let (account, secret) = Account::create_with_kit(me, 1_794_200_000);
        kit = Some(secret);
        account
    })
    .await;
    let b = joiner("phone", &a).await;
    try_sync(&a, &b).await.unwrap();
    let c = joiner("tablet", &a).await;
    try_sync(&a, &c).await.unwrap();
    try_sync(&a, &b).await.unwrap();
    (a, b, c, kit.unwrap())
}

/// `from` dials `to` and they run one round, or `to` refuses.
async fn try_sync(to: &SyncNode, from: &SyncNode) -> anyhow::Result<()> {
    let accepting = {
        let to = to.clone();
        tokio::spawn(async move { to.accept().await })
    };
    let result = from.sync_once(to.loopback_addr()).await.map(drop);
    let _ = accepting.await;
    result
}

fn account(node: &SyncNode) -> mirrorleaf_proto::AccountId {
    node.state().lock().unwrap().account.id()
}

fn note(node: &SyncNode, title: &str) -> String {
    node.state()
        .lock()
        .unwrap()
        .replica
        .create_note(title)
        .unwrap()
}

fn has_note(node: &SyncNode, id: &str) -> bool {
    node.state()
        .lock()
        .unwrap()
        .replica
        .list_notes()
        .unwrap()
        .iter()
        .any(|n| n.id == id)
}

#[tokio::test(flavor = "multi_thread")]
async fn replacing_the_key_moves_the_listed_devices_and_shuts_out_the_removed_one() {
    let (a, b, c, _) = three().await;
    let old = account(&a);

    // The tablet was lost: the laptop replaces the key without it.
    let secret = a.replace_key(&[c.device()]).await.unwrap();
    let new = account(&a);
    assert_ne!(new, old);
    assert_eq!(
        secret.account_key().id(),
        new,
        "the new kit opens the new account"
    );
    {
        let state = a.state().lock().unwrap();
        assert!(Account::pending_kit(&state.replica).unwrap().is_some());
        assert!(state.account.devices.was_removed(&c.device()));
    }

    // The phone gets the new key the next time it connects.
    try_sync(&a, &b).await.unwrap();
    assert_eq!(account(&b), new);
    assert_eq!(
        b.state().lock().unwrap().account.devices,
        a.state().lock().unwrap().account.devices
    );

    // The tablet is refused by both, and gets nothing written since.
    let after = note(&a, "After the tablet was lost");
    try_sync(&a, &b).await.unwrap();
    assert!(has_note(&b, &after));
    assert!(try_sync(&a, &c).await.is_err());
    assert!(try_sync(&b, &c).await.is_err());
    assert!(!has_note(&c, &after));
    assert_eq!(account(&c), old);
    for n in [a, b, c] {
        n.close().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_offline_during_the_replacement_gets_the_key_from_any_device() {
    let (a, b, c, _) = three().await;
    a.replace_key(&[]).await.unwrap();
    let new = account(&a);

    // The phone dials the laptop; later the tablet, offline until now, dials
    // the phone rather than the laptop.
    try_sync(&a, &b).await.unwrap();
    try_sync(&b, &c).await.unwrap();
    assert_eq!(account(&c), new);

    // The other way round too: a device with the new key dialling one
    // without it hands it over.
    let d = joiner("desktop", &c).await;
    // (d joined from the tablet's new list, so it's on the new account.)
    assert_eq!(account(&d), new);
    let (e_old, e) = {
        // A device that joined before the replacement and missed it.
        let (a2, b2, c2, _) = three().await;
        let before = account(&a2);
        a2.replace_key(&[]).await.unwrap();
        try_sync(&b2, &a2).await.unwrap(); // the laptop dials the phone
        assert_ne!(account(&b2), before);
        c2.close().await;
        a2.close().await;
        (before, b2)
    };
    assert_ne!(account(&e), e_old);
    for n in [a, b, c, d, e] {
        n.close().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_removed_device_is_refused_once_the_list_reaches_the_others() {
    let (a, b, c, _) = three().await;
    a.remove_device(c.device()).await.unwrap();

    // The laptop refuses the tablet at once, and the tablet learns why.
    assert!(try_sync(&a, &c).await.is_err());
    assert!(c.state().lock().unwrap().account.removed(&c.device()));
    // The phone hasn't heard yet; once it syncs with the laptop it has.
    try_sync(&a, &b).await.unwrap();
    assert!(try_sync(&b, &c).await.is_err());
    // A removed device stays removed, even re-added from an old list.
    assert!(a.remove_device(c.device()).await.is_err());
    for n in [a, b, c] {
        n.close().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_recovery_kit_brings_the_account_back_until_it_is_replaced() {
    let (a, b, c, secret) = three().await;
    let id = note(&a, "Kept on the laptop");
    try_sync(&a, &b).await.unwrap();

    // Every device lost but the laptop: a new phone recovers with the kit.
    let r = node("new phone", |me| Account::recover(&secret, me)).await;
    assert_eq!(account(&r), account(&a));
    try_sync(&a, &r).await.unwrap();
    assert!(has_note(&r, &id));
    {
        let state = r.state().lock().unwrap();
        let names: Vec<&str> = state
            .account
            .devices
            .list
            .devices
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        assert!(
            names.contains(&"laptop") && names.contains(&"new phone"),
            "{names:?}"
        );
        assert_eq!(state.account.devices.kit_made_at(), Some(1_794_200_000));
    }

    // A new kit retires the old one: a device recovering with it is refused.
    r.replace_key(&[b.device(), c.device()]).await.unwrap();
    try_sync(&r, &a).await.unwrap();
    let old_kit = node("thief", |me| Account::recover(&secret, me)).await;
    assert!(try_sync(&a, &old_kit).await.is_err());
    assert!(try_sync(&r, &old_kit).await.is_err());
    for n in [a, b, c, r, old_kit] {
        n.close().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_out_warns_about_unsent_changes_and_reaches_the_others() {
    let (a, b, _c, _) = three().await;
    let unsent = note(&b, "Only on the phone");
    let others = [a.device()];
    assert!(
        b.state()
            .lock()
            .unwrap()
            .replica
            .only_here(&others)
            .unwrap()
            > 0
    );
    try_sync(&a, &b).await.unwrap();
    assert!(has_note(&a, &unsent));
    assert_eq!(
        b.state()
            .lock()
            .unwrap()
            .replica
            .only_here(&others)
            .unwrap(),
        0
    );

    // The phone signs out and tells the laptop.
    b.leave_account().await.unwrap();
    b.add_addr_hint(a.device(), a.loopback_addr());
    let serving = {
        let a = a.clone();
        tokio::spawn(async move { while a.accept().await.is_some() {} })
    };
    assert!(b.announce().await >= 1);
    assert!(a.state().lock().unwrap().account.removed(&b.device()));
    serving.abort();
    a.close().await;
    b.close().await;
}

/// Adds a photo with made-up bytes to a note, as an import would.
async fn add_photo(node: &SyncNode, note: &str) -> mirrorleaf_core::PhotoRef {
    use mirrorleaf_core::{PhotoRef, photos};
    let full: Vec<u8> = (0..300_000).map(|i| (i * 31 % 251) as u8).collect();
    let thumbnail = b"a small thumbnail ".repeat(1000);
    let photo = PhotoRef {
        full: photos::hash(&full),
        thumbnail: photos::hash(&thumbnail),
    };
    {
        let replica = &mut node.state().lock().unwrap().replica;
        replica.stage_blob(photo.full, &full).unwrap();
        replica.stage_blob(photo.thumbnail, &thumbnail).unwrap();
        replica.add_photo(note, &photo, 4, 3).unwrap();
    }
    node.store_staged().await.unwrap();
    photo
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backup_restores_notes_and_photos_with_or_without_encryption() {
    use mirrorleaf_net::backup::{BackupReader, key_for};

    let (a, b, c, secret) = three().await;
    let id = note(&a, "Holiday");
    let photo = add_photo(&a, &id).await;

    for encrypt in [false, true] {
        let (bytes, counts) = a.write_backup(Vec::new(), encrypt).await.unwrap();
        assert!(counts.changes > 0);
        assert_eq!(counts.photos, 2, "the thumbnail and the full image");

        // Every device lost: a new one recovers with the kit, then restores.
        let r = node("new laptop", |me| Account::recover(&secret, me)).await;
        let reader = BackupReader::open(bytes.as_slice()).unwrap();
        assert_eq!(reader.header().encrypted, encrypt);
        let key = {
            let state = r.state().lock().unwrap();
            key_for(reader.header(), state.account.key.as_ref(), None).unwrap()
        };
        let mut reader = reader;
        if let Some(key) = key {
            reader.unlock(key);
        }
        let restored = r.read_backup(reader).await.unwrap();
        assert!(restored.changes > 0 && restored.photos == 2);
        assert!(has_note(&r, &id));
        assert!(r.blob(photo.full).await.unwrap().is_some());
        r.close().await;
    }

    // An encrypted backup made before the key was replaced opens only with
    // the kit it was made with.
    let (old_backup, _) = a.write_backup(Vec::new(), true).await.unwrap();
    a.replace_key(&[]).await.unwrap();
    let reader = BackupReader::open(old_backup.as_slice()).unwrap();
    let new_key = a
        .state()
        .lock()
        .unwrap()
        .account
        .key
        .as_ref()
        .unwrap()
        .to_bytes();
    let new_key = mirrorleaf_proto::AccountKey::from_bytes(new_key);
    let needs = key_for(reader.header(), Some(&new_key), None).unwrap_err();
    assert!(
        needs.to_string().contains("earlier recovery kit"),
        "{needs}"
    );
    let words = secret.words().join(" ");
    assert!(
        key_for(reader.header(), Some(&new_key), Some(&words))
            .unwrap()
            .is_some()
    );
    assert!(
        key_for(
            reader.header(),
            None,
            Some(&RecoverySecret::generate().words().join(" "))
        )
        .is_err(),
        "another account's kit"
    );
    for n in [a, b, c] {
        n.close().await;
    }
}

/// Two devices find each other on the local network with nothing but their
/// IDs: no addresses given, no relay, no discovery server (M12, mDNS). Needs a
/// network interface that carries multicast, so it runs by hand:
/// `cargo test -p mirrorleaf-net --test lifecycle -- --ignored local_network`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs multicast on this machine's network"]
async fn devices_find_each_other_on_the_local_network() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "mirrorleaf_net=debug".into()),
        )
        .try_init();
    let a = node("laptop", |me| Account::create_with_kit(me, 1_794_200_000).0).await;
    let b = joiner("phone", &a).await;
    assert!(a.set_local_discovery(true));
    assert!(b.set_local_discovery(true));
    let accepting = {
        let b = b.clone();
        tokio::spawn(async move { b.accept().await })
    };
    let only_id = mirrorleaf_net::endpoint_addr(b.device()).unwrap();
    let dialled =
        tokio::time::timeout(std::time::Duration::from_secs(30), a.sync_once(only_id)).await;
    accepting.abort();
    assert!(
        matches!(dialled, Ok(Ok(_))),
        "found and synced over the local network: {dialled:?}"
    );
    assert!(a.on_this_network().contains(&b.device()));
    // Stopping ends the announcements: A stops seeing B once its record
    // expires, and B no longer answers lookups.
    assert!(!b.set_local_discovery(false));
    assert!(!b.local_discovery_running());
    assert!(b.on_this_network().is_empty());
}

/// A device remembered at an address on the home network is dialled there
/// with no lookup at all (M12: no internet, and a network that drops mDNS).
#[tokio::test(flavor = "multi_thread")]
async fn a_remembered_home_address_is_dialled_without_any_lookup() {
    let a = node("laptop", |me| Account::create_with_kit(me, 1_794_200_000).0).await;
    let b = joiner("phone", &a).await;
    // Network::Local has no lookups, so dialling by ID alone can't work.
    let accepting = {
        let b = b.clone();
        tokio::spawn(async move { b.accept().await })
    };
    let alone = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        a.sync_once(a.dial_addr(b.device()).unwrap()),
    )
    .await;
    assert!(!matches!(alone, Ok(Ok(_))), "nothing to dial");
    accepting.abort();

    // As if remembered from an earlier direct connection on this network.
    let at = b.loopback_addr().ip_addrs().next().copied().unwrap();
    a.state()
        .lock()
        .unwrap()
        .replica
        .set_meta(
            &format!("lan_addr:{}", b.device()),
            at.to_string().as_bytes(),
        )
        .unwrap();
    assert_eq!(a.remembered_lan_addr(b.device()), Some(at));
    let accepting = {
        let b = b.clone();
        tokio::spawn(async move { b.accept().await })
    };
    let dialled = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        a.sync_once(a.dial_addr(b.device()).unwrap()),
    )
    .await;
    accepting.abort();
    assert!(matches!(dialled, Ok(Ok(_))), "{dialled:?}");
    // A loopback connection isn't the home network: nothing new remembered.
    assert_eq!(b.remembered_lan_addr(a.device()), None);
}

/// A device listens on the same port after a restart, so the address the
/// other devices remember for it stays right (M12).
#[tokio::test(flavor = "multi_thread")]
async fn a_device_listens_on_the_same_port_again() {
    let dir = std::env::temp_dir().join(format!("mirrorleaf-port-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("device.db");
    let secret = SecretKey::generate();
    let device = DeviceId(*secret.public().as_bytes());
    let bind = |secret: SecretKey| {
        let replica = Replica::open(&path, device).unwrap();
        let account = Account::create_with_kit(
            DeviceEntry {
                id: device,
                name: "laptop".into(),
                platform: None,
                added_at: 0,
            },
            1_794_200_000,
        )
        .0;
        SyncNode::bind(
            State { replica, account },
            secret,
            Network::Local,
            BlobStorage::Memory,
        )
    };
    let port = |node: &SyncNode| node.loopback_addr().ip_addrs().next().unwrap().port();
    let first = bind(secret.clone()).await.unwrap();
    let was = port(&first);
    first.close().await;
    drop(first);
    let again = bind(secret).await.unwrap();
    assert_eq!(port(&again), was);
    again.close().await;
    std::fs::remove_dir_all(&dir).unwrap();
}
