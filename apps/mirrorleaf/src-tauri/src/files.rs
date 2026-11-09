//! Saving the recovery kit where the person chooses (E2): a Save dialog on the
//! desktop, Android's own "Save to" picker on Android (`DocumentsPlugin.kt`).
//! The engine writes the file; the kit never passes through the web view.
//! Also restarting the app after signing out (E3), so the engine opens afresh.

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

/// Starts the app again, in a new process.
pub async fn restart<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    #[cfg(desktop)]
    app.restart();
    #[cfg(target_os = "android")]
    crate::device::restart(app).await
}
