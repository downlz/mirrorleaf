//! The photo import pipeline (plan §5.5): decode, turn upright, resize to at
//! most 2048 px, compress to at most 600 KB, and make a 320 px thumbnail.
//!
//! Output is JPEG. The full image keeps the photo's EXIF block (including its
//! location) and colour profile, by the owner's decision (ADR-003); only the
//! orientation tag is reset, because the pixels are already turned upright. The
//! thumbnail keeps the colour profile and nothing else.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageReader, Rgb, RgbImage,
    RgbaImage,
};
use mirrorleaf_proto::BlobHash;

use crate::{CoreError, PhotoRef, Replica, Result, heif};

/// Longest side of the full image, in pixels.
pub const FULL_EDGE: u32 = 2048;
/// Longest side of the thumbnail, in pixels.
pub const THUMBNAIL_EDGE: u32 = 320;
/// Largest full image, in bytes (the lightweight target, plan §7.3).
pub const MAX_FULL_BYTES: usize = 600_000;
/// JPEG qualities tried in turn until the full image fits.
const QUALITIES: [u8; 6] = [85, 80, 75, 70, 65, 60];
const THUMBNAIL_QUALITY: u8 = 75;
/// A JPEG APP1 segment holds at most this much EXIF.
const MAX_EXIF_BYTES: usize = 65_000;

/// Pixels from a platform decoder: 8-bit RGBA, upright.
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decodes the formats Rust can't (HEIC and AVIF) with the platform's decoder:
/// ImageIO on macOS, Android's ImageDecoder from M6 (plan §5.5).
pub trait PlatformDecoder: Send + Sync {
    /// Decodes `bytes`, applying the photo's orientation, scaled so that neither
    /// side is over `max_edge`.
    fn decode(&self, bytes: &[u8], max_edge: u32) -> Result<DecodedImage, String>;
}

/// The decoder this platform provides from Rust, if any.
pub fn system_decoder() -> Option<Box<dyn PlatformDecoder>> {
    #[cfg(target_os = "macos")]
    return Some(Box::new(crate::imageio::ImageIo));
    #[cfg(not(target_os = "macos"))]
    None
}

/// A photo ready to store: both blobs, named by their hashes.
pub struct PreparedPhoto {
    pub photo: PhotoRef,
    pub full: Vec<u8>,
    pub thumbnail: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Size of the EXIF block kept in the full image (0 if the photo had none).
    pub exif_bytes: usize,
    /// The JPEG quality the full image needed to fit.
    pub quality: u8,
}

/// Runs the import pipeline on a photo file's bytes.
pub fn prepare(bytes: &[u8], platform: Option<&dyn PlatformDecoder>) -> Result<PreparedPhoto> {
    let (image, exif, icc) = if heif::is_heif(bytes) {
        let decoder = platform.ok_or_else(|| {
            photo_error(
                "HEIC and AVIF photos need the system's decoder, which isn't available here",
            )
        })?;
        let decoded = decoder.decode(bytes, FULL_EDGE).map_err(CoreError::Photo)?;
        let pixels = RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba)
            .ok_or_else(|| photo_error("the system decoder returned a malformed image"))?;
        // The platform decodes into sRGB, so no colour profile is needed.
        (DynamicImage::ImageRgba8(pixels), heif::exif(bytes), None)
    } else {
        let mut decoder = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(photo_error)?
            .into_decoder()
            .map_err(photo_error)?;
        let exif = decoder.exif_metadata().map_err(photo_error)?;
        let icc = decoder.icc_profile().map_err(photo_error)?;
        let orientation = decoder.orientation().map_err(photo_error)?;
        let mut image = DynamicImage::from_decoder(decoder).map_err(photo_error)?;
        image.apply_orientation(orientation);
        (image, exif, icc)
    };
    // The pixels are upright now, so viewers mustn't turn them again.
    let exif = exif
        .map(|mut exif| {
            let _applied = Orientation::remove_from_exif_chunk(&mut exif);
            exif
        })
        .filter(|exif| exif.len() <= MAX_EXIF_BYTES);

    let mut pixels = flatten(&fit(image, FULL_EDGE));
    let (full, quality) = loop {
        let mut fitted = None;
        for quality in QUALITIES {
            let jpeg = encode(&pixels, quality, exif.as_deref(), icc.as_deref())?;
            if jpeg.len() <= MAX_FULL_BYTES {
                fitted = Some((jpeg, quality));
                break;
            }
        }
        match fitted {
            Some(found) => break found,
            // Still too big at the lowest quality: a smaller image, and again.
            None if pixels.width() > THUMBNAIL_EDGE && pixels.height() > THUMBNAIL_EDGE => {
                let (w, h) = (pixels.width() * 4 / 5, pixels.height() * 4 / 5);
                pixels = image::imageops::resize(&pixels, w, h, FilterType::Lanczos3);
            }
            None => return Err(photo_error("can't compress this photo below 600 KB")),
        }
    };
    let thumbnail = flatten(&fit(
        DynamicImage::ImageRgb8(pixels.clone()),
        THUMBNAIL_EDGE,
    ));
    let thumbnail = encode(&thumbnail, THUMBNAIL_QUALITY, None, icc.as_deref())?;

    Ok(PreparedPhoto {
        photo: PhotoRef {
            full: hash(&full),
            thumbnail: hash(&thumbnail),
        },
        width: pixels.width(),
        height: pixels.height(),
        exif_bytes: exif.map_or(0, |e| e.len()),
        quality,
        full,
        thumbnail,
    })
}

