fn main() {
    // Android release builds (M7): the Gradle build generates the engine's Kotlin bindings
    // from the uniffi metadata symbols in the library's symbol table, so Cargo keeps that
    // table there (`CARGO_PROFILE_RELEASE_STRIP=debuginfo`, set by gen/android's BuildTask.kt
    // and CI). The table would otherwise add about 10 MB: keep only its global symbols
    // (a few hundred, the metadata among them) and drop the local ones. Tauri's template
    // keeps symbols in every APK (its `keepDebugSymbols`), so this is what ships.
    let android = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android");
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    if android && release {
        println!("cargo:rustc-link-arg-cdylib=-Wl,--discard-all");
    }
    tauri_build::build()
}
