//! The desktop agent (plan §5.7, M5). One process owns the engine. Closing the
//! window destroys the web view; the process stays in the menu bar (macOS) or
//! tray (Linux) and keeps syncing. It can start at login without a window (off
//! until the user turns it on in Settings), allows one instance, stores the last
//! edits when the window closes or the app quits (flush-on-close), and goes
//! offline while the Mac sleeps.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mirrorleaf_ffi::Engine;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{App, AppHandle, Emitter, Manager, RunEvent, Window, WindowEvent, Wry};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

use crate::window;

/// The argument the login item starts the app with: no window, menu bar only.
const BACKGROUND_ARG: &str = "--background";
/// The longest a quit waits for the last changes to go out and the ledger to
/// hear this device is going offline.
const QUIT_WAIT: Duration = Duration::from_secs(3);
/// How often a quit checks whether the window has closed.
const CLOSE_CHECK: Duration = Duration::from_millis(20);
/// The tray icon's ID.
const TRAY: &str = "main";

/// Set once the person chose Quit: the app looks gone, and a second launch
/// mustn't open a window in the process about to exit.
static QUITTING: AtomicBool = AtomicBool::new(false);
/// Told to the screen before its window closes, so it sends the edits it is
/// still holding and then calls `close_window`.
pub const WILL_CLOSE: &str = "will-close";
/// How long a closing window waits for the screen before closing anyway.
const CLOSE_WAIT: Duration = Duration::from_secs(1);

const OPEN: &str = "open";
const QUIT: &str = "quit";
const STATUS: &str = "status";

/// The tray menu's status line, kept to update it.
struct TrayStatus(MenuItem<Wry>);

pub fn plugins(builder: tauri::Builder<Wry>) -> tauri::Builder<Wry> {
    builder
        // Must come first: a second launch only shows the first one's window.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_window(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![BACKGROUND_ARG]),
        ))
        // The Save dialog for the recovery kit (M10).
        .plugin(tauri_plugin_dialog::init())
}

pub fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    init_logging();
    let dir = app.path().app_data_dir()?;
    mirrorleaf_ffi::set_app_version(app.package_info().version.to_string());
    let engine = Engine::open(dir.to_string_lossy().into_owned(), device_name())?;

    #[cfg(target_os = "macos")]
    app.set_menu(app_menu(app.handle())?)?;
    tray(app)?;
    #[cfg(target_os = "macos")]
    crate::power::watch_sleep();

    if std::env::args().any(|a| a == BACKGROUND_ARG) {
        // Started at login: menu bar only, until the user opens the window.
        #[cfg(target_os = "macos")]
        app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    } else {
        window::build_main(app.handle())?;
    }

    tauri::async_runtime::spawn(async move {
        if let Err(e) = engine.go_live().await {
            tracing::info!("live sync didn't start: {e}");
        }
    });
    Ok(())
}

pub fn on_window_event(window: &Window, event: &WindowEvent) {
    match event {
        // Flush-on-close: the screen sends what it holds, then the window goes.
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            ask_to_close(window.app_handle(), window.label());
        }
        WindowEvent::Focused(false) => {
            if let Some(engine) = Engine::get() {
                let _ = engine.flush_edits();
            }
        }
        // The last window is gone: leave the Dock and stay in the menu bar.
        WindowEvent::Destroyed => {
            #[cfg(target_os = "macos")]
            {
                let app = window.app_handle();
                if app.webview_windows().keys().all(|l| l == window.label()) {
                    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                }
            }
            let _ = window;
        }
        _ => {}
    }
}

pub fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        OPEN => show_window(app),
        QUIT => quit(app),
        _ => {}
    }
}

pub fn on_run_event(app: &AppHandle, event: &RunEvent) {
    match event {
        // Every window closed: keep running as the agent. An exit the app
        // asks for itself (Quit) carries a code and goes ahead.
        RunEvent::ExitRequested {
            code: None, api, ..
        } => api.prevent_exit(),
        // A click on the Dock icon while no window is open.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } => show_window(app),
        _ => {}
    }
    let _ = app;
}

/// Shows the main window, creating it if it was closed.
pub fn show_window(app: &AppHandle) {
    if QUITTING.load(Ordering::Acquire) {
        return;
    }
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(window) = app.get_webview_window(window::MAIN) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    match window::build_main(app) {
        Ok(window) => {
            let _ = window.set_focus();
        }
        Err(e) => tracing::warn!("couldn't open the window: {e}"),
    }
}

