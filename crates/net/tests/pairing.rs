//! Pairing over loopback (E1): a new device joins an account by a QR code or
//! a word code, with the four-digit check and the person's approval, and a
//! code gets exactly one attempt.

use std::sync::{Arc, Mutex};

use mirrorleaf_core::Replica;
use mirrorleaf_net::pair::{
    self, JoinRefused, JoinRequest, JoinTarget, PairingEvent, QrCode, WordCode,
};
use mirrorleaf_net::{Account, BlobStorage, Network, SecretKey, State, SyncNode};
use mirrorleaf_proto::ledger::Platform;
use mirrorleaf_proto::pair::Refusal;
use mirrorleaf_proto::{DeviceEntry, DeviceId};
use tokio::sync::mpsc;

/// The inviting device, on its own account, accepting connections.
async fn inviter() -> SyncNode {
    let secret = SecretKey::generate();
    let device = DeviceId(*secret.public().as_bytes());
    let replica = Replica::open_in_memory(device).unwrap();
    let account = Account::create(DeviceEntry {
        id: device,
        name: "mac".into(),
        platform: Some(Platform::Macos),
        added_at: 1,
    });
    let node = SyncNode::bind(
        State { replica, account },
        secret,
        Network::Local,
        BlobStorage::Memory,
    )
    .await
    .unwrap();
    let serving = node.clone();
    tokio::spawn(async move {
        // Hold each sync connection, so the peer can finish reading its round.
        let mut open = Vec::new();
        while let Some(accepted) = serving.accept().await {
            if let Ok((conn, _)) = accepted {
                open.push(conn);
            }
        }
    });
    node
}

/// Answers every request with `allow`, and keeps what the inviter reported.
fn answer_with(
    allow: bool,
    mut events: mpsc::UnboundedReceiver<PairingEvent>,
) -> Arc<Mutex<Vec<String>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let line = match event {
                PairingEvent::Request {
                    name,
                    number,
                    answer,
                    ..
                } => {
                    if allow {
                        answer.allow();
                    } else {
                        answer.deny();
                    }
                    format!("request {name} {number}")
                }
                PairingEvent::Joined { name, .. } => format!("joined {name}"),
                PairingEvent::Ended(reason) => format!("ended {reason:?}"),
            };
            log.lock().unwrap().push(line);
        }
    });
    seen
}

fn request() -> JoinRequest {
    JoinRequest {
        name: "phone".into(),
        platform: Platform::Android,
        app_version: "test".into(),
    }
}

/// The QR code as the joiner would scan it, pointed at loopback.
fn qr_target(node: &SyncNode, qr: &QrCode) -> JoinTarget {
    JoinTarget::qr(&QrCode {
        inviter: node.loopback_addr(),
        ..qr.clone()
    })
}

async fn join(
    secret: &SecretKey,
    target: &JoinTarget,
) -> (anyhow::Result<pair::Welcome>, Option<String>) {
    let number = Arc::new(Mutex::new(None));
    let shown = number.clone();
    let result = pair::join(
        secret.clone(),
        Network::Local,
        target,
        request(),
        move |n| {
            *shown.lock().unwrap() = Some(n);
        },
    )
    .await;
    let number = number.lock().unwrap().clone();
    (result, number)
}

async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
}

