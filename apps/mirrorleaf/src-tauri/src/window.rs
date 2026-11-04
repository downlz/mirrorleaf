//! The main window. It is not created at startup (`"create": false`) so that
//! the desktop agent can start at login without one; the app builds it here
//! instead, from the same configuration, on every platform.

use tauri::{AppHandle, WebviewWindow, WebviewWindowBuilder};

pub const MAIN: &str = "main";

pub fn build_main(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == MAIN)
        .cloned()
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    WebviewWindowBuilder::from_config(app, &config)?.build()
}
