//! Saving the recovery kit (E2) and backups (E9) where the person chooses, and
//! picking a backup to restore: a Save or Open dialog on the desktop, Android's
//! own pickers on Android (`DocumentsPlugin.kt`). The engine writes and reads
//! the files; they never pass through the web view. Also restarting the app
//! after signing out (E3), so the engine opens afresh.

use std::sync::Arc;

use mirrorleaf_ffi::Engine;
use tauri::{AppHandle, Runtime};

fn engine() -> Result<Arc<Engine>, String> {
    Engine::get().ok_or_else(|| "the engine isn't open".to_owned())
}

/// Saves the kit waiting to be saved as "pdf" or "text". `false` if the
/// person cancelled.
pub async fn save_kit<R: Runtime>(app: &AppHandle<R>, kind: &str) -> Result<bool, String> {
    let engine = engine()?;
    let name = engine.kit_file_name(kind).map_err(|e| e.to_string())?;
    #[cfg(desktop)]
    {
        use tauri_plugin_dialog::DialogExt;
        let (sent, chosen) = tokio::sync::oneshot::channel();
        let dialog = app.dialog().file().set_file_name(&name);
        let dialog = if kind == "pdf" {
            dialog.add_filter("PDF", &["pdf"])
        } else {
            dialog.add_filter("Text", &["txt"])
        };
        dialog.save_file(move |path| {
            let _ = sent.send(path);
        });
        let Some(path) = chosen.await.map_err(|e| e.to_string())? else {
            return Ok(false);
        };
        let path = path.into_path().map_err(|e| e.to_string())?;
        engine
            .write_kit(kind, &path.to_string_lossy())
            .map_err(|e| e.to_string())?;
        Ok(true)
    }
    #[cfg(target_os = "android")]
    {
        // Written to the app's own storage, then copied by Kotlin to the
        // place picked, which deletes this copy.
        let path = std::path::Path::new(&engine.data_dir()).join(format!("kit-export-{kind}"));
        let path = path.to_string_lossy().into_owned();
        engine.write_kit(kind, &path).map_err(|e| e.to_string())?;
        let mime = if kind == "pdf" {
            "application/pdf"
        } else {
            "text/plain"
        };
        crate::device::save_document(app, path, name, mime.to_owned()).await
    }
}

/// Writes a backup and saves it where the person chooses. `None` if they
/// cancelled.
pub async fn save_backup<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<Option<mirrorleaf_ffi::BackupDone>, String> {
    let engine = engine()?;
    let name = engine.backup_file_name();
    #[cfg(desktop)]
    {
        use tauri_plugin_dialog::DialogExt;
        let (sent, chosen) = tokio::sync::oneshot::channel();
        app.dialog()
            .file()
            .set_file_name(&name)
            .add_filter("Mirrorleaf backup", &[mirrorleaf_ffi::BACKUP_EXTENSION])
            .save_file(move |path| {
                let _ = sent.send(path);
            });
        let Some(path) = chosen.await.map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let path = path.into_path().map_err(|e| e.to_string())?;
        let done = engine
            .clone()
            .write_backup(path.to_string_lossy().into_owned())
            .await
            .map_err(|e| e.to_string())?;
        engine.backup_saved().map_err(|e| e.to_string())?;
        Ok(Some(done))
    }
    #[cfg(target_os = "android")]
    {
        // Written to the app's own storage, then copied by Kotlin to the
        // place picked, which deletes this copy. Needs room for the backup
        // twice while it is copied.
        let path = std::path::Path::new(&engine.data_dir()).join("backup-export");
        let path = path.to_string_lossy().into_owned();
        let done = engine
            .clone()
            .write_backup(path.clone())
            .await
            .map_err(|e| e.to_string())?;
        let saved = crate::device::save_document(
            app,
            path.clone(),
            name,
            "application/octet-stream".to_owned(),
        )
        .await;
        let _ = std::fs::remove_file(&path);
        if !saved? {
            return Ok(None);
        }
        engine.backup_saved().map_err(|e| e.to_string())?;
        Ok(Some(done))
    }
}

/// A backup picked to restore, and what it says about itself.
#[derive(serde::Serialize)]
pub struct PickedBackup {
    pub path: String,
    pub info: mirrorleaf_ffi::BackupInfo,
}