pub fn hash(bytes: &[u8]) -> BlobHash {
    BlobHash(*blake3::hash(bytes).as_bytes())
}

impl Replica {
    /// Imports a photo file into note `id`: prepares it, stages both blobs for
    /// the blob store, and adds the photo at the end of the note.
    pub fn import_photo(
        &mut self,
        id: &str,
        bytes: &[u8],
        platform: Option<&dyn PlatformDecoder>,
    ) -> Result<PreparedPhoto> {
        let prepared = prepare(bytes, platform)?;
        self.stage_blob(prepared.photo.full, &prepared.full)?;
        self.stage_blob(prepared.photo.thumbnail, &prepared.thumbnail)?;
        self.add_photo(id, &prepared.photo, prepared.width, prepared.height)?;
        Ok(prepared)
    }
}

/// `image` scaled down, keeping its proportions, so neither side is over `edge`.
fn fit(image: DynamicImage, edge: u32) -> DynamicImage {
    if image.width().max(image.height()) <= edge {
        image
    } else {
        image.resize(edge, edge, FilterType::Lanczos3)
    }
}

/// Opaque RGB pixels; transparent areas become white, since JPEG has no alpha.
fn flatten(image: &DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.to_rgb8();
    }
    let rgba = image.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let over_white =
            |c: u8| ((u32::from(c) * u32::from(a) + 255 * (255 - u32::from(a))) / 255) as u8;
        Rgb([over_white(r), over_white(g), over_white(b)])
    })
}

fn encode(
    pixels: &RgbImage,
    quality: u8,
    exif: Option<&[u8]>,
    icc: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut out, quality);
    if let Some(exif) = exif {
        encoder
            .set_exif_metadata(exif.to_vec())
            .map_err(photo_error)?;
    }
    if let Some(icc) = icc {
        encoder.set_icc_profile(icc.to_vec()).map_err(photo_error)?;
    }
    encoder
        .write_image(
            pixels.as_raw(),
            pixels.width(),
            pixels.height(),
            ExtendedColorType::Rgb8,
        )
        .map_err(photo_error)?;
    Ok(out)
}

fn photo_error(e: impl ToString) -> CoreError {
    CoreError::Photo(e.to_string())
}

#[cfg(test)]
mod tests {
    use image::{GenericImageView, ImageFormat};
    use mirrorleaf_proto::DeviceId;
    use rand::{RngExt, SeedableRng};

    use super::*;
    use crate::Block;

    /// A little-endian EXIF block holding an orientation and a camera make.
    pub(crate) fn exif(orientation: u16) -> Vec<u8> {
        let mut tiff = b"II*\0\x08\0\0\0\x02\0".to_vec();
        tiff.extend([0x12, 0x01, 3, 0, 1, 0, 0, 0]);
        tiff.extend(orientation.to_le_bytes());
        tiff.extend([0, 0]);
        tiff.extend([0x0f, 0x01, 2, 0, 4, 0, 0, 0]);
        tiff.extend(b"MLF\0");
        tiff.extend([0, 0, 0, 0]);
        tiff
    }

