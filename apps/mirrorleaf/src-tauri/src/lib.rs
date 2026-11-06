//! mirrorleaf-app: the Tauri app (implementation plan §5.6–§5.7). The screen is
//! a Svelte app in `apps/mirrorleaf/src`, shared by the desktop, Android and
//! later tablets; it drives the engine through the commands in `commands`.
//!
//! On Android the Kotlin startup code opens the engine before the screen exists,
//! because a push-woken job may need it with no screen at all. On the desktop
//! the app opens it here, and the same process is the background agent.

mod commands;
#[cfg(desktop)]
mod desktop;
mod photos;
mod picker;
#[cfg(target_os = "macos")]
mod power;
mod startup;
mod window;

use mirrorleaf_ffi::Engine;
use tauri::{AppHandle, Emitter};

/// The event the screen listens to for refreshes (`UiEvent` as its payload).
const ENGINE_EVENT: &str = "engine";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(desktop)]
    let builder = desktop::plugins(builder)
        .on_menu_event(desktop::on_menu_event)
        .on_window_event(desktop::on_window_event);
    #[cfg(target_os = "android")]
    let builder = builder.plugin(picker::init());
    let app = builder
        .register_asynchronous_uri_scheme_protocol(photos::SCHEME, photos::handle)
        .setup(|app| {
            #[cfg(desktop)]
            desktop::setup(app)?;
            #[cfg(mobile)]
            window::build_main(app.handle())?;
            forward_engine_events(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::platform,
            commands::status,
            commands::create_account,
            commands::ticket,
            commands::join,
            commands::use_ledger,
            commands::fallback_prompt,
            commands::note_list,
            commands::create_note,
            commands::trash_note,
            commands::restore_note,
            commands::set_pinned,
            commands::move_note,
            commands::create_folder,
            commands::rename_folder,
            commands::delete_folder,
            commands::set_note_meta,
            commands::open_note,
            commands::close_note,
            commands::edit_note,
            commands::flush_edits,
            commands::close_window,
            commands::import_photo,
            commands::pick_photos,
            commands::take_pending_open,
            commands::startup_done,
            commands::autostart,
            commands::set_autostart,
        ])
        .build(tauri::generate_context!())
        .expect("error while starting the Mirrorleaf app");
    app.run(|app, event| {
        #[cfg(desktop)]
        desktop::on_run_event(app, &event);
        let _ = (app, event);
    });
}

/// Passes the engine's refresh events to the screen (and, on the desktop, to
/// the menu bar's status line).
fn forward_engine_events(app: &AppHandle) {
    let Some(engine) = Engine::get() else {
        tracing::warn!("the engine isn't open");
        return;
    };
    let app = app.clone();
    engine.set_ui_listener(move |event| {
        #[cfg(desktop)]
        if event == mirrorleaf_ffi::UiEvent::StatusChanged {
            desktop::refresh_tray(&app);
        }
        let _ = app.emit(ENGINE_EVENT, event);
    });
}
