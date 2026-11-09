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
