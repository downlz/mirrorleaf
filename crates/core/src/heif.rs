//! Just enough of the HEIF container (ISO/IEC 23008-12) to recognise HEIC and
//! AVIF photos and find their EXIF block. The platform decodes their pixels
//! (plan §5.5); the metadata is read here so every platform keeps the same EXIF.

/// Whether `bytes` is a HEIF-family image (HEIC, AVIF), which Rust can't decode.
pub(crate) fn is_heif(bytes: &[u8]) -> bool {
    bytes.get(4..8) == Some(b"ftyp")
        && matches!(
            bytes.get(8..12),
            Some(b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"mif1" | b"msf1")
                | Some(b"avif" | b"avis")
        )
}

/// The photo's EXIF block as a TIFF structure (starting `II*\0` or `MM\0*`), as
/// the JPEG encoder takes it. `None` if there is none or the file is malformed.
pub(crate) fn exif(bytes: &[u8]) -> Option<Vec<u8>> {
    let meta = boxes(bytes).find(|b| &b.kind == b"meta")?.body.get(4..)?;
    let item = exif_item_id(boxes(meta).find(|b| &b.kind == b"iinf")?.body)?;
    let (offset, len) = item_extent(boxes(meta).find(|b| &b.kind == b"iloc")?.body, item)?;
    let payload = bytes.get(offset..offset.checked_add(len)?)?;
    // The item starts with the offset of the TIFF header, after the 4 bytes holding it.
    let skip = u32::from_be_bytes(payload.get(..4)?.try_into().ok()?) as usize;
    let tiff = payload.get(4usize.checked_add(skip)?..)?;
    let tiff = tiff.strip_prefix(b"Exif\0\0").unwrap_or(tiff);
    matches!(tiff.get(..4), Some(b"II*\0" | b"MM\0*")).then(|| tiff.to_vec())
}

struct HeifBox<'a> {
    kind: [u8; 4],
    body: &'a [u8],
}

/// The boxes laid end to end in `bytes`, stopping at the first malformed one.
fn boxes(mut bytes: &[u8]) -> impl Iterator<Item = HeifBox<'_>> {
    std::iter::from_fn(move || {
        let size = u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
        let kind: [u8; 4] = bytes.get(4..8)?.try_into().ok()?;
        let (header, size) = match size {
            0 => (8, bytes.len()),
            1 => (
                16,
                usize::try_from(u64::from_be_bytes(bytes.get(8..16)?.try_into().ok()?)).ok()?,
            ),
            n => (8, n),
        };
        let body = bytes.get(header..size)?;
        bytes = &bytes[size..];
        Some(HeifBox { kind, body })
    })
}

/// The ID of the item whose type is `Exif`, from the item information box.
fn exif_item_id(iinf: &[u8]) -> Option<u32> {
    let version = *iinf.first()?;
    let entries = if version == 0 { 4 + 2 } else { 4 + 4 };
    boxes(iinf.get(entries..)?)
        .filter(|b| &b.kind == b"infe")
        .find_map(|infe| {
            let body = infe.body;
            let (id, kind) = match *body.first()? {
                2 => (read(body, 4, 2)? as u32, body.get(8..12)?),
                3 => (read(body, 4, 4)? as u32, body.get(10..14)?),
                _ => return None,
            };
            (kind == b"Exif").then_some(id)
        })
}

/// Where item `id` lies in the file, from the item location box. Only items
/// stored at a file offset in one extent are supported, which is how cameras
/// store EXIF.
fn item_extent(iloc: &[u8], id: u32) -> Option<(usize, usize)> {
    let version = *iloc.first()?;
    let sizes = *iloc.get(4)?;
    let (offset_size, length_size) = ((sizes >> 4) as usize, (sizes & 0xf) as usize);
    let sizes = *iloc.get(5)?;
    let base_offset_size = (sizes >> 4) as usize;
    let index_size = if version == 0 {
        0
    } else {
        (sizes & 0xf) as usize
    };
    let id_size = if version < 2 { 2 } else { 4 };
    let (count, mut at) = (read(iloc, 6, id_size)?, 6 + id_size);
    for _ in 0..count {
        let item = read(iloc, at, id_size)? as u32;
        at += id_size;
        let method = if version == 0 {
            0
        } else {
            at += 2;
            read(iloc, at - 2, 2)? & 0xf
        };
        at += 2; // data reference index
        let base = read(iloc, at, base_offset_size)?;
        at += base_offset_size;
        let extents = read(iloc, at, 2)?;
        at += 2;
        let mut first = None;
        for _ in 0..extents {
            at += index_size;
            let offset = read(iloc, at, offset_size)?;
            let length = read(iloc, at + offset_size, length_size)?;
            at += offset_size + length_size;
            first.get_or_insert((offset, length));
        }
        if item == id {
            let (offset, length) = first?;
            return (method == 0 && extents == 1).then_some((
                usize::try_from(base.checked_add(offset)?).ok()?,
                usize::try_from(length).ok()?,
            ));
        }
    }
    None
}

/// A big-endian unsigned number of `size` bytes (0, 2, 4 or 8) at `at`.
fn read(bytes: &[u8], at: usize, size: usize) -> Option<u64> {
    let field = bytes.get(at..at.checked_add(size)?)?;
    Some(field.iter().fold(0, |n, b| n << 8 | u64::from(*b)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend(kind);
        out.extend(body);
        out
    }

    /// A minimal HEIC file whose only item is an EXIF block holding `tiff`, laid
    /// out the way phones write it.
    pub(crate) fn heic_with_exif(tiff: &[u8]) -> Vec<u8> {
        let ftyp = bx(b"ftyp", b"heic\0\0\0\0mif1heic");
        let mut infe = vec![2, 0, 0, 0, 0, 7, 0, 0];
        infe.extend(b"Exif");
        let mut iinf = vec![0, 0, 0, 0, 0, 1];
        iinf.extend(bx(b"infe", &infe));
        let mut payload = 6u32.to_be_bytes().to_vec();
        payload.extend(b"Exif\0\0");
        payload.extend(tiff);
        // iloc version 1: item 7 in one extent, 4-byte offset and length, no
        // base offset or index.
        let meta = |data_at: usize| {
            let mut iloc = vec![1, 0, 0, 0, 0x44, 0x00, 0, 1, 0, 7, 0, 0, 0, 0, 0, 1];
            iloc.extend((data_at as u32).to_be_bytes());
            iloc.extend((payload.len() as u32).to_be_bytes());
            let mut meta = vec![0, 0, 0, 0];
            meta.extend(bx(b"iinf", &iinf));
            meta.extend(bx(b"iloc", &iloc));
            bx(b"meta", &meta)
        };
        let data_at = ftyp.len() + meta(0).len() + 8;
        let mut file = ftyp;
        file.extend(meta(data_at));
        file.extend(bx(b"mdat", &payload));
        file
    }

    #[test]
    fn finds_the_exif_block_in_a_heic_file() {
        let tiff = b"MM\0*\0\0\0\x08rest of the block";
        let file = heic_with_exif(tiff);
        assert!(is_heif(&file));
        assert_eq!(exif(&file).as_deref(), Some(&tiff[..]));
        assert!(!is_heif(b"\xff\xd8\xff\xe0 a jpeg"));
        assert_eq!(exif(&file[..file.len() - 4]), None);
    }
}
