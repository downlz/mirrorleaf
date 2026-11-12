//! The Tauri commands the screen calls (M5). Each one is a thin wrapper around
//! the engine (`mirrorleaf-ffi`), which owns the data and sync.
//!
//! Yjs updates travel as raw bytes, not JSON: `open_note` answers with the
//! note's state, other devices' updates arrive on the note's channel, and
//! `edit_note` and `import_photo` take their bytes as the request body
//! (plan §5.6).

use std::sync::Arc;

use mirrorleaf_ffi::{Engine, EngineStatus, NoteList, PhotoInfo};
use tauri::ipc::{Channel, InvokeBody, InvokeResponseBody, Request, Response};

/// The header carrying the note an `edit_note` update belongs to.
const NOTE_HEADER: &str = "x-mirrorleaf-note";

type Result<T> = std::result::Result<T, String>;

fn engine() -> Result<Arc<Engine>> {
    Engine::get().ok_or_else(|| "the engine isn't open".to_owned())
}

fn text<E: ToString>(error: E) -> String {
    error.to_string()
}

/// The bytes a request carries. On the desktop they arrive raw. On Android
/// the web view can't send a request body, so Tauri sends the call as JSON and
/// the bytes arrive as an array of numbers (M6).
fn bytes(request: &Request<'_>) -> Result<Vec<u8>> {
    match request.body() {
        InvokeBody::Raw(bytes) => Ok(bytes.clone()),
        InvokeBody::Json(serde_json::Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_u64()
                    .and_then(|n| u8::try_from(n).ok())
                    .ok_or_else(|| "expected bytes".to_owned())
            })
            .collect(),
        InvokeBody::Json(_) => Err("expected bytes".to_owned()),
    }
}

/// Starts live sync in the background, as opening the app does.
fn go_live(engine: Arc<Engine>) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = engine.go_live().await {
            tracing::warn!("live sync didn't start: {e}");
        }
    });
}

#[tauri::command]
pub fn platform() -> &'static str {
    std::env::consts::OS
}

#[tauri::command]
pub fn status() -> Result<EngineStatus> {
    engine()?.status().map_err(text)
}

#[tauri::command]
pub fn create_account() -> Result<String> {
    let engine = engine()?;
    let created = engine.create_account().map_err(text)?;
    go_live(engine);
    Ok(created)
}

#[tauri::command]
pub async fn use_ledger(url: String) -> Result<String> {
    engine()?.use_ledger(url).await.map_err(text)
}

// --- Adding a device (E1) ---

#[tauri::command]
pub fn set_device_name(name: String) -> Result<()> {
    engine()?.set_device_name(name).map_err(text)
}

/// Shows a QR code and a word code for a new device.
#[tauri::command]
pub async fn invite() -> Result<mirrorleaf_ffi::Invitation> {
    engine()?.invite().await.map_err(text)
}

#[tauri::command]
pub fn answer_pairing(allow: bool) -> Result<()> {
    engine()?.answer_pairing(allow).map_err(text)
}

#[tauri::command]
pub fn cancel_pairing() -> Result<()> {
    engine()?.cancel_pairing();
    Ok(())
}

#[tauri::command]
pub fn pairing_status() -> Result<mirrorleaf_ffi::PairingStatus> {
    Ok(engine()?.pairing_status())
}

/// Joins an account with a code from one of its devices.
#[tauri::command]
pub async fn join_with_code(code: String, ledger: Option<String>) -> Result<String> {
    engine()?.join_with_code(code, ledger).await.map_err(text)
}

/// Scans another device's QR code (Android). `None` if cancelled.
#[tauri::command]
pub async fn scan_code(app: tauri::AppHandle) -> Result<Option<String>> {
    crate::device::scan(&app).await
}

/// Whether notifications and nearby devices are allowed (always on the desktop).
#[tauri::command]
pub async fn permissions(app: tauri::AppHandle) -> Result<crate::device::Permissions> {
    crate::device::permissions(&app).await
}

/// Asks Android for "notifications" and/or "nearby", where not granted yet.
#[tauri::command]
pub async fn request_permissions(
    app: tauri::AppHandle,
    which: Vec<String>,
) -> Result<crate::device::Permissions> {
    let before = crate::device::permissions(&app).await?;
    let after = crate::device::request_permissions(&app, which).await?;
    // Android applies a new nearby-devices permission only to sockets opened
    // after it (M4 finding 1): start the network again (M12, E8).
    if after.nearby && !before.nearby {
        engine()?.restart_network().await.map_err(text)?;
    }
    Ok(after)
}

