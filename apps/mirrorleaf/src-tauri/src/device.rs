//! Android's device features: scanning the other device's QR code
//! (`QrScannerPlugin.kt`, Google's code scanner, no camera permission), the
//! two permissions sync uses (`PermissionsPlugin.kt`), asked for when the
//! screen needs them rather than at every start (E1), and saving the recovery
//! kit (`DocumentsPlugin.kt`, M10).

use tauri::{AppHandle, Runtime};

/// The Kotlin plugins' handles, one Tauri plugin each: under one Tauri
/// plugin, the last Kotlin plugin registered took every call (M10 finding:
/// the scanner's calls went to the permissions plugin).
#[cfg(target_os = "android")]
struct Scanner<R: Runtime>(tauri::plugin::PluginHandle<R>);
#[cfg(target_os = "android")]
struct PermissionsHandle<R: Runtime>(tauri::plugin::PluginHandle<R>);
/// Saving the recovery kit and restarting the app (M10, `DocumentsPlugin.kt`).
#[cfg(target_os = "android")]
struct Documents<R: Runtime>(tauri::plugin::PluginHandle<R>);
/// The app lock's unlock (M11, `LockPlugin.kt`).
#[cfg(target_os = "android")]
struct Lock<R: Runtime>(tauri::plugin::PluginHandle<R>);

/// Registers the Kotlin plugins (Android only).
#[cfg(target_os = "android")]
pub fn init<R: Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    use tauri::Manager;
    builder
        .plugin(
            tauri::plugin::Builder::<R>::new("qr-scanner")
                .setup(|app, api| {
                    let handle =
                        api.register_android_plugin("app.mirrorleaf", "QrScannerPlugin")?;
                    app.manage(Scanner(handle));
                    Ok(())
                })
                .build(),
        )
        .plugin(
            tauri::plugin::Builder::<R>::new("permissions")
                .setup(|app, api| {
                    let handle =
                        api.register_android_plugin("app.mirrorleaf", "PermissionsPlugin")?;
                    app.manage(PermissionsHandle(handle));
                    Ok(())
                })
                .build(),
        )
        .plugin(
            tauri::plugin::Builder::<R>::new("lock")
                .setup(|app, api| {
                    let handle = api.register_android_plugin("app.mirrorleaf", "LockPlugin")?;
                    app.manage(Lock(handle));
                    Ok(())
                })
                .build(),
        )
        .plugin(
            tauri::plugin::Builder::<R>::new("documents")
                .setup(|app, api| {
                    let handle =
                        api.register_android_plugin("app.mirrorleaf", "DocumentsPlugin")?;
                    app.manage(Documents(handle));
                    Ok(())
                })
                .build(),
        )
}

/// Whether sync's two permissions are granted. Where Android has no such
/// permission (older versions, or the desktop), it counts as granted.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct Permissions {
    pub notifications: bool,
    pub nearby: bool,
}

#[cfg(target_os = "android")]
#[derive(serde::Deserialize)]
struct Scanned {
    text: String,
}

/// Opens the code scanner. `None` if the person cancelled.
pub async fn scan<R: Runtime>(app: &AppHandle<R>) -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        let scanned: Scanned = app
            .state::<Scanner<R>>()
            .0
            .run_mobile_plugin_async("scan", ())
            .await
            .map_err(|e| e.to_string())?;
        Ok(Some(scanned.text).filter(|t| !t.is_empty()))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("scanning a code is only offered on Android; type the words instead".to_owned())
    }
}

pub async fn permissions<R: Runtime>(app: &AppHandle<R>) -> Result<Permissions, String> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        app.state::<PermissionsHandle<R>>()
            .0
            .run_mobile_plugin_async("status", ())
            .await
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(Permissions {
            notifications: true,
            nearby: true,
        })
    }
}

/// Asks for `which` ("notifications", "nearby") where not granted yet, and
/// says what is granted afterwards.
pub async fn request_permissions<R: Runtime>(
    app: &AppHandle<R>,
    which: Vec<String>,
) -> Result<Permissions, String> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        #[derive(serde::Serialize)]
        struct Args {
            which: Vec<String>,
        }
        app.state::<PermissionsHandle<R>>()
            .0
            .run_mobile_plugin_async("request", Args { which })
            .await
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = which;
        permissions(app).await
    }
}

/// Copies the file at `path` to where the person picks with Android's "Save
/// to" picker, then deletes it. `false` if they cancelled.
#[cfg(target_os = "android")]
pub async fn save_document<R: Runtime>(
    app: &AppHandle<R>,
    path: String,
    name: String,
    mime: String,
) -> Result<bool, String> {
    use tauri::Manager;
    #[derive(serde::Serialize)]
    struct Args {
        path: String,
        name: String,
        mime: String,
    }
    #[derive(serde::Deserialize)]
    struct Saved {
        saved: bool,
    }
    let saved: Saved = app
        .state::<Documents<R>>()
        .0
        .run_mobile_plugin_async("save", Args { path, name, mime })
        .await
        .map_err(|e| e.to_string())?;
    Ok(saved.saved)
}

/// Starts the app again in a fresh process (after signing out).
#[cfg(target_os = "android")]
pub async fn restart<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    use tauri::Manager;
    app.state::<Documents<R>>()
        .0
        .run_mobile_plugin_async::<()>("restart", ())
        .await
        .map_err(|e| e.to_string())
}

/// Lets the person pick a file and copies it to `path` (M11: a backup to
/// restore). `false` if they cancelled.
#[cfg(target_os = "android")]
pub async fn open_document<R: Runtime>(app: &AppHandle<R>, path: String) -> Result<bool, String> {
    use tauri::Manager;
    #[derive(serde::Serialize)]
    struct Args {
        path: String,
    }
    #[derive(serde::Deserialize)]
    struct Opened {
        opened: bool,
    }
    let opened: Opened = app
        .state::<Documents<R>>()
        .0
        .run_mobile_plugin_async("open", Args { path })
        .await
        .map_err(|e| e.to_string())?;
    Ok(opened.opened)
}

/// Whether the phone has a screen lock (M11).
#[cfg(target_os = "android")]
pub async fn lock_available<R: Runtime>(app: &AppHandle<R>) -> bool {
    use tauri::Manager;
    #[derive(serde::Deserialize)]
    struct Available {
        available: bool,
    }
    app.state::<Lock<R>>()
        .0
        .run_mobile_plugin_async::<Available>("available", ())
        .await
        .map(|a| a.available)
        .unwrap_or(false)
}

/// The phone's own unlock (M11). `true` once passed.
#[cfg(target_os = "android")]
pub async fn authenticate<R: Runtime>(app: &AppHandle<R>) -> Result<bool, String> {
    use tauri::Manager;
    #[derive(serde::Deserialize)]
    struct Unlocked {
        unlocked: bool,
    }
    app.state::<Lock<R>>()
        .0
        .run_mobile_plugin_async::<Unlocked>("authenticate", ())
        .await
        .map(|u| u.unlocked)
        .map_err(|e| e.to_string())
}
