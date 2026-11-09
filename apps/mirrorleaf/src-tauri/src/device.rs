//! Android's device features for adding a device (E1): scanning the other
//! device's QR code (`QrScannerPlugin.kt`, Google's code scanner, no camera
//! permission) and the two permissions sync uses (`PermissionsPlugin.kt`),
//! asked for when the screen needs them rather than at every start.

use tauri::{AppHandle, Runtime};

#[cfg(target_os = "android")]
struct Plugins<R: Runtime> {
    scanner: tauri::plugin::PluginHandle<R>,
    permissions: tauri::plugin::PluginHandle<R>,
}

/// Registers the Kotlin plugins (Android only).
#[cfg(target_os = "android")]
pub fn init<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    use tauri::Manager;
    tauri::plugin::Builder::new("device")
        .setup(|app, api| {
            let scanner = api.register_android_plugin("app.mirrorleaf", "QrScannerPlugin")?;
            let permissions = api.register_android_plugin("app.mirrorleaf", "PermissionsPlugin")?;
            app.manage(Plugins {
                scanner,
                permissions,
            });
            Ok(())
        })
        .build()
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
            .state::<Plugins<R>>()
            .scanner
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
        app.state::<Plugins<R>>()
            .permissions
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
        app.state::<Plugins<R>>()
            .permissions
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