/// Battery restrictions, the phone's maker and "Seed while charging", for
/// Settings on Android (M12, E8). `None` elsewhere.
#[tauri::command]
pub async fn battery(app: tauri::AppHandle) -> Result<Option<crate::device::Battery>> {
    crate::device::battery(&app).await
}

/// Opens Android's own settings page for Mirrorleaf.
#[tauri::command]
pub async fn open_app_settings(app: tauri::AppHandle) -> Result<()> {
    crate::device::open_app_settings(&app).await
}

#[tauri::command]
pub async fn set_seeding(app: tauri::AppHandle, on: bool) -> Result<()> {
    crate::device::set_seeding(&app, on).await
}

// --- The account's lifecycle (M10: E2, E3) ---

/// The recovery kit waiting to be saved, if any.
#[tauri::command]
pub fn pending_kit() -> Result<Option<mirrorleaf_ffi::KitView>> {
    engine()?.pending_kit().map_err(text)
}

/// Saves the kit as "pdf" or "text" where the person chooses. `false` if
/// they cancelled.
#[tauri::command]
pub async fn save_kit(app: tauri::AppHandle, kind: String) -> Result<bool> {
    crate::files::save_kit(&app, &kind).await
}

/// The kit is saved or its words written down: forget it.
#[tauri::command]
pub fn finish_kit() -> Result<()> {
    engine()?.finish_kit().map_err(text)
}

/// Replaces the account key, leaving out `remove`, and makes a new kit.
#[tauri::command]
pub async fn replace_key(remove: Vec<String>) -> Result<()> {
    engine()?.replace_key(remove).await.map_err(text)
}

#[tauri::command]
pub async fn remove_device(id: String) -> Result<()> {
    engine()?.remove_device(id).await.map_err(text)
}

/// How many changes exist only on this device.
#[tauri::command]
pub fn only_here() -> Result<u64> {
    engine()?.only_here().map_err(text)
}

/// Signs out and erases this device, then starts the app afresh.
#[tauri::command]
pub async fn sign_out(app: tauri::AppHandle, force: bool) -> Result<()> {
    engine()?.sign_out(force).await.map_err(text)?;
    crate::files::restart(&app).await
}

/// Recovers the account with the kit's words or QR code.
#[tauri::command]
pub async fn recover(code: String, ledger: Option<String>) -> Result<String> {
    engine()?.recover(code, ledger).await.map_err(text)
}

// --- Backups (E9) ---

#[tauri::command]
pub fn backup_settings() -> Result<mirrorleaf_ffi::BackupSettings> {
    engine()?.backup_settings().map_err(text)
}

#[tauri::command]
pub fn set_backup_encryption(on: bool) -> Result<()> {
    engine()?.set_backup_encryption(on).map_err(text)
}

/// "Later" on the main screen's reminder.
#[tauri::command]
pub fn snooze_backup() -> Result<()> {
    engine()?.snooze_backup_reminder().map_err(text)
}

/// Writes a backup where the person chooses. `None` if they cancelled.
#[tauri::command]
pub async fn save_backup(app: tauri::AppHandle) -> Result<Option<mirrorleaf_ffi::BackupDone>> {
    crate::files::save_backup(&app).await
}

/// Lets the person pick a backup to restore. `None` if they cancelled.
#[tauri::command]
pub async fn pick_backup(app: tauri::AppHandle) -> Result<Option<crate::files::PickedBackup>> {
    crate::files::pick_backup(&app).await
}

/// Restores the backup picked; `words` are its own kit's, when it was made
/// with an earlier account key.
#[tauri::command]
pub async fn restore_backup(
    path: String,
    words: Option<String>,
) -> Result<mirrorleaf_ffi::BackupDone> {
    let restored = engine()?
        .restore_backup(path.clone(), words)
        .await
        .map_err(text);
    // A wrong kit can be tried again; anything else ends the restore.
    if restored.is_ok() {
        crate::files::forget_backup(&path);
    }
    restored
}

/// The person closed the restore without restoring.
#[tauri::command]
pub fn forget_backup(path: String) {
    crate::files::forget_backup(&path);
}

// --- The app lock (E4) ---

/// The lock's settings, and whether this device has an unlock it can use.
#[derive(serde::Serialize)]
pub struct LockView {
    #[serde(flatten)]
    settings: mirrorleaf_ffi::LockSettings,
    available: bool,
}

