//! Loopback tests (implementation plan, M1): real Iroh peers in one process,
//! connected over 127.0.0.1 with relays and address lookup switched off.

use std::time::Duration;

use mirrorleaf_core::{BlobKind, PhotoRef, Replica, photos};
use mirrorleaf_net::{
    Account, AuthError, BlobStorage, Event, Network, PathKind, PhotoStore, SecretKey, State,
    SyncNode,
};
use mirrorleaf_proto::{AccountKey, DeviceEntry, DeviceId};

async fn node(name: &str, account: impl FnOnce(DeviceEntry) -> Account) -> SyncNode {
    let secret = SecretKey::generate();
    let device = DeviceId(*secret.public().as_bytes());
    let replica = Replica::open_in_memory(device).unwrap();
    let account = account(DeviceEntry {
        id: device,
        name: name.into(),
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

/// Two devices on one developer account, the second joined from the first's list.
async fn pair() -> (SyncNode, SyncNode) {
    let a = node("laptop", Account::create).await;
    let (key, devices) = {
        let state = a.state().lock().unwrap();
        let key = state.account.key.as_ref().unwrap().to_bytes();
        (key, state.account.devices.clone())
    };
    let b = node("phone", |me| {
        Account::join(AccountKey::from_bytes(key), &devices, me).unwrap()
    })
    .await;
    (a, b)
}

/// B dials A and they run one round.
async fn sync(a: &SyncNode, b: &SyncNode) -> mirrorleaf_net::RoundReport {
    let accepting = {
        let a = a.clone();
        tokio::spawn(async move { a.accept().await.unwrap().unwrap() })
    };
    let (path, report) = b.sync_once(a.loopback_addr()).await.unwrap();
    assert_eq!(path, PathKind::Direct);
    accepting.await.unwrap();
    report
}

fn paragraphs(node: &SyncNode, note: &str) -> Vec<String> {
    node.state()
        .lock()
        .unwrap()
        .replica
        .note_paragraphs(note)
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn two_peers_sync_over_loopback() {
    let (a, b) = pair().await;
    let from_a = {
        let replica = &mut a.state().lock().unwrap().replica;
        let id = replica.create_note("From the laptop").unwrap();
        replica
            .append_paragraph(&id, "written offline on A")
            .unwrap();
        id
    };
    let from_b = {
        let replica = &mut b.state().lock().unwrap().replica;
        let id = replica.create_note("From the phone").unwrap();
        replica
            .append_paragraph(&id, "written offline on B")
            .unwrap();
        id
    };

    let report = sync(&a, &b).await;
    assert!(report.sent_changes > 0 && report.received_changes > 0);

    for node in [&a, &b] {
        let state = node.state().lock().unwrap();
        assert_eq!(state.replica.list_notes().unwrap().len(), 2);
        // A learned about B from B's newer, signed device list.
        assert_eq!(state.account.devices.list.devices.len(), 2);
    }
    assert_eq!(paragraphs(&b, &from_a), ["written offline on A"]);
    assert_eq!(paragraphs(&a, &from_b), ["written offline on B"]);
    a.close().await;
    b.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_edits_to_one_paragraph_merge() {
    let (a, b) = pair().await;
    let note = {
        let replica = &mut a.state().lock().unwrap().replica;
        let id = replica.create_note("Shared").unwrap();
        replica.append_paragraph(&id, "start end").unwrap();
        id
    };
    sync(&a, &b).await;

    a.state()
        .lock()
        .unwrap()
        .replica
        .insert_text(&note, 0, 5, " from-A")
        .unwrap();
    b.state()
        .lock()
        .unwrap()
        .replica
        .insert_text(&note, 0, 9, " from-B")
        .unwrap();
    sync(&a, &b).await;

    let merged = paragraphs(&a, &note);
    assert_eq!(merged, paragraphs(&b, &note));
    assert_eq!(merged, ["start from-A end from-B"]);
    a.close().await;
    b.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn one_text_edit_costs_under_5_kb() {
    let (a, b) = pair().await;
    let note = {
        let replica = &mut a.state().lock().unwrap().replica;
        let id = replica.create_note("Traffic").unwrap();
        replica.append_paragraph(&id, "first line").unwrap();
        id
    };
    sync(&a, &b).await;

    a.state()
        .lock()
        .unwrap()
        .replica
        .append_paragraph(&note, "A sentence typed on one device.")
        .unwrap();
    let report = sync(&a, &b).await;
    // The whole session: Hello with device lists, version vectors, the edit, End, Ack.
    eprintln!("traffic for one text edit: {:?}", report.traffic);
    assert!(report.traffic.total() <= 5 * 1024, "{:?}", report.traffic);
    assert_eq!(report.received_changes, 2, "the note and its index entry");
    a.close().await;
    b.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn devices_from_another_account_are_refused() {
    let a = node("laptop", Account::create).await;
    let stranger = node("stranger", Account::create).await;
    let accepting = {
        let a = a.clone();
        tokio::spawn(async move { a.accept().await.unwrap() })
    };
    // Each side refuses the other. The stranger either spots the mismatch itself
    // or is told why when A closes the connection.
    let err = stranger.sync_once(a.loopback_addr()).await.unwrap_err();
    let reason = format!("{err:#}");
    assert!(
        matches!(err.downcast_ref(), Some(AuthError::OtherAccount))
            || reason.contains("refused: the peer belongs to another account"),
        "{reason}"
    );
    assert!(accepting.await.unwrap().is_err());
    assert!(
        a.state()
            .lock()
            .unwrap()
            .replica
            .list_notes()
            .unwrap()
            .is_empty()
    );
    a.close().await;
    stranger.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn live_mode_pushes_new_edits() {
    let (a, b) = pair().await;
    a.add_addr_hint(b.device(), b.loopback_addr());
    b.add_addr_hint(a.device(), a.loopback_addr());
    let (events, mut log) = tokio::sync::mpsc::unbounded_channel();
    for node in [&a, &b] {
        let (node, events) = (node.clone(), events.clone());
        tokio::spawn(async move { node.watch(events).await });
    }
    // Wait until the two are connected.
    tokio::time::timeout(Duration::from_secs(10), async {
        while !matches!(log.recv().await, Some(mirrorleaf_net::Event::Synced { .. })) {}
    })
    .await
    .unwrap();

    let note = {
        let replica = &mut b.state().lock().unwrap().replica;
        let id = replica.create_note("Live").unwrap();
        replica
            .append_paragraph(&id, "pushed without asking")
            .unwrap();
        id
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if a.state().lock().unwrap().replica.find_note(&note).is_ok()
                && paragraphs(&a, &note) == ["pushed without asking"]
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the edit should reach A within seconds");
    a.close().await;
    b.close().await;
}

/// Made-up blob contents: what an imported photo's full image would be.
fn image_bytes(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

/// Adds a photo with made-up image bytes to a note on `node`, and stores its
/// blobs as an import would.
async fn add_photo(node: &SyncNode, note: &str, full: &[u8]) -> PhotoRef {
    let thumbnail = b"a small thumbnail ".repeat(1000);
    let photo = PhotoRef {
        full: photos::hash(full),
        thumbnail: photos::hash(&thumbnail),
    };
    {
        let replica = &mut node.state().lock().unwrap().replica;
        replica.stage_blob(photo.full, full).unwrap();
        replica.stage_blob(photo.thumbnail, &thumbnail).unwrap();
        replica.add_photo(note, &photo, 4, 3).unwrap();
    }
    node.store_staged().await.unwrap();
    photo
}

fn new_note(node: &SyncNode, title: &str) -> String {
    node.state()
        .lock()
        .unwrap()
        .replica
        .create_note(title)
        .unwrap()
}

/// Serves A's incoming connections (blob requests included) until A closes.
fn serve(a: &SyncNode) {
    let a = a.clone();
    tokio::spawn(async move { a.accept().await });
}

#[tokio::test(flavor = "multi_thread")]
async fn thumbnails_follow_the_note_and_full_images_come_on_request() {
    let (a, b) = pair().await;
    let note = new_note(&a, "Holiday");
    let full = image_bytes(300_000);
    let photo = add_photo(&a, &note, &full).await;
    sync(&a, &b).await;

    b.add_addr_hint(a.device(), a.loopback_addr());
    serve(&a);
    let fetch = b.fetch_missing(a.device()).await.unwrap();
    assert_eq!(fetch.path, Some(PathKind::Direct));
    assert!(fetch.failed.is_empty(), "{:?}", fetch.failed);
    let kinds: Vec<_> = fetch.fetched.iter().map(|(kind, _)| *kind).collect();
    assert_eq!(
        kinds,
        [BlobKind::Thumbnail],
        "only the thumbnail comes unasked"
    );
    assert!(b.blob(photo.thumbnail).await.unwrap().is_some());
    assert!(b.blob(photo.full).await.unwrap().is_none());

    b.state()
        .lock()
        .unwrap()
        .replica
        .want_blob(photo.full)
        .unwrap();
    let fetch = b.fetch_missing(a.device()).await.unwrap();
    let (kind, report) = &fetch.fetched[0];
    assert_eq!((*kind, report.len), (BlobKind::Full, 300_000));
    assert_eq!(b.blob(photo.full).await.unwrap().unwrap(), full);
    let status = b.state().lock().unwrap().replica.photos().unwrap();
    assert!(status[0].has_full && status[0].has_thumbnail && !status[0].wanted);
    a.close().await;
    b.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_transfer_resumes() {
    let (a, b) = pair().await;
    let note = new_note(&a, "Big photo");
    let full = image_bytes(2_000_000);
    let photo = add_photo(&a, &note, &full).await;
    sync(&a, &b).await;
    serve(&a);

    // The transfer stops once about a quarter has arrived.
    let stopped = b
        .fetch_blob(a.loopback_addr(), photo.full, Some(500_000))
        .await
        .unwrap_err();
    assert!(
        stopped.to_string().starts_with("stopped after"),
        "{stopped:#}"
    );
    assert!(b.blob(photo.full).await.unwrap().is_none());

    let report = b
        .fetch_blob(a.loopback_addr(), photo.full, None)
        .await
        .unwrap();
    eprintln!("resumed transfer read {} bytes", report.bytes_read);
    assert_eq!(report.len, 2_000_000);
    assert!(
        report.bytes_read < 1_600_000,
        "only the rest is sent again, not {} bytes",
        report.bytes_read
    );
    assert_eq!(b.blob(photo.full).await.unwrap().unwrap(), full);
    a.close().await;
    b.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn blobs_are_served_only_to_the_account() {
    let a = node("laptop", Account::create).await;
    let note = new_note(&a, "Private");
    let photo = add_photo(&a, &note, &image_bytes(10_000)).await;
    let stranger = node("stranger", Account::create).await;
    serve(&a);

    let err = stranger
        .fetch_blob(a.loopback_addr(), photo.thumbnail, None)
        .await
        .unwrap_err();
    let reason = mirrorleaf_net::describe(&err);
    assert!(
        reason.contains("refused: the peer isn't on this account's device list"),
        "{reason}"
    );
    assert!(stranger.blob(photo.thumbnail).await.unwrap().is_none());
    a.close().await;
    stranger.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn live_mode_fetches_new_thumbnails() {
    let (a, b) = pair().await;
    a.add_addr_hint(b.device(), b.loopback_addr());
    b.add_addr_hint(a.device(), a.loopback_addr());
    let (events, mut log) = tokio::sync::mpsc::unbounded_channel();
    for node in [&a, &b] {
        let (node, events) = (node.clone(), events.clone());
        tokio::spawn(async move { node.watch(events).await });
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while !matches!(log.recv().await, Some(Event::Synced { .. })) {}
    })
    .await
    .unwrap();

    // Staged as `photo add` in another process would leave it; A's watch stores it.
    let note = new_note(&a, "Live photo");
    let thumbnail = b"live thumbnail".repeat(500);
    let photo = PhotoRef {
        full: photos::hash(b"full"),
        thumbnail: photos::hash(&thumbnail),
    };
    {
        let replica = &mut a.state().lock().unwrap().replica;
        replica.stage_blob(photo.full, b"full").unwrap();
        replica.stage_blob(photo.thumbnail, &thumbnail).unwrap();
        replica.add_photo(&note, &photo, 4, 3).unwrap();
    }
    let fetched = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match log.recv().await {
                Some(Event::Fetched {
                    peer, kind, report, ..
                }) => break (peer, kind, report),
                Some(_) => {}
                None => panic!("watch stopped"),
            }
        }
    })
    .await
    .expect("the thumbnail should reach B within seconds");
    assert_eq!(fetched.0, a.device());
    assert_eq!(
        (fetched.1, fetched.2.hash),
        (BlobKind::Thumbnail, photo.thumbnail)
    );
    assert_eq!(b.blob(photo.thumbnail).await.unwrap().unwrap(), thumbnail);
    assert!(b.blob(photo.full).await.unwrap().is_none());
    a.close().await;
    b.close().await;
}

/// The engine binds a node for each push-woken job and keeps one photo store
/// open for the whole process (ADR-005), so closing a node must leave a shared
/// store working for the next one.
#[tokio::test(flavor = "multi_thread")]
async fn a_shared_photo_store_outlives_the_nodes_using_it() {
    let dir = std::env::temp_dir().join(format!("mirrorleaf-shared-store-{}", std::process::id()));
    let store = PhotoStore::open(&BlobStorage::Disk(dir.clone()))
        .await
        .unwrap();
    let secret = SecretKey::generate();
    let device = DeviceId(*secret.public().as_bytes());
    let full = image_bytes(50_000);
    let mut photo = None;
    for job in 0..3 {
        let replica = Replica::open_in_memory(device).unwrap();
        let account = Account::create(DeviceEntry {
            id: device,
            name: "phone".into(),
        });
        let node = SyncNode::bind_with_store(
            State { replica, account },
            secret.clone(),
            Network::Local,
            &store,
        )
        .await
        .unwrap();
        if job == 0 {
            let note = new_note(&node, "Photos");
            photo = Some(add_photo(&node, &note, &full).await);
        }
        let held = node.blob(photo.unwrap().full).await.unwrap();
        assert_eq!(held.as_deref(), Some(&full[..]), "job {job}");
        node.close().await;
    }
    store.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}