/// Where Android copies a picked backup, in the app's own storage.
#[cfg(target_os = "android")]
fn import_copy(engine: &Engine) -> std::path::PathBuf {
    std::path::Path::new(&engine.data_dir()).join("backup-import")
}

/// Lets the person pick a backup to restore. `None` if they cancelled.
pub async fn pick_backup<R: Runtime>(app: &AppHandle<R>) -> Result<Option<PickedBackup>, String> {
    let engine = engine()?;
    #[cfg(desktop)]
    let path = {
        use tauri_plugin_dialog::DialogExt;
        let (sent, chosen) = tokio::sync::oneshot::channel();
        app.dialog()
            .file()
            .add_filter("Mirrorleaf backup", &[mirrorleaf_ffi::BACKUP_EXTENSION])
            .pick_file(move |path| {
                let _ = sent.send(path);
            });
        let Some(path) = chosen.await.map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        path.into_path()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned()
    };
    #[cfg(target_os = "android")]
    let path = {
        let path = import_copy(&engine).to_string_lossy().into_owned();
        if !crate::device::open_document(app, path.clone()).await? {
            return Ok(None);
        }
        path
    };
    match engine.backup_info(&path) {
        Ok(info) => Ok(Some(PickedBackup { path, info })),
        Err(e) => {
            forget_backup(&path);
            Err(e.to_string())
        }
    }
}

/// Deletes Android's copy of a picked backup, once restored or cancelled. The
/// desktop reads the file where it is, so there is nothing to delete there.
pub fn forget_backup(path: &str) {
    #[cfg(target_os = "android")]
    if let Some(engine) = Engine::get()
        && std::path::Path::new(path) == import_copy(&engine)
    {
        let _ = std::fs::remove_file(path);
    }
    let _ = path;
}

// --- Files dropped on the window or the Dock icon (M12, E5) ---

/// Most of a dropped text file Mirrorleaf takes into a note.
#[cfg(target_os = "macos")]
const DROPPED_TEXT_LIMIT: u64 = 1 << 20;

/// Whether a file is a photo Mirrorleaf can take, by its name.
#[cfg(target_os = "macos")]
pub fn is_photo_name(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    matches!(
        ext.as_deref(),
        Some(
            "jpg"
                | "jpeg"
                | "png"
                | "heic"
                | "heif"
                | "gif"
                | "webp"
                | "tif"
                | "tiff"
                | "bmp"
                | "avif"
        )
    )
}

/// Keeps a copy of a dropped photo in the app's own folder, for
/// `share_into_note`, which reads and then deletes the copies it is given.
pub fn stage_dropped(bytes: &[u8]) -> Result<String, String> {
    let engine = engine()?;
    let path =
        std::path::Path::new(&engine.data_dir()).join(format!("dropped-{:016x}", rand_suffix()));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

fn rand_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ u64::from(std::process::id()).rotate_left(32)
}

/// Makes a note from files dropped on the Dock icon: photos by copy (the
/// originals stay where they are), text files read. The screen opens it.
#[cfg(target_os = "macos")]
pub async fn note_from_paths(paths: Vec<std::path::PathBuf>) -> Result<(), String> {
    use std::io::Read;
    let engine = engine()?;
    let mut text = Vec::new();
    let mut photos = Vec::new();
    for path in paths {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let Some(name) = name else { continue };
        if is_photo_name(&name) {
            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            photos.push(stage_dropped(&bytes)?);
        } else {
            let mut read = String::new();
            std::fs::File::open(&path)
                .and_then(|f| f.take(DROPPED_TEXT_LIMIT).read_to_string(&mut read))
                .map_err(|e| format!("can't read {name}: {e}"))?;
            text.push(read);
        }
    }
    let text = (!text.is_empty()).then(|| text.join("\n"));
    engine
        .share_into_note(None, text, photos)
        .await
        .map(drop)
        .map_err(|e| e.to_string())
}

/// Starts the app again, in a new process.
pub async fn restart<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    #[cfg(desktop)]
    app.restart();
    #[cfg(target_os = "android")]
    crate::device::restart(app).await
}
