use anyhow::{Result, bail};
use iroh::endpoint::{RecvStream, SendStream};
use mirrorleaf_proto::{
    DeviceId, DeviceListMerge, Hello, Message, PROTOCOL_VERSIONS, VersionVector, choose_version,
};
use tokio::sync::oneshot;

use crate::AuthError;
use crate::frame::{Traffic, read, write};
use crate::node::Shared;

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

/// Plan §5.3 steps 1–2: exchange Hellos, reconcile device lists, and accept the
/// peer only if its Iroh key is a device on our account's signed list.
pub(crate) async fn handshake(
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
        }
    };
    write(send, &Message::Hello(ours.clone()), &mut traffic.sent).await?;
    let theirs = match read(recv, &mut traffic.received).await? {
        Message::Hello(hello) => hello,
        other => bail!("expected Hello, got {other:?}"),
    };

    if choose_version(&theirs.versions).is_none() {
        bail!(AuthError::UpdateNeeded);
    }
    if theirs.device != remote {
        bail!(AuthError::WrongKey);
    }
    if theirs.account != ours.account {
        bail!(AuthError::OtherAccount);
    }
    theirs
        .device_list
        .verify(&ours.account)
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
    Ok(RoundReport {
        sent_changes,
        received_changes,
        traffic,
        peer_holds: ours.merged(&theirs),
    })
}
