//! Loopback tests (implementation plan, M1): real Iroh peers in one process,
//! connected over 127.0.0.1 with relays and address lookup switched off.

use std::time::Duration;

use mirrorleaf_core::Replica;
use mirrorleaf_net::{Account, AuthError, Network, PathKind, SecretKey, State, SyncNode};
use mirrorleaf_proto::{AccountKey, DeviceEntry, DeviceId};

async fn node(name: &str, account: impl FnOnce(DeviceEntry) -> Account) -> SyncNode {
    let secret = SecretKey::generate();
    let device = DeviceId(*secret.public().as_bytes());
    let replica = Replica::open_in_memory(device).unwrap();
    let account = account(DeviceEntry {
        id: device,
        name: name.into(),
    });
    SyncNode::bind(State { replica, account }, secret, Network::Local)
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