/// Asks the screen in window `label` to send its last edits and close; closes
/// the window anyway if it hasn't within `CLOSE_WAIT`.
fn ask_to_close(app: &AppHandle, label: &str) {
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    let _ = window.emit(WILL_CLOSE, ());
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(CLOSE_WAIT).await;
        close_window(&window);
    });
}

/// Closes a window for good, storing the editor's edits first. Called by the
/// screen once it has sent them, or after `CLOSE_WAIT`.
pub fn close_window(window: &tauri::WebviewWindow) {
    if let Some(engine) = Engine::get() {
        let _ = engine.close_note();
    }
    let _ = window.destroy();
}

/// Quits for real: the screen sends its last edits, live sync sends them on
/// and the ledger hears this device is going offline, then the app exits.
/// The window, the menu-bar icon and the Dock icon go at once; the rest takes
/// as long as it needs (usually a fraction of a second, at most about 4 s),
/// out of sight.
pub fn quit(app: &AppHandle) {
    if QUITTING.swap(true, Ordering::AcqRel) {
        return;
    }
    let started = Instant::now();
    if let Some(tray) = app.tray_by_id(TRAY) {
        let _ = tray.set_visible(false);
    }
    for window in app.webview_windows().values() {
        let _ = window.hide();
        ask_to_close(app, window.label());
    }
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // The screen closes its window as soon as it has sent its last edits
        // (or `ask_to_close` closes it after `CLOSE_WAIT`).
        while !app.webview_windows().is_empty() && started.elapsed() < CLOSE_WAIT {
            tokio::time::sleep(CLOSE_CHECK).await;
        }
        if let Some(engine) = Engine::get() {
            let _ = engine.flush_edits();
            let _ = tokio::time::timeout(QUIT_WAIT, engine.go_offline()).await;
        }
        tracing::info!("quit in {} ms", started.elapsed().as_millis());
        app.exit(0);
    });
}

/// Updates the menu bar's status line from the engine.
pub fn refresh_tray(app: &AppHandle) {
    let (Some(item), Some(engine)) = (app.try_state::<TrayStatus>(), Engine::get()) else {
        return;
    };
    let text = match engine.status() {
        Ok(s) if s.account.is_none() => "Not set up yet".to_owned(),
        Ok(s) if !s.live => "Sync paused".to_owned(),
        Ok(s) => match s.devices.iter().filter(|d| d.connection.is_some()).count() {
            0 => "Waiting for your other devices".to_owned(),
            1 => "Syncing with 1 device".to_owned(),
            n => format!("Syncing with {n} devices"),
        },
        Err(_) => return,
    };
    let _ = item.0.set_text(text);
}

pub fn autostart_enabled(app: &AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

pub fn set_autostart(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let launcher = app.autolaunch();
    let changed = if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    };
    changed.map_err(|e| e.to_string())
}

fn tray(app: &App) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, STATUS, "Starting…", false, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, OPEN, "Open Mirrorleaf", true, None::<&str>)?,
            &MenuItem::with_id(app, QUIT, "Quit Mirrorleaf", true, None::<&str>)?,
        ],
    )?;
    TrayIconBuilder::with_id(TRAY)
        .icon(tauri::include_image!("icons/tray.png"))
        .icon_as_template(true)
        .tooltip("Mirrorleaf")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .build(app)?;
    app.manage(TrayStatus(status));
    refresh_tray(app.handle());
    Ok(())
}

/// The macOS menu bar's menus. Undo and Redo are left out on purpose: as menu
/// items they would catch ⌘Z before the editor, whose own undo knows about
/// sync (TipTap's collaboration undo). Cut, copy, paste and select-all must be
/// menu items, or their shortcuts don't reach the web view.
#[cfg(target_os = "macos")]
fn app_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    use tauri::menu::Submenu;
    let separator = || PredefinedMenuItem::separator(app);
    let app_menu = Submenu::with_items(
        app,
        "Mirrorleaf",
        true,
        &[
            &PredefinedMenuItem::about(app, None, None)?,
            &separator()?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &separator()?,
            &MenuItem::with_id(app, QUIT, "Quit Mirrorleaf", true, Some("CmdOrCtrl+Q"))?,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    Menu::with_items(app, &[&app_menu, &edit, &window])
}

/// What other devices show for this one, from the computer's name.
fn device_name() -> String {
    #[cfg(target_os = "macos")]
    let name = std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned());
    #[cfg(target_os = "linux")]
    let name = std::fs::read_to_string("/etc/hostname").ok();
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").ok();
    name.map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Desktop".to_owned())
}

/// Logs to standard error; `RUST_LOG` overrides the default levels.
fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("warn,mirrorleaf_ffi=info,mirrorleaf_net=info,mirrorleaf_app_lib=info")
    });
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