#[tauri::command]
pub async fn lock_settings(app: tauri::AppHandle) -> Result<LockView> {
    let settings = engine()?.lock_settings().map_err(text)?;
    let available = crate::auth::available(&app).await;
    Ok(LockView {
        settings,
        available,
    })
}

/// Changes the lock after the device's own unlock, so only its owner can.
/// `false` if they cancelled the unlock.
#[tauri::command]
pub async fn set_lock(app: tauri::AppHandle, enabled: bool, after_minutes: u32) -> Result<bool> {
    let engine = engine()?;
    if !authenticate(&app, &engine).await? {
        return Ok(false);
    }
    engine.set_lock(enabled, after_minutes).map_err(text)?;
    engine.unlocked();
    Ok(true)
}

/// Whether the screen must show the lock. Called as the screen starts.
#[tauri::command]
pub fn is_locked() -> Result<bool> {
    engine()?.screen_returned().map_err(text)
}

/// The screen went out of sight.
#[tauri::command]
pub fn screen_left() -> Result<()> {
    engine()?.screen_left();
    Ok(())
}

/// The screen is back in sight: whether it must show the lock.
#[tauri::command]
pub fn screen_returned() -> Result<bool> {
    engine()?.screen_returned().map_err(text)
}

/// Asks for the device's own unlock. `true` once unlocked.
#[tauri::command]
pub async fn unlock(app: tauri::AppHandle) -> Result<bool> {
    let engine = engine()?;
    if !crate::auth::available(&app).await {
        // The device's own lock was removed since the app lock was turned
        // on: anyone holding the device can already open it, so the app lock
        // goes too rather than shutting its owner out.
        tracing::info!("no device unlock any more: turning the app lock off");
        let minutes = engine.lock_settings().map_err(text)?.after_minutes;
        engine.set_lock(false, minutes).map_err(text)?;
        engine.unlocked();
        return Ok(true);
    }
    let passed = authenticate(&app, &engine).await?;
    if passed {
        engine.unlocked();
    }
    Ok(passed)
}

/// The device's own unlock, with the time away paused while it shows.
async fn authenticate(app: &tauri::AppHandle, engine: &Engine) -> Result<bool> {
    engine.authenticating(true);
    let passed = crate::auth::authenticate(app).await;
    engine.authenticating(false);
    passed
}

/// The device to name in "Open Mirrorleaf on <name> to sync", once due.
#[tauri::command]
pub fn fallback_prompt() -> Option<String> {
    Engine::get()?.fallback_prompt()
}

#[tauri::command]
pub fn note_list() -> Result<NoteList> {
    engine()?.note_list().map_err(text)
}

#[tauri::command]
pub fn create_note(folder: Option<String>, title: Option<String>) -> Result<String> {
    engine()?.create_note(folder, title).map_err(text)
}

#[tauri::command]
pub fn trash_note(id: String) -> Result<()> {
    engine()?.trash_note(&id).map_err(text)
}

#[tauri::command]
pub fn restore_note(id: String) -> Result<()> {
    engine()?.restore_note(&id).map_err(text)
}

#[tauri::command]
pub fn set_pinned(id: String, pinned: bool) -> Result<()> {
    engine()?.set_pinned(&id, pinned).map_err(text)
}

#[tauri::command]
pub fn move_note(id: String, folder: Option<String>) -> Result<()> {
    engine()?.move_note(&id, folder).map_err(text)
}

#[tauri::command]
pub fn create_folder(name: String, parent: Option<String>) -> Result<String> {
    engine()?
        .create_folder(&name, parent.as_deref())
        .map_err(text)
}

/// Moves a folder into another, or to the top level (M12).
#[tauri::command]
pub fn move_folder(id: String, parent: Option<String>) -> Result<()> {
    engine()?.move_folder(&id, parent.as_deref()).map_err(text)
}

#[tauri::command]
pub fn rename_folder(id: String, name: String) -> Result<()> {
    engine()?.rename_folder(&id, &name).map_err(text)
}

#[tauri::command]
pub fn delete_folder(id: String) -> Result<()> {
    engine()?.delete_folder(&id).map_err(text)
}

#[tauri::command]
pub fn set_note_meta(id: String, title: String, preview: String) -> Result<()> {
    engine()?.set_note_meta(&id, &title, &preview).map_err(text)
}

/// Opens a note: answers with its whole state as one Yjs update, then sends
/// other devices' updates to `updates` until another note is opened.
#[tauri::command]
pub fn open_note(id: String, updates: Channel<InvokeResponseBody>) -> Result<Response> {
    let state = engine()?
        .open_note(&id, move |update| {
            let _ = updates.send(InvokeResponseBody::Raw(update));
        })
        .map_err(text)?;
    Ok(Response::new(state))
}

