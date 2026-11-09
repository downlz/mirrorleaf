use std::time::Duration;

use anyhow::{Context, Result, bail};
use iroh::endpoint::{RecvStream, SendStream};
use mirrorleaf_proto::ledger::Platform;
use mirrorleaf_proto::{
    DeviceId, DeviceListMerge, Hello, Message, PROTOCOL_VERSIONS, VersionVector, choose_version,
};
use tokio::sync::oneshot;

use crate::frame::{Traffic, read, write};
use crate::node::Shared;
use crate::{Account, AuthError};

/// Changes are sent in batches of about this many bytes of updates.
const BATCH_BYTES: usize = 64 * 1024;

/// What one sync round moved.
#[derive(Clone, Debug, Default)]
pub struct RoundReport {
    pub sent_changes: usize,
    pub received_changes: usize,
    pub traffic: Traffic,
    /// Everything the peer holds once the round is complete.
    pub peer_holds: VersionVector,
}

/// How long a refusing device waits for the peer to read its `Hello` before
/// hanging up.
const REFUSAL_GRACE: Duration = Duration::from_secs(2);

/// Plan §5.3 steps 1–2: exchange Hellos, reconcile device lists, and accept the
/// peer only if its Iroh key is a device on our account's signed list.
pub(crate) async fn handshake(
    state: &Shared,
    send: &mut SendStream,
    recv: &mut RecvStream,
    remote: DeviceId,
    traffic: &mut Traffic,
) -> Result<()> {
    let result = exchange(state, send, recv, remote, traffic).await;
    if let Err(e) = &result
        && e.downcast_ref::<AuthError>().is_some()
    {
        // Our Hello is on its way, and with it our device list: let the peer
        // read it before the connection closes, so a removed device learns
        // that it was removed (E3). It reads it before it sends anything else
        // or hangs up itself.
        let _ = send.finish();
        let _ = tokio::time::timeout(REFUSAL_GRACE, read(recv, &mut traffic.received)).await;
    }
    result
}

async fn exchange(
    state: &Shared,
    send: &mut SendStream,
    recv: &mut RecvStream,
    remote: DeviceId,
    traffic: &mut Traffic,
) -> Result<()> {
    let ours = {
        let state = state.lock().unwrap();
        Hello {
            versions: PROTOCOL_VERSIONS.to_vec(),
            device: state.replica.device(),
            account: state.account.id(),
            device_list: state.account.devices.clone(),
            app_version: crate::app_version().to_owned(),
            platform: Platform::this(),
        }
    };
    write(
        send,
        &Message::Hello(Box::new(ours.clone())),
        &mut traffic.sent,
    )
    .await?;
    let theirs = match read(recv, &mut traffic.received).await? {
        Message::Hello(hello) => *hello,
        other => bail!("expected Hello, got {other:?}"),
    };

    if choose_version(&theirs.versions).is_none() {
        bail!(AuthError::UpdateNeeded);
    }
    if theirs.device != remote {
        bail!(AuthError::WrongKey);
    }
    if theirs.account != ours.account {
        let replaced = |list: &mirrorleaf_proto::SignedDeviceList| {
            list.list.rotated_from.as_ref().map(|link| link.previous)
        };
        if replaced(&ours.device_list) == Some(theirs.account) {
            // The peer still uses the key ours replaced (E3): hand it the new
            // one, if it is on the new list. It adopts it and goes on as one
            // of ours, so the round can follow.
            if !ours.device_list.contains(&remote) {
                bail!(AuthError::NotOnList);
            }
            let handover = state
                .lock()
                .unwrap()
                .account
                .handover()
                .context("this device lost the account's new key")?;
            write(
                send,
                &Message::Handover(Box::new(handover)),
                &mut traffic.sent,
            )
            .await?;
            return Ok(());
        }
        if replaced(&theirs.device_list) != Some(ours.account) {
            bail!(AuthError::OtherAccount);
        }
        // The peer replaced our account's key: it sends the new one next, if
        // this device is on the new list (otherwise it hangs up).
        theirs
            .device_list
            .verify_replacement_of(&ours.account)
            .map_err(AuthError::BadList)?;
        if !theirs.device_list.contains(&ours.device) {
            if theirs.device_list.was_removed(&ours.device) {
                // The new list, which our own key vouches for, says so.
                let mut state = state.lock().unwrap();
                Account::mark_removed(&mut state.replica)?;
                bail!(AuthError::Removed);
            }
            bail!(AuthError::NotOnList);
        }
        let handover = match read(recv, &mut traffic.received).await? {
            Message::Handover(handover) => *handover,
            other => bail!("expected the account's new key, got {other:?}"),
        };
        if handover.devices.list.account != theirs.account {
            bail!("the account's new key doesn't match the peer's account");
        }
        let mut state = state.lock().unwrap();
        let adopted = state.account.adopt(handover, &ours.device)?;
        state.account = adopted;
        let state = &mut *state;
        state.account.save(&mut state.replica)?;
        tracing::info!(
            "took the account's new key from {}: now account {}",
            remote.short(),
            theirs.account.short()
        );
    }
    let account = state.lock().unwrap().account.id();
    theirs
        .device_list
        .verify(&account)
        .map_err(AuthError::BadList)?;

    let mut state = state.lock().unwrap();
    let merged = DeviceListMerge::of(
        &state.account.devices,
        &theirs.device_list,
        state.account.key.as_ref(),
    );
    let changed = match merged {
        DeviceListMerge::Same | DeviceListMerge::KeepOurs => None,
        DeviceListMerge::AdoptTheirs => Some(theirs.device_list),
        DeviceListMerge::Resigned(list) => Some(list),
    };
    if let Some(list) = changed {
        state.account.devices = list;
        let state = &mut *state;
        state.account.save(&mut state.replica)?;
    }
    if state.account.removed(&ours.device) {
        bail!(AuthError::Removed);
    }
    if !state.account.devices.contains(&remote) {
        bail!(AuthError::NotOnList);
    }
    Ok(())
}

