//! The desktop agent (plan §5.7, M5). One process owns the engine. Closing the
//! window destroys the web view; the process stays in the menu bar (macOS) or
//! the tray (Windows, Linux) and keeps syncing. It can start at login without a
//! window (off until the user turns it on in Settings), allows one instance,
//! stores the last edits when the window closes or the app quits
//! (flush-on-close), and goes offline while the computer sleeps.

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
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            show_window(app);
            // Windows and Linux open files ("Open with Mirrorleaf") by
            // starting the app with their paths: a note is made from them.
            open_files(file_args(args.into_iter().skip(1)));
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![BACKGROUND_ARG]),
        ))
        // The Save dialog for the recovery kit (M10).
        .plugin(tauri_plugin_dialog::init())
}

pub fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    // On Windows, in the local (not roaming) application data: the database
    // and its key belong to this PC. Elsewhere the two are the same folder.
    #[cfg(windows)]
    let dir = app.path().app_local_data_dir()?;
    #[cfg(not(windows))]
    let dir = app.path().app_data_dir()?;
    init_logging(&dir);
    mirrorleaf_ffi::set_app_version(app.package_info().version.to_string());
    let data_dir = dir.to_string_lossy().into_owned();
    let engine = Engine::open(data_dir.clone(), device_name(), database_key(&data_dir)?)?;

    #[cfg(target_os = "macos")]
    app.set_menu(app_menu(app.handle())?)?;
    tray(app)?;
    #[cfg(any(target_os = "macos", windows))]
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
    // Started by "Open with Mirrorleaf" on Windows or Linux.
    open_files(file_args(std::env::args().skip(1)));
    Ok(())
}

/// The arguments that name existing files (not `--background`).
fn file_args(args: impl Iterator<Item = String>) -> Vec<std::path::PathBuf> {
    args.filter(|a| !a.starts_with("--"))
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_file())
        .collect()
}

/// Makes a note from files opened with Mirrorleaf, as a drop on the Dock icon
/// does on the Mac (M12).
fn open_files(paths: Vec<std::path::PathBuf>) {
    if paths.is_empty() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::files::note_from_paths(paths).await {
            tracing::warn!("couldn't make a note from the files: {e}");
        }
    });
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
        // The app lock's time away starts (E4).
        WindowEvent::Destroyed => {
            if let Some(engine) = Engine::get() {
                engine.screen_left();
            }
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
        // Files dropped on the Dock icon (or opened with Mirrorleaf) become a
        // note (M12, E5: instead of a share extension).
        #[cfg(target_os = "macos")]
        RunEvent::Opened { urls } => {
            let paths: Vec<_> = urls.iter().filter_map(|u| u.to_file_path().ok()).collect();
            if !paths.is_empty() {
                show_window(app);
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::files::note_from_paths(paths).await {
                        tracing::warn!("couldn't make a note from the files: {e}");
                    }
                });
            }
        }
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

/// Closes a window as its close button does (Ctrl+W on Windows and Linux).
pub fn request_close(window: &tauri::WebviewWindow) {
    ask_to_close(window.app_handle(), window.label());
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
    let builder = TrayIconBuilder::with_id(TRAY)
        .tooltip("Mirrorleaf")
        .menu(&menu);
    // The menu bar's icon is a black outline macOS tints; Windows' tray shows
    // the app's colour icon, opens the window on a click and the menu on a
    // right-click, as Windows apps do.
    #[cfg(not(windows))]
    let builder = builder
        .icon(tauri::include_image!("icons/tray.png"))
        .icon_as_template(true)
        .show_menu_on_left_click(true);
    #[cfg(windows)]
    let builder = builder
        .icon(tauri::include_image!("icons/32x32.png"))
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_window(tray.app_handle());
            }
        });
    builder.build(app)?;
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

/// The Keychain item holding the database key (M11, E4).
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "app.mirrorleaf";
#[cfg(target_os = "macos")]
const KEYCHAIN_ACCOUNT: &str = "database-key";

