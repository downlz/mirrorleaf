//! Plan §9: yrs and Yjs must read each other's updates. Both sides build the
//! same small note with a fixed client ID, compare it with a committed fixture,
//! and read the other side's fixture (`packages/editor/src/interop.test.ts`).
//! Set UPDATE_FIXTURES=1 to rewrite a fixture after a deliberate change.

use std::path::PathBuf;
use std::str::FromStr;

use mirrorleaf_core::yrs::types::Attrs;
use mirrorleaf_core::yrs::{
    Any, Doc, ReadTxn, StateVector, Text, Transact, WriteTxn, Xml, XmlElementPrelim, XmlFragment,
    XmlTextPrelim,
};
use mirrorleaf_core::{BlobHash, Block, CONTENT_FRAGMENT, DeviceId, PhotoRef, Replica};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn hash(c: char) -> BlobHash {
    BlobHash::from_str(&c.to_string().repeat(64)).unwrap()
}

/// The note as the Rust side writes it: a formatted paragraph, then a photo.
fn yrs_note() -> Vec<u8> {
    let doc = Doc::with_client_id(2);
    let mut txn = doc.transact_mut();
    let content = txn.get_or_insert_xml_fragment(CONTENT_FRAGMENT);
    let paragraph = content.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
    let text = paragraph.insert(&mut txn, 0, XmlTextPrelim::new(""));
    let image = content.insert(&mut txn, 1, XmlElementPrelim::empty("image"));
    text.insert(&mut txn, 0, "From ");
    let italic = Attrs::from([("italic".into(), Any::Map(Default::default()))]);
    text.insert_with_attributes(&mut txn, 5, "Rust", italic);
    let photo = PhotoRef {
        full: hash('b'),
        thumbnail: hash('a'),
    };
    image.insert_attribute(&mut txn, "src", photo.to_uri());
    image.insert_attribute(&mut txn, "width", 640.0);
    txn.encode_state_as_update_v1(&StateVector::default())
}

#[test]
fn yrs_writes_the_fixture_yjs_reads() {
    let path = fixture("yrs-note.bin");
    let update = yrs_note();
    if std::env::var_os("UPDATE_FIXTURES").is_some() || !path.exists() {
        std::fs::write(&path, &update).unwrap();
    }
    assert_eq!(std::fs::read(&path).unwrap(), update);
}

#[test]
fn the_core_reads_what_yjs_wrote() {
    let update = std::fs::read(fixture("yjs-note.bin"))
        .expect("run `pnpm -r test` once to write the Yjs fixture");
    let mut replica = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
    replica.record_local("note", update).unwrap();
    let photo = PhotoRef {
        full: hash('a'),
        thumbnail: hash('b'),
    };
    assert_eq!(
        replica.note_blocks("note").unwrap(),
        [
            Block::Paragraph("Hello bold".into()),
            Block::Photo {
                photo,
                width: 2048,
                height: 1536
            },
        ]
    );
    // The photo was found in the update's bytes without loading the document.
    assert_eq!(replica.photos().unwrap()[0].photo, photo);
}