fn refusal(result: &anyhow::Result<pair::Welcome>) -> Option<Refusal> {
    result
        .as_ref()
        .err()
        .and_then(|e| e.downcast_ref::<JoinRefused>())
        .map(|r| r.0)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scanned_code_adds_the_device_and_it_then_syncs() {
    let a = inviter().await;
    let (events, rx) = mpsc::unbounded_channel();
    let seen = answer_with(true, rx);
    let invitation = a.invite(None, events);
    assert!(invitation.words.is_none(), "no ledger, no word code");

    let secret = SecretKey::generate();
    let me = DeviceId(*secret.public().as_bytes());
    let (welcome, number) = join(&secret, &qr_target(&a, &invitation.qr)).await;
    let welcome = welcome.unwrap();
    settle().await;

    // Both screens showed the same number, and the person was asked.
    let number = number.expect("the joiner shows a number");
    assert_eq!(
        *seen.lock().unwrap(),
        [format!("request phone {number}"), "joined phone".to_owned()]
    );
    // The inviter's list holds the phone, with its platform.
    let entry = a
        .state()
        .lock()
        .unwrap()
        .account
        .devices
        .list
        .entry(&me)
        .cloned();
    assert_eq!(
        entry.map(|e| (e.name, e.platform)),
        Some(("phone".into(), Some(Platform::Android)))
    );

    // The joiner checks what it received, and can sync straight away.
    let account = Account::from_pairing(
        welcome.account_secret,
        welcome.devices,
        welcome.approval,
        &me,
    )
    .unwrap();
    assert_eq!(account.id(), a.state().lock().unwrap().account.id());
    let note = {
        let replica = &mut a.state().lock().unwrap().replica;
        let id = replica.create_note("Packing list").unwrap();
        replica.append_paragraph(&id, "walking boots").unwrap();
        id
    };
    let b = SyncNode::bind(
        State {
            replica: Replica::open_in_memory(me).unwrap(),
            account,
        },
        secret,
        Network::Local,
        BlobStorage::Memory,
    )
    .await
    .unwrap();
    b.sync_once(a.loopback_addr()).await.unwrap();
    let paragraphs = b
        .state()
        .lock()
        .unwrap()
        .replica
        .note_paragraphs(&note)
        .unwrap();
    assert_eq!(paragraphs, ["walking boots"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_typed_word_code_works_too() {
    let a = inviter().await;
    let (events, rx) = mpsc::unbounded_channel();
    let seen = answer_with(true, rx);
    let words = WordCode::generate(7);
    let invitation = a.invite(Some(words.clone()), events);
    assert_eq!(invitation.words.as_ref(), Some(&words));

    // As typed, with the number the ledger would have resolved.
    let typed: WordCode = words.to_string().to_uppercase().parse().unwrap();
    let secret = SecretKey::generate();
    let target = JoinTarget::words(a.loopback_addr(), &typed, None);
    let (welcome, number) = join(&secret, &target).await;
    welcome.unwrap();
    settle().await;
    assert_eq!(
        *seen.lock().unwrap(),
        [
            format!("request phone {}", number.unwrap()),
            "joined phone".to_owned()
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_code_is_refused_and_ends_the_invitation() {
    let a = inviter().await;
    let (events, rx) = mpsc::unbounded_channel();
    let seen = answer_with(true, rx);
    let invitation = a.invite(Some(WordCode::generate(3)), events);

    // A guess at the words: refused, and nobody is asked.
    let guess: WordCode = "3-abandon-ability-able".parse().unwrap();
    let (result, number) = join(
        &SecretKey::generate(),
        &JoinTarget::words(a.loopback_addr(), &guess, None),
    )
    .await;
    assert_eq!(refusal(&result), Some(Refusal::WrongCode));
    assert_eq!(number, None, "no number is shown for a wrong code");

    // The code had its one attempt: even the right one is turned away now.
    let (result, _) = join(&SecretKey::generate(), &qr_target(&a, &invitation.qr)).await;
    assert_eq!(refusal(&result), Some(Refusal::CodeExpired));
    settle().await;
    assert_eq!(*seen.lock().unwrap(), ["ended WrongCode"]);
    assert_eq!(
        a.state().lock().unwrap().account.devices.list.devices.len(),
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_leaves_the_device_out() {
    let a = inviter().await;
    let (events, rx) = mpsc::unbounded_channel();
    let seen = answer_with(false, rx);
    let invitation = a.invite(None, events);
    let (result, number) = join(&SecretKey::generate(), &qr_target(&a, &invitation.qr)).await;
    assert_eq!(refusal(&result), Some(Refusal::Denied));
    settle().await;
    assert_eq!(
        *seen.lock().unwrap(),
        [
            format!("request phone {}", number.unwrap()),
            "ended Denied".to_owned()
        ]
    );
    assert_eq!(
        a.state().lock().unwrap().account.devices.list.devices.len(),
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_an_invitation_the_code_has_expired() {
    let a = inviter().await;
    let (events, _rx) = mpsc::unbounded_channel();
    let invitation = a.invite(None, events);
    a.cancel_invitation();
    let (result, _) = join(&SecretKey::generate(), &qr_target(&a, &invitation.qr)).await;
    assert_eq!(refusal(&result), Some(Refusal::CodeExpired));
}
