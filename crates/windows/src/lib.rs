//! Windows-only code for the desktop app (the Windows app, owner's request of
//! 15 November 2026): what the Mac gets from the Keychain, Touch ID, ImageIO
//! and its sleep notifications. Empty on other platforms.
//!
//! It depends only on Microsoft's `windows` bindings, which Tauri already
//! brings in, and has no C code, so it can be checked from a Mac with
//! `cargo check --target x86_64-pc-windows-msvc -p mirrorleaf-windows`.

#![cfg(windows)]

pub mod dpapi;
pub mod hello;
pub mod imaging;
pub mod power;
