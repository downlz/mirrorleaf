//! `cargo run -p uniffi-bindgen -- generate --library <lib> --language kotlin --out-dir <dir>`
//! (the Android build runs it, see the app's `build.gradle.kts`).

fn main() {
    uniffi::uniffi_bindgen_main()
}