/// Which side of a round this device is: the one that opened its stream, or
/// the one answering.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Opener,
    Responder,
}

/// Plan §5.3 step 3: exchange version vectors, then each side streams what the
/// other lacks and ends with `End`. The responder answers the opener's `End`
/// with `Ack` once everything before it is stored. Only the opener waits for an
/// `Ack`, so when the opener's round ends nothing is left unread on either side
/// and it may hang up.
pub(crate) async fn round(
    state: &Shared,
    mut send: SendStream,
    mut recv: RecvStream,
    role: Role,
    remote: DeviceId,
) -> Result<RoundReport> {
    let mut traffic = Traffic::default();
    let ours = state.lock().unwrap().replica.version_vector()?;
    write(
        &mut send,
        &Message::VersionVector(ours.clone()),
        &mut traffic.sent,
    )
    .await?;
    let theirs = match read(&mut recv, &mut traffic.received).await? {
        Message::VersionVector(vv) => vv,
        other => bail!("expected a version vector, got {other:?}"),
    };

    let (got_their_end, their_end) = oneshot::channel();
    let (mut sent, mut received) = (0, 0);
    let sending = async {
        let mut cursor = 0;
        let mut count = 0;
        loop {
            let (batch, next) =
                state
                    .lock()
                    .unwrap()
                    .replica
                    .changes_since(&theirs, cursor, BATCH_BYTES)?;
            if batch.is_empty() {
                break;
            }
            cursor = next;
            count += batch.len();
            write(&mut send, &Message::Changes(batch), &mut sent).await?;
        }
        write(&mut send, &Message::End, &mut sent).await?;
        if role == Role::Responder {
            their_end.await?;
            write(&mut send, &Message::Ack, &mut sent).await?;
        }
        // The opener may already have hung up after reading our last message.
        let _ = send.finish();
        anyhow::Ok(count)
    };
    let receiving = async {
        let mut count = 0;
        loop {
            match read(&mut recv, &mut received).await? {
                Message::Changes(batch) => {
                    count += state.lock().unwrap().replica.apply_remote(&batch)?;
                }
                Message::End => break,
                other => bail!("expected changes, got {other:?}"),
            }
        }
        let _ = got_their_end.send(());
        if role == Role::Opener {
            match read(&mut recv, &mut received).await? {
                Message::Ack => {}
                other => bail!("expected Ack, got {other:?}"),
            }
        }
        anyhow::Ok(count)
    };
    let (sent_changes, received_changes) = tokio::try_join!(sending, receiving)?;
    traffic.sent += sent;
    traffic.received += received;
    let peer_holds = ours.merged(&theirs);
    // Remembered for "only on this device" (sign-out, E3). The opener's `Ack`
    // says the peer stored all we sent; the responder only knows what the
    // peer said it held.
    let known = match role {
        Role::Opener => &peer_holds,
        Role::Responder => &theirs,
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    state
        .lock()
        .unwrap()
        .replica
        .record_peer_holds(remote, known, now)?;
    Ok(RoundReport {
        sent_changes,
        received_changes,
        traffic,
        peer_holds,
    })
}
