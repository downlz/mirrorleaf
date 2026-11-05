//! What the platform's native code gives the engine and asks of it (M6): a
//! photo decoder for the formats Rust can't read, photos picked with the
//! system's photo picker, and text or photos shared into Mirrorleaf from other
//! apps. Android calls these from Kotlin through uniffi; they compile and are
//! tested everywhere.

use std::path::Path;
use std::sync::Arc;

use mirrorleaf_core::photos::{self, DecodedImage, PlatformDecoder};
use tracing::warn;

use super::PhotoInfo;
use super::{Engine, EngineError, UiEvent, lock};

/// The longest title a shared note gets from its text's first line.
const TITLE_CHARS: usize = 120;

/// Pixels from the platform's decoder: 8-bit RGBA, upright.
#[derive(Clone, Debug, uniffi::Record)]
pub struct DecodedPixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decodes HEIC and AVIF photos, which Rust can't (plan §5.5): Android's
/// `ImageDecoder` (M6), as macOS ImageIO does on the Mac. Returns nothing if
/// the photo can't be decoded.
#[uniffi::export(callback_interface)]
pub trait PhotoDecoder: Send + Sync {
    /// Decodes `bytes`, turned upright, scaled so neither side is over `max_edge`.
    fn decode(&self, bytes: Vec<u8>, max_edge: u32) -> Option<DecodedPixels>;
}

/// The native decoder as the core's photo pipeline uses it.
struct Native(Box<dyn PhotoDecoder>);

impl PlatformDecoder for Native {
    fn decode(&self, bytes: &[u8], max_edge: u32) -> Result<DecodedImage, String> {
        let pixels = self
            .0
            .decode(bytes.to_vec(), max_edge)
            .ok_or_else(|| "this device can't decode the photo".to_owned())?;
        if pixels.rgba.len() as u64 != u64::from(pixels.width) * u64::from(pixels.height) * 4 {
            return Err("the system decoder returned a malformed image".to_owned());
        }
        Ok(DecodedImage {
            width: pixels.width,
            height: pixels.height,
            rgba: pixels.rgba,
        })
    }
}

#[uniffi::export]
impl Engine {
    /// The platform's photo decoder, used for photos Rust can't decode.
    pub fn set_photo_decoder(&self, decoder: Box<dyn PhotoDecoder>) {
        *lock(&self.decoder) = Some(Arc::new(Native(decoder)));
    }

    /// Makes a note from what another app shared (plan M6): its title is the
    /// shared subject, or else the text's first line; then the text; then the
    /// photos, read from `photo_paths` (copies the app made, which are removed).
    /// The screen opens the note. Returns its ID.
    pub async fn share_into_note(
        self: Arc<Self>,
        subject: Option<String>,
        text: Option<String>,
        photo_paths: Vec<String>,
    ) -> Result<String, EngineError> {
        let (title, lines) = shared_title_and_lines(subject, text, photo_paths.len());
        let id = {
            let mut replica = lock(&self.replica);
            let id = replica.create_note_in(&title, None)?;
            for line in std::iter::once(title.as_str()).chain(lines.iter().map(String::as_str)) {
                replica.append_paragraph(&id, line)?;
            }
            id
        };
        for path in photo_paths {
            match self.clone().prepare_file(&path).await {
                Ok(prepared) => {
                    let mut replica = lock(&self.replica);
                    replica.stage_photo(&prepared)?;
                    replica.add_photo(&id, &prepared.photo, prepared.width, prepared.height)?;
                }
                Err(e) => warn!("a shared photo couldn't be added: {e}"),
            }
        }
        // The preview the list shows, until the note is first edited.
        let preview = lines.join(" ");
        lock(&self.replica).update_note_meta(&id, &title, &preview)?;
        *lock(&self.pending_open) = Some(id.clone());
        self.notify(UiEvent::NotesChanged);
        self.notify(UiEvent::OpenNote { id: id.clone() });
        Ok(id)
    }
}

/// For the Tauri screen, not exported to Kotlin.
impl Engine {
    /// The photo decoder to use: the native one if set, else the system's.
    pub(super) fn decoder(&self) -> Option<Arc<dyn PlatformDecoder>> {
        lock(&self.decoder)
            .clone()
            .or_else(|| photos::system_decoder().map(Arc::from))
    }

    /// Imports photos the system's photo picker copied to `paths`, removing the
    /// copies. Photos that fail are left out; fails only if every one did.
    pub async fn import_photo_files(
        self: Arc<Self>,
        paths: Vec<String>,
    ) -> Result<Vec<PhotoInfo>, EngineError> {
        let mut imported = Vec::new();
        let mut last_error = None;
        for path in &paths {
            match self.clone().prepare_file(path).await {
                Ok(prepared) => {
                    lock(&self.replica).stage_photo(&prepared)?;
                    imported.push(PhotoInfo {
                        src: prepared.photo.to_uri(),
                        width: prepared.width,
                        height: prepared.height,
                    });
                }
                Err(e) => last_error = Some(e),
            }
        }
        match last_error {
            Some(e) if imported.is_empty() => Err(e),
            _ => Ok(imported),
        }
    }

    /// The note the screen should open (a shared one), once.
    pub fn take_pending_open(&self) -> Option<String> {
        lock(&self.pending_open).take()
    }

    /// Reads a photo file, removes it, and prepares it on a blocking thread.
    async fn prepare_file(
        self: Arc<Self>,
        path: &str,
    ) -> Result<photos::PreparedPhoto, EngineError> {
        let path = path.to_owned();
        let decoder = self.decoder();
        self.runtime
            .spawn_blocking(move || {
                let bytes = std::fs::read(&path);
                let _ = std::fs::remove_file(Path::new(&path));
                let bytes =
                    bytes.map_err(|e| EngineError::Failed(format!("can't read the photo: {e}")))?;
                photos::prepare(&bytes, decoder.as_deref()).map_err(EngineError::from)
            })
            .await
            .map_err(|e| EngineError::Failed(format!("photo import failed: {e}")))?
    }
}

