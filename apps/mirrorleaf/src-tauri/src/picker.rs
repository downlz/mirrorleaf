//! Android's photo picker (plan M6). A small Kotlin plugin
//! (`PhotoPickerPlugin.kt`) opens the system's picker and copies the chosen
//! photos into the app's cache; only their paths come back, and the engine
//! imports the files. Photo bytes never cross the web view: on Android, Tauri
//! sends a web view's data to Rust as JSON, where a 10 MB photo would become
//! hundreds of megabytes.

use tauri::{AppHandle, Runtime};

#[cfg(target_os = "android")]
struct Picker<R: Runtime>(tauri::plugin::PluginHandle<R>);

/// Registers the Kotlin plugin (Android only).
#[cfg(target_os = "android")]
pub fn init<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    use tauri::Manager;
    tauri::plugin::Builder::new("photo-picker")
        .setup(|app, api| {
            let handle = api.register_android_plugin("app.mirrorleaf", "PhotoPickerPlugin")?;
            app.manage(Picker(handle));
            Ok(())
        })
        .build()
}

#[cfg(target_os = "android")]
#[derive(serde::Deserialize)]
struct Picked {
    paths: Vec<String>,
}

/// Opens the system's photo picker. Returns the paths of copies of the photos
/// chosen; none if the user cancelled.
pub async fn pick<R: Runtime>(app: &AppHandle<R>) -> Result<Vec<String>, String> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        let picker = app.state::<Picker<R>>();
        let picked: Picked = picker
            .0
            .run_mobile_plugin_async("pick", ())
            .await
            .map_err(|e| e.to_string())?;
        Ok(picked.paths)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("the system photo picker is only used on Android".to_owned())
    }
}