    /// A noisy photo-sized JPEG: noise compresses worst, so it's the hardest case
    /// for the 600 KB limit. The top half is red and the bottom half blue.
    fn camera_jpeg(width: u32, height: u32, exif: Option<Vec<u8>>) -> Vec<u8> {
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let pixels = RgbImage::from_fn(width, height, |_, y| {
            let noise: u8 = rng.random_range(0..40);
            if y < height / 2 {
                Rgb([215 + noise, noise, noise])
            } else {
                Rgb([noise, noise, 215 + noise])
            }
        });
        let mut out = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut out, 95);
        if let Some(exif) = exif {
            encoder.set_exif_metadata(exif).unwrap();
        }
        encoder
            .write_image(pixels.as_raw(), width, height, ExtendedColorType::Rgb8)
            .unwrap();
        out
    }

    fn decode(jpeg: &[u8]) -> DynamicImage {
        image::load_from_memory_with_format(jpeg, ImageFormat::Jpeg).unwrap()
    }

    #[test]
    fn a_12_mp_photo_fits_the_limits_and_keeps_its_exif() {
        // 4032 × 3024 taken with the phone on its side: EXIF orientation 6 means
        // "turn 90° clockwise to view".
        let original = camera_jpeg(4032, 3024, Some(exif(6)));
        let prepared = prepare(&original, None).unwrap();
        assert!(
            prepared.full.len() <= MAX_FULL_BYTES,
            "{} bytes",
            prepared.full.len()
        );
        assert_eq!((prepared.width, prepared.height), (1536, 2048));

        let full = decode(&prepared.full);
        assert_eq!(full.dimensions(), (1536, 2048));
        // Turned clockwise, the red top half is now on the right.
        let right = full.get_pixel(1500, 1024);
        assert!(right[0] > 150 && right[2] < 100, "{right:?}");

        let mut decoder =
            image::codecs::jpeg::JpegDecoder::new(Cursor::new(&prepared.full)).unwrap();
        let kept = decoder.exif_metadata().unwrap().unwrap();
        assert_eq!(
            Orientation::from_exif_chunk(&kept),
            Some(Orientation::NoTransforms)
        );
        assert!(kept.windows(4).any(|w| w == b"MLF\0"), "camera make kept");

        let thumbnail = decode(&prepared.thumbnail);
        assert_eq!(thumbnail.dimensions(), (240, 320));
        assert!(prepared.thumbnail.len() < 40_000);
        let mut decoder =
            image::codecs::jpeg::JpegDecoder::new(Cursor::new(&prepared.thumbnail)).unwrap();
        assert_eq!(decoder.exif_metadata().unwrap(), None);

        assert_eq!(prepared.photo.full, hash(&prepared.full));
        assert_ne!(prepared.photo.full, prepared.photo.thumbnail);
    }

    #[test]
    fn small_and_transparent_images_are_kept_as_they_are() {
        let mut png = Vec::new();
        RgbaImage::from_pixel(300, 200, image::Rgba([0, 0, 0, 0]))
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let prepared = prepare(&png, None).unwrap();
        assert_eq!((prepared.width, prepared.height), (300, 200));
        let white = decode(&prepared.full).get_pixel(10, 10);
        assert!(
            white[0] > 245 && white[2] > 245,
            "transparent became {white:?}"
        );
        assert_eq!(prepared.exif_bytes, 0);
    }

    #[test]
    fn heic_needs_a_platform_decoder() {
        struct Grey;
        impl PlatformDecoder for Grey {
            fn decode(&self, _: &[u8], max_edge: u32) -> Result<DecodedImage, String> {
                let (width, height) = (max_edge, max_edge * 3 / 4);
                let rgba = vec![128; (width * height * 4) as usize];
                Ok(DecodedImage {
                    width,
                    height,
                    rgba,
                })
            }
        }
        let heic = crate::heif::tests::heic_with_exif(&exif(6));
        assert!(matches!(prepare(&heic, None), Err(CoreError::Photo(_))));

        let prepared = prepare(&heic, Some(&Grey)).unwrap();
        assert_eq!((prepared.width, prepared.height), (2048, 1536));
        let mut decoder =
            image::codecs::jpeg::JpegDecoder::new(Cursor::new(&prepared.full)).unwrap();
        let kept = decoder.exif_metadata().unwrap().unwrap();
        assert_eq!(
            Orientation::from_exif_chunk(&kept),
            Some(Orientation::NoTransforms)
        );
    }

    #[test]
    fn importing_stages_both_blobs_and_adds_the_photo_to_the_note() {
        let mut replica = Replica::open_in_memory(DeviceId([1; 32])).unwrap();
        let note = replica.create_note("Trip").unwrap();
        let prepared = replica
            .import_photo(&note, &camera_jpeg(800, 600, None), None)
            .unwrap();
        assert_eq!(
            replica.note_blocks(&note).unwrap(),
            [Block::Photo {
                photo: prepared.photo,
                width: 800,
                height: 600
            }]
        );
        let staged = replica.staged_blobs(10).unwrap();
        assert_eq!(staged.len(), 2);
        assert!(staged.contains(&(prepared.photo.full, prepared.full.clone())));
        let status = &replica.photos().unwrap()[0];
        assert!(status.has_full && status.has_thumbnail);
    }

    /// Decodes a real HEIC made by macOS's `sips` from a test JPEG.
    #[cfg(target_os = "macos")]
    #[test]
    fn imageio_decodes_heic_upright() {
        let dir = std::env::temp_dir().join(format!("mirrorleaf-heic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (jpeg, heic) = (dir.join("in.jpg"), dir.join("out.heic"));
        std::fs::write(&jpeg, camera_jpeg(4032, 3024, Some(exif(6)))).unwrap();
        let status = std::process::Command::new("sips")
            .args(["-s", "format", "heic"])
            .arg(&jpeg)
            .arg("--out")
            .arg(&heic)
            .output()
            .unwrap();
        assert!(status.status.success(), "{status:?}");
        let bytes = std::fs::read(&heic).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(heif::is_heif(&bytes));

        let decoder = system_decoder().unwrap();
        let prepared = prepare(&bytes, Some(decoder.as_ref())).unwrap();
        assert_eq!((prepared.width, prepared.height), (1536, 2048));
        assert!(prepared.full.len() <= MAX_FULL_BYTES);
        let full = decode(&prepared.full);
        let right = full.get_pixel(1500, 1024);
        let left = full.get_pixel(30, 1024);
        assert!(right[0] > 150 && right[2] < 100, "right {right:?}");
        assert!(left[2] > 150 && left[0] < 100, "left {left:?}");
    }
}