/// The database's key: in the login Keychain on the Mac, made on first use.
/// Builds signed with the same certificate read it without asking; an
/// unsigned (ad-hoc) build counts as a new app each time, so macOS asks once
/// per build (ADR-011).
#[cfg(target_os = "macos")]
fn database_key(data_dir: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use security_framework::passwords::{get_generic_password, set_generic_password};
    // errSecItemNotFound
    const NOT_FOUND: i32 = -25300;
    match get_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
        Ok(key) if key.len() == 32 => Ok(key),
        Ok(_) => Err("the database key in the Keychain is damaged".into()),
        Err(e) if e.code() == NOT_FOUND => {
            // A CLI device folder's key file (the CLI converted this folder
            // before the app did): adopt it, so the database still opens.
            let file = std::path::Path::new(data_dir).join(mirrorleaf_ffi::DB_KEY_FILE);
            let (key, from_file) = match std::fs::read(&file) {
                Ok(key) if key.len() == 32 => (key, true),
                _ => (mirrorleaf_ffi::new_database_key(), false),
            };
            set_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, &key)?;
            if from_file {
                std::fs::remove_file(&file)?;
                tracing::info!("moved the database key from its file into the Keychain");
            } else {
                tracing::info!("made the database key and kept it in the Keychain");
            }
            Ok(key)
        }
        Err(e) => Err(format!("couldn't read the database key from the Keychain: {e}").into()),
    }
}

/// The database's key on Windows: kept in `database.key.dpapi`, encrypted
/// with Windows' Data Protection API for this Windows user, the Keychain's
/// role on the Mac. Made on first use; a plain key file (the CLI's) is moved
/// under DPAPI.
#[cfg(windows)]
fn database_key(data_dir: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use mirrorleaf_windows::dpapi;
    const PROTECTED: &str = "database.key.dpapi";
    let dir = std::path::Path::new(data_dir);
    let protected = dir.join(PROTECTED);
    match std::fs::read(&protected) {
        Ok(bytes) => {
            let key = dpapi::unprotect(&bytes)
                .map_err(|e| format!("Windows couldn't unlock the database key: {e}"))?;
            return if key.len() == 32 {
                Ok(key)
            } else {
                Err("the database key is damaged".into())
            };
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        Err(_) => {}
    }
    let file = dir.join(mirrorleaf_ffi::DB_KEY_FILE);
    let (key, from_file) = match std::fs::read(&file) {
        Ok(key) if key.len() == 32 => (key, true),
        _ => (mirrorleaf_ffi::new_database_key(), false),
    };
    std::fs::create_dir_all(dir)?;
    // Written beside it, then renamed: never half a key.
    let partial = dir.join(format!("{PROTECTED}.new"));
    std::fs::write(&partial, dpapi::protect(&key)?)?;
    std::fs::rename(&partial, &protected)?;
    if from_file {
        std::fs::remove_file(&file)?;
        tracing::info!("moved the database key under Windows' protection");
    } else {
        tracing::info!("made the database key, protected for this Windows user");
    }
    Ok(key)
}

/// Linux (not a release target): a private key file beside the database.
#[cfg(target_os = "linux")]
fn database_key(data_dir: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(mirrorleaf_ffi::database_key_file(data_dir)?)
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

/// Logs to standard error; `RUST_LOG` overrides the default levels. Windows
/// apps have no console, so there the log goes to `logs\mirrorleaf.log` in
/// the app's folder, started afresh once it passes 5 MB.
fn init_logging(dir: &std::path::Path) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("warn,mirrorleaf_ffi=info,mirrorleaf_net=info,mirrorleaf_app_lib=info")
    });
    #[cfg(windows)]
    {
        let logs = dir.join("logs");
        let path = logs.join("mirrorleaf.log");
        let file = std::fs::create_dir_all(&logs).and_then(|()| {
            let fresh = std::fs::metadata(&path).is_ok_and(|m| m.len() > 5 * 1024 * 1024);
            std::fs::OpenOptions::new()
                .create(true)
                .append(!fresh)
                .write(true)
                .truncate(fresh)
                .open(&path)
        });
        if let Ok(file) = file {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file))
                .try_init();
            return;
        }
    }
    let _ = dir;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
