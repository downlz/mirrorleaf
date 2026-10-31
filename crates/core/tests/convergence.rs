//! Convergence test (implementation plan, M1): 3–5 peers make random offline edits,
//! including concurrent edits to the same paragraph and edit/delete races, then sync
//! in random orders. Every peer must end with identical documents, and no inserted
//! text may be lost unless it was deleted.
//!
//! Text is made of unique tokens ("p2x17 "), inserted only between tokens and
//! deleted only as whole tokens, so every token's fate can be checked at the end.

use std::collections::{BTreeMap, BTreeSet};

use mirrorleaf_core::yrs::{
    Any, GetString, Out, ReadTxn, Text, Transact, TransactionMut, WriteTxn, Xml, XmlElementPrelim,
    XmlElementRef, XmlFragment, XmlOut, XmlTextPrelim, XmlTextRef,
};
use mirrorleaf_core::{CONTENT_FRAGMENT, DeviceId, Replica};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{RngExt, SeedableRng};

#[test]
fn random_offline_edits_converge_without_losing_text() {
    let mut total = Coverage::default();
    for seed in 0..40 {
        let c = run(seed);
        total.inserted += c.inserted;
        total.deleted += c.deleted;
        total.paragraphs_deleted += c.paragraphs_deleted;
        total.races += c.races;
    }
    eprintln!("convergence coverage over 40 seeds: {total:?}");
    // Guard against a vacuous pass: the scenarios must include every kind of edit,
    // and real races where text went into a paragraph another peer deleted.
    assert!(total.inserted > 500, "{total:?}");
    assert!(total.deleted > 100, "{total:?}");
    assert!(total.paragraphs_deleted > 20, "{total:?}");
    assert!(total.races > 0, "{total:?}");
}

#[derive(Debug, Default)]
struct Coverage {
    inserted: usize,
    deleted: usize,
    paragraphs_deleted: usize,
    /// Tokens lost only because a peer concurrently deleted their paragraph.
    races: usize,
}

/// What the test did, to judge the final documents against.
#[derive(Default)]
struct Record {
    /// token → (note, paragraph it was inserted into)
    inserted: BTreeMap<String, (String, String)>,
    deleted_tokens: BTreeSet<String>,
    deleted_paragraphs: BTreeSet<(String, String)>,
}