#[tauri::command]
pub fn close_note() -> Result<()> {
    engine()?.close_note().map_err(text)
}

/// One Yjs update from the editor, as the raw body, for the note named in the
/// `x-mirrorleaf-note` header.
#[tauri::command]
pub fn edit_note(request: Request<'_>) -> Result<()> {
    let note = request
        .headers()
        .get(NOTE_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or("no note given")?
        .to_owned();
    engine()?.edit_note(&note, bytes(&request)?).map_err(text)
}

/// Android: picks photos with the system's photo picker and prepares them,
/// ready for the editor to place. Empty if the user cancelled.
#[tauri::command]
pub async fn pick_photos(app: tauri::AppHandle) -> Result<Vec<PhotoInfo>> {
    let paths = crate::picker::pick(&app).await?;
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    engine()?.import_photo_files(paths).await.map_err(text)
}

/// The note to open because it was made outside the screen (shared from
/// another app), once.
#[tauri::command]
pub fn take_pending_open() -> Option<String> {
    Engine::get()?.take_pending_open()
}

/// The screen is ready to use: logs the cold start once (`startup`).
#[tauri::command]
pub fn startup_done(what: String) {
    crate::startup::report(&what);
}

#[tauri::command]
pub fn flush_edits() -> Result<()> {
    engine()?.flush_edits().map_err(text)
}

/// A photo file's bytes as the raw body: prepared and staged, ready for the
/// editor to place.
#[tauri::command]
pub async fn import_photo(request: Request<'_>) -> Result<PhotoInfo> {
    let photo = bytes(&request)?;
    engine()?.import_photo(photo).await.map_err(text)
}

/// Notes matching every word of `query` in their title or text (M12, E5).
/// Runs off the main thread: the first search after an update reads every
/// note into the index once.
#[tauri::command]
pub async fn search_notes(query: String) -> Result<Vec<mirrorleaf_ffi::SearchResult>> {
    let engine = engine()?;
    tauri::async_runtime::spawn_blocking(move || engine.search_notes(&query))
        .await
        .map_err(text)?
        .map_err(text)
}

/// Whether this device finds the account's devices on the same network
/// without internet (M12, on by default).
#[tauri::command]
pub fn local_discovery() -> Result<bool> {
    Ok(engine()?.local_discovery_on())
}

#[tauri::command]
pub fn set_local_discovery(on: bool) -> Result<()> {
    engine()?.set_local_discovery(on).map_err(text)
}

/// Deletes a note for good, on every device.
#[tauri::command]
pub fn delete_note(id: String) -> Result<()> {
    engine()?.delete_note(&id).map_err(text)
}

/// Deletes every note in the trash for good. Returns how many.
#[tauri::command]
pub fn empty_trash() -> Result<u32> {
    engine()?.empty_trash().map_err(text)
}

/// A photo dropped on the window outside the editor (M12): kept in the app's
/// folder until `drop_into_note` takes it. Returns where.
#[tauri::command]
pub fn stage_dropped(request: Request<'_>) -> Result<String> {
    crate::files::stage_dropped(&bytes(&request)?)
}

/// Makes a note from what was dropped on the window outside the editor: the
/// text of dropped text files and the photos staged for it. The screen opens
/// it (`open_note` event).
#[tauri::command]
pub async fn drop_into_note(text: Option<String>, photos: Vec<String>) -> Result<String> {
    engine()?
        .share_into_note(None, text, photos)
        .await
        .map_err(|e| e.to_string())
}

/// The screen has sent its last edits after `will-close`: close its window.
#[tauri::command]
pub fn close_window(window: tauri::WebviewWindow) {
    #[cfg(desktop)]
    crate::desktop::close_window(&window);
    #[cfg(mobile)]
    let _ = window;
}

/// Whether the desktop agent starts at login. Always false on Android.
#[tauri::command]
pub fn autostart(app: tauri::AppHandle) -> bool {
    #[cfg(desktop)]
    return crate::desktop::autostart_enabled(&app);
    #[cfg(mobile)]
    {
        let _ = app;
        false
    }
}

#[tauri::command]
pub fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<()> {
    #[cfg(desktop)]
    return crate::desktop::set_autostart(&app, enabled);
    #[cfg(mobile)]
    {
        let _ = (app, enabled);
        Err("not available on this device".to_owned())
    }
}