/// A shared note's title and its other lines. The title is the subject (as a
/// browser shares a page's title), else the text's first line, else a word
/// for the photos.
fn shared_title_and_lines(
    subject: Option<String>,
    text: Option<String>,
    photos: usize,
) -> (String, Vec<String>) {
    let clean = |s: Option<String>| s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let mut lines: Vec<String> = clean(text)
        .map(|t| {
            t.lines()
                .map(|l| l.trim_end().to_owned())
                .skip_while(|l| l.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let title = match clean(subject) {
        Some(subject) => subject,
        None if !lines.is_empty() => lines.remove(0),
        None if photos == 1 => "Shared photo".to_owned(),
        None if photos > 1 => "Shared photos".to_owned(),
        None => "Shared note".to_owned(),
    };
    let title: String = title.chars().take(TITLE_CHARS).collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    (title, lines)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mirrorleaf-native-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbImage::from_fn(width, height, |x, y| image::Rgb([x as u8, y as u8, 120]))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    /// Stands in for Android's ImageDecoder: counts calls, returns grey pixels.
    struct FakeDecoder(Arc<AtomicU32>);

    impl PhotoDecoder for FakeDecoder {
        fn decode(&self, _bytes: Vec<u8>, max_edge: u32) -> Option<DecodedPixels> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let (width, height) = (max_edge.min(400), max_edge.min(300));
            Some(DecodedPixels {
                width,
                height,
                rgba: vec![128; (width * height * 4) as usize],
            })
        }
    }

    #[test]
    fn shared_text_and_photos_become_a_note_the_screen_opens() {
        let dir = temp_dir("share");
        let engine = Arc::new(Engine::start(dir.to_str().unwrap(), "Test").unwrap());
        let opened = Arc::new(std::sync::Mutex::new(Vec::new()));
        let heard = opened.clone();
        engine.set_ui_listener(move |event| {
            if let UiEvent::OpenNote { id } = event {
                heard.lock().unwrap().push(id);
            }
        });
        let photo = dir.join("picked-1.png");
        std::fs::write(&photo, png(64, 48)).unwrap();

        let id = engine
            .runtime
            .block_on(engine.clone().share_into_note(
                None,
                Some("Packing list\nBoots\nMap".into()),
                vec![photo.to_string_lossy().into_owned()],
            ))
            .unwrap();
        assert!(!photo.exists(), "the copy is removed");
        let replica = lock(&engine.replica);
        let note = replica.find_note(&id).unwrap();
        assert_eq!(
            (note.title.as_str(), note.preview.as_str()),
            ("Packing list", "Boots Map")
        );
        let blocks = replica.note_blocks(&id).unwrap();
        assert_eq!(blocks.len(), 4);
        assert!(matches!(
            blocks[3],
            mirrorleaf_core::Block::Photo {
                width: 64,
                height: 48,
                ..
            }
        ));
        drop(replica);
        assert_eq!(engine.take_pending_open().as_deref(), Some(id.as_str()));
        assert_eq!(engine.take_pending_open(), None);
        assert_eq!(*opened.lock().unwrap(), [id]);
        drop(engine);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn picked_heic_photos_go_through_the_native_decoder() {
        let dir = temp_dir("picked");
        let engine = Arc::new(Engine::start(dir.to_str().unwrap(), "Test").unwrap());
        let calls = Arc::new(AtomicU32::new(0));
        engine.set_photo_decoder(Box::new(FakeDecoder(calls.clone())));
        // Only the file type box matters to the pipeline; the fake decodes it.
        let heic = dir.join("picked-1.heic");
        std::fs::write(&heic, b"\0\0\0\x18ftypheic\0\0\0\0mif1heic").unwrap();
        let broken = dir.join("picked-2.jpg");
        std::fs::write(&broken, b"not a photo").unwrap();

        let photos = engine
            .runtime
            .block_on(engine.clone().import_photo_files(vec![
                heic.to_string_lossy().into_owned(),
                broken.to_string_lossy().into_owned(),
            ]))
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(photos.len(), 1, "the broken file is left out");
        assert_eq!((photos[0].width, photos[0].height), (400, 300));
        assert!(!heic.exists() && !broken.exists());

        let only_broken = dir.join("picked-3.jpg");
        std::fs::write(&only_broken, b"still not a photo").unwrap();
        let failed = engine.runtime.block_on(
            engine
                .clone()
                .import_photo_files(vec![only_broken.to_string_lossy().into_owned()]),
        );
        assert!(failed.is_err(), "fails when every photo failed");
        drop(engine);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn shared_text_gets_a_title() {
        let (title, lines) = shared_title_and_lines(
            Some("Lake District walks".into()),
            Some("https://example.org/walks".into()),
            0,
        );
        assert_eq!(
            (title.as_str(), lines),
            (
                "Lake District walks",
                vec!["https://example.org/walks".to_owned()]
            )
        );

        let (title, lines) =
            shared_title_and_lines(None, Some("\n\nMilk\nEggs\n\nBread\n\n".into()), 0);
        assert_eq!(title, "Milk");
        assert_eq!(lines, ["Eggs", "", "Bread"]);

        assert_eq!(shared_title_and_lines(None, None, 2).0, "Shared photos");
        assert_eq!(
            shared_title_and_lines(Some(" ".into()), None, 1).0,
            "Shared photo"
        );
        assert_eq!(
            shared_title_and_lines(None, Some("x".repeat(300)), 0)
                .0
                .len(),
            TITLE_CHARS
        );
    }
}
