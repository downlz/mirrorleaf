//! mirrorleaf-app: the Tauri app (implementation plan §5.7). The Phase 0 screen is
//! a throwaway test page; what it drives, the engine, is the real foundation.
//!
//! On Android the Kotlin startup code opens the engine before the screen exists,
//! because a push-woken job may need it with no screen at all. On the desktop
//! the app opens it here.

use std::sync::Arc;

use mirrorleaf_ffi::{Engine, EngineStatus, NoteSummary};

fn engine() -> Result<Arc<Engine>, String> {
    Engine::get().ok_or_else(|| "the engine isn't open".to_owned())
}

#[tauri::command]
fn status() -> Result<EngineStatus, String> {
    engine()?.status().map_err(|e| e.to_string())
}

#[tauri::command]
async fn join(ticket: String) -> Result<String, String> {
    let engine = engine()?;
    let joined = engine
        .clone()
        .join(ticket)
        .await
        .map_err(|e| e.to_string())?;
    // The app is open, so start syncing straight away.
    match engine.go_live().await {
        Ok(()) => Ok(joined),
        Err(e) => Ok(format!("{joined} Live sync didn't start: {e}")),
    }
}

#[tauri::command]
async fn use_ledger(url: String) -> Result<String, String> {
    engine()?.use_ledger(url).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn go_live() -> Result<(), String> {
    engine()?.go_live().await.map_err(|e| e.to_string())
}

#[tauri::command]
fn notes() -> Result<Vec<NoteSummary>, String> {
    engine()?.notes().map_err(|e| e.to_string())
}

#[tauri::command]
fn new_note(title: String, text: String) -> Result<String, String> {
    engine()?
        .create_note(&title, &text)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn append(id: String, text: String) -> Result<(), String> {
    engine()?.append(&id, &text).map_err(|e| e.to_string())
}

/// The device to name in "Open Mirrorleaf on <name> to sync", once due.
#[tauri::command]
fn fallback_prompt() -> Option<String> {
    Engine::get()?.fallback_prompt()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            #[cfg(desktop)]
            {
                use tauri::Manager;
                let dir = app.path().app_data_dir()?;
                let engine = Engine::open(dir.to_string_lossy().into_owned(), "Mac".to_owned())?;
                tauri::async_runtime::spawn(async move {
                    let _ = engine.go_live().await;
                });
            }
            let _ = app;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            status,
            join,
            use_ledger,
            go_live,
            notes,
            new_note,
            append,
            fallback_prompt
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Mirrorleaf app");
}