fn run(seed: u64) -> Coverage {
    let mut rng = StdRng::seed_from_u64(seed);
    let peer_count = rng.random_range(3..=5);
    let mut peers: Vec<Replica> = (0..peer_count)
        .map(|i| Replica::open_in_memory(DeviceId([i as u8 + 1; 32])).unwrap())
        .collect();
    let mut record = Record::default();
    let mut counters = vec![0u32; peer_count];

    let notes: Vec<String> = (0..2)
        .map(|n| peers[0].create_note(&format!("note {n}")).unwrap())
        .collect();
    for note in &notes {
        add_paragraph(
            &mut peers[0],
            0,
            &mut counters[0],
            note,
            &mut rng,
            &mut record,
        );
    }
    for i in 1..peer_count {
        sync(&mut peers, 0, i);
    }

    for _ in 0..80 {
        let p = rng.random_range(0..peer_count);
        let note = notes[rng.random_range(0..notes.len())].clone();
        match rng.random_range(0..100) {
            0..40 => insert_token(
                &mut peers[p],
                p,
                &mut counters[p],
                &note,
                &mut rng,
                &mut record,
            ),
            40..60 => delete_tokens(&mut peers[p], &note, &mut rng, &mut record),
            60..70 => add_paragraph(
                &mut peers[p],
                p,
                &mut counters[p],
                &note,
                &mut rng,
                &mut record,
            ),
            70..74 => delete_paragraph(&mut peers[p], &note, &mut rng, &mut record),
            74..78 => peers[p]
                .set_title(&note, &format!("retitled by {p}"))
                .unwrap(),
            78..80 => peers[p].trash_note(&note).unwrap(),
            _ => {
                let q = rng.random_range(0..peer_count);
                if p != q {
                    sync(&mut peers, p, q);
                }
            }
        }
    }

    // Bring everyone up to date, in a random order.
    let mut pairs: Vec<(usize, usize)> = (0..peer_count)
        .flat_map(|a| {
            (0..peer_count)
                .filter(move |&b| a != b)
                .map(move |b| (a, b))
        })
        .collect();
    for _ in 0..2 {
        pairs.shuffle(&mut rng);
        for &(a, b) in &pairs {
            sync(&mut peers, a, b);
        }
    }

    let expected_vv = peers[0].version_vector().unwrap();
    let expected_notes = peers[0].list_notes().unwrap();
    let expected_docs: Vec<String> = notes.iter().map(|n| content_xml(&peers[0], n)).collect();
    for (i, peer) in peers.iter().enumerate() {
        assert_eq!(
            peer.version_vector().unwrap(),
            expected_vv,
            "seed {seed}: peer {i} change log"
        );
        assert_eq!(
            peer.list_notes().unwrap(),
            expected_notes,
            "seed {seed}: peer {i} index"
        );
        for (note, expected) in notes.iter().zip(&expected_docs) {
            assert_eq!(
                &content_xml(peer, note),
                expected,
                "seed {seed}: peer {i} note text"
            );
        }
    }

    let present = present_tokens(&peers[0], &notes);
    let mut races = 0;
    for (token, (note, pid)) in &record.inserted {
        let count = present.iter().filter(|t| *t == token).count();
        if record.deleted_tokens.contains(token) {
            assert_eq!(count, 0, "seed {seed}: deleted token {token} came back");
        } else if record
            .deleted_paragraphs
            .contains(&(note.clone(), pid.clone()))
        {
            assert!(count <= 1, "seed {seed}: token {token} duplicated");
            races += usize::from(count == 0);
        } else {
            assert_eq!(
                count, 1,
                "seed {seed}: token {token} was lost or duplicated"
            );
        }
    }
    Coverage {
        inserted: record.inserted.len(),
        deleted: record.deleted_tokens.len(),
        paragraphs_deleted: record.deleted_paragraphs.len(),
        races,
    }
}

fn sync(peers: &mut [Replica], from: usize, to: usize) {
    let theirs = peers[to].version_vector().unwrap();
    let mut cursor = 0;
    loop {
        let (batch, next) = peers[from].changes_since(&theirs, cursor, 2048).unwrap();
        if batch.is_empty() {
            break;
        }
        peers[to].apply_remote(&batch).unwrap();
        cursor = next;
    }
}

fn paragraphs(txn: &mut TransactionMut) -> Vec<XmlElementRef> {
    let content = txn.get_or_insert_xml_fragment(CONTENT_FRAGMENT);
    content
        .children(txn)
        .filter_map(|node| match node {
            XmlOut::Element(p) => Some(p),
            _ => None,
        })
        .collect()
}

fn pid<T: ReadTxn>(txn: &T, paragraph: &XmlElementRef) -> String {
    match paragraph.get_attribute(txn, "pid") {
        Some(Out::Any(Any::String(s))) => s.to_string(),
        other => panic!("paragraph without pid: {other:?}"),
    }
}

fn text_of(txn: &mut TransactionMut, paragraph: &XmlElementRef) -> XmlTextRef {
    match paragraph.get(txn, 0) {
        Some(XmlOut::Text(t)) => t,
        _ => paragraph.insert(txn, 0, XmlTextPrelim::new("")),
    }
}

fn add_paragraph(
    replica: &mut Replica,
    peer: usize,
    counter: &mut u32,
    note: &str,
    rng: &mut StdRng,
    record: &mut Record,
) {
    *counter += 1;
    let (pid, token) = (format!("p{peer}y{counter}"), format!("p{peer}x{counter}"));
    let at = rng.random_range(0..=8);
    replica
        .edit_doc(note, |txn| {
            let at = at.min(paragraphs(txn).len() as u32);
            let content = txn.get_or_insert_xml_fragment(CONTENT_FRAGMENT);
            let paragraph = content.insert(txn, at, XmlElementPrelim::empty("paragraph"));
            paragraph.insert_attribute(txn, "pid", pid.as_str());
            paragraph.insert(txn, 0, XmlTextPrelim::new(format!("{token} ")));
        })
        .unwrap();
    record.inserted.insert(token, (note.to_owned(), pid));
}

fn insert_token(
    replica: &mut Replica,
    peer: usize,
    counter: &mut u32,
    note: &str,
    rng: &mut StdRng,
    record: &mut Record,
) {
    *counter += 1;
    let token = format!("p{peer}x{counter}");
    let pick = rng.random_range(0..64usize);
    let slot = rng.random_range(0..64usize);
    let (inserted, _) = replica
        .edit_doc(note, |txn| {
            let all = paragraphs(txn);
            let paragraph = all.get(pick % all.len().max(1))?.clone();
            let text = text_of(txn, &paragraph);
            let current = text.get_string(txn);
            // Token boundaries: the start, and just after each space.
            let boundaries: Vec<u32> = std::iter::once(0)
                .chain(current.match_indices(' ').map(|(i, _)| i as u32 + 1))
                .collect();
            let offset = boundaries[slot % boundaries.len()];
            text.insert(txn, offset, &format!("{token} "));
            Some(pid(txn, &paragraph))
        })
        .unwrap();
    if let Some(pid) = inserted {
        record.inserted.insert(token, (note.to_owned(), pid));
    }
}

fn delete_tokens(replica: &mut Replica, note: &str, rng: &mut StdRng, record: &mut Record) {
    let pick = rng.random_range(0..64usize);
    let start = rng.random_range(0..64usize);
    let count = rng.random_range(1..=3usize);
    let (deleted, _) = replica
        .edit_doc(note, |txn| {
            let all = paragraphs(txn);
            let paragraph = all.get(pick % all.len().max(1))?.clone();
            let text = text_of(txn, &paragraph);
            let current = text.get_string(txn);
            let tokens: Vec<&str> = current.split_terminator(' ').collect();
            if tokens.is_empty() {
                return None;
            }
            let first = start % tokens.len();
            let last = (first + count).min(tokens.len());
            let from: usize = tokens[..first].iter().map(|t| t.len() + 1).sum();
            let len: usize = tokens[first..last].iter().map(|t| t.len() + 1).sum();
            text.remove_range(txn, from as u32, len as u32);
            Some(
                tokens[first..last]
                    .iter()
                    .map(|t| t.to_string())
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap();
    record.deleted_tokens.extend(deleted.into_iter().flatten());
}

fn delete_paragraph(replica: &mut Replica, note: &str, rng: &mut StdRng, record: &mut Record) {
    let pick = rng.random_range(0..64usize);
    let (deleted, _) = replica
        .edit_doc(note, |txn| {
            let all = paragraphs(txn);
            if all.is_empty() {
                return None;
            }
            let index = pick % all.len();
            let pid = pid(txn, &all[index]);
            txn.get_or_insert_xml_fragment(CONTENT_FRAGMENT)
                .remove_range(txn, index as u32, 1);
            Some(pid)
        })
        .unwrap();
    if let Some(pid) = deleted {
        record.deleted_paragraphs.insert((note.to_owned(), pid));
    }
}

fn content_xml(replica: &Replica, note: &str) -> String {
    let doc = replica.load_doc(note).unwrap();
    let txn = doc.transact();
    txn.get_xml_fragment(CONTENT_FRAGMENT)
        .map(|f| f.get_string(&txn))
        .unwrap_or_default()
}

fn present_tokens(replica: &Replica, notes: &[String]) -> Vec<String> {
    notes
        .iter()
        .flat_map(|note| replica.note_paragraphs(note).unwrap())
        .flat_map(|text| {
            text.split_terminator(' ')
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}
