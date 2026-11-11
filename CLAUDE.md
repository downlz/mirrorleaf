# Mirrorleaf: notes for Claude Code

Mirrorleaf keeps notes only on the user's own devices and syncs them device to device. Milestones and design are in `docs/implementation-plan.md`; decisions are in `docs/adr/`.

## Rules

- **Never commit or push.** The owner makes every commit by hand. Leave changes unstaged and say when work is ready to commit.
- **Docs need approval.** Don't edit `docs/implementation-plan.md` without the owner's approval; list the exact changes first.
- British spelling in docs and comments.
- Keep all Iroh use inside `mirrorleaf-net` (plan §9).
- The Rust core, the network layer and the ledger are the real foundation. The notes screen (M5) is kept and shared by the desktop, Android and later tablets: keep it adaptive (`layout-wide`, `layout-medium`, `layout-narrow`), with touch-sized targets and no hover-only controls (owner, 4 Nov 2026).

## Commands

| Task | Command |
|---|---|
| Rust format | `cargo fmt --all` |
| Rust lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Rust tests | `cargo test --workspace` (includes the convergence test and the Iroh loopback tests; no internet needed) |
| Rust dependency check | `cargo deny check` |
| Install JS dependencies | `pnpm install` |
| Lint / format | `pnpm lint`, `pnpm format` (CI runs `pnpm format:check`) |
| Types / tests | `pnpm -r typecheck`, `pnpm -r test` (the editor's tests include the Yjs ↔ yrs fixture check; `UPDATE_FIXTURES=1` rewrites the fixtures after a deliberate change) |
| Mac app | in `apps/mirrorleaf/src-tauri`: `cargo tauri dev`; a release `.app` with `cargo tauri build --bundles app` (slow; CI builds the `.dmg`) |
| Android APK (arm64, debug) | in `apps/mirrorleaf/src-tauri`: `cargo tauri android build --debug --target aarch64 --apk --ci`. Prefix `MIRRORLEAF_DEFAULT_LEDGER=http://192.168.1.13:8080` so a fresh phone can look up word codes on the Mac's ledger (M9) |
| Android APK (arm64, release, for measuring) | in `apps/mirrorleaf/src-tauri`: `CARGO_PROFILE_RELEASE_STRIP=debuginfo cargo tauri android build --target aarch64 --apk --ci` (the bindings step needs the library's symbol table; `build.rs` keeps only its global symbols). The APK is unsigned: sign it with the debug key (`apksigner`, `~/.android/debug.keystore`) to install it over a debug build and keep the app's data. Release builds aren't debuggable, so `tools/mem-trace.py` and the web view debugger need the debug build |
| Cold start | the app logs `cold start: N ms from process start to …` once per run (`startup.rs`); on the Mac run the binary in `Mirrorleaf.app/Contents/MacOS` from a terminal to see it |
| Rust lint for Android | `cargo clippy --target aarch64-linux-android -p mirrorleaf-ffi -p mirrorleaf-app -- -D warnings`, with `CC_aarch64_linux_android` and `AR_aarch64_linux_android` pointing at the NDK's `aarch64-linux-android24-clang` and `llvm-ar` |
| Phone logs | `adb logcat -s mirrorleaf` (the engine logs there; the push-woken job has no screen) |

## Android (M4, ADR-005)

- One engine per process (`crates/ffi`). On Android, `MirrorleafApp.onCreate` starts it in every process, before the screen or a push-woken job; Tauri commands and the Kotlin code share it.
- The Kotlin bindings are generated during the Gradle build into `.../java/app/mirrorleaf/generated/` and are not committed.
- Tauri's template targets Java 8: Kotlin helpers compiled for Java 11 (such as WorkManager's `OneTimeWorkRequestBuilder`) don't compile; use the plain builders.
- `vendor/ndk-context` is a patched copy that allows a second initialisation (owner's decision). Don't remove the `[patch.crates-io]` entry.
- The Android build uses the Gradle and Kotlin versions Tauri's template generated (owner's decision); don't upgrade them without asking.

## Android screen (M6, ADR-007)

- On Android, Tauri sends every call from the screen as JSON (the WebView can't send a request body), so bytes arrive as arrays of numbers: commands taking bytes must accept both (`commands::bytes`). Never send photos or other large data through the web view on Android: the photo button uses `PhotoPickerPlugin` (Kotlin), which hands Rust file paths.
- HEIC and AVIF are decoded by Android's `ImageDecoder` through the engine's `PhotoDecoder` callback (`PhotoDecoding.kt`), set in `MirrorleafApp.onCreate`.
- `MainActivity` pads the content view by the system bars, cutout and keyboard; don't rely on CSS `safe-area-inset` on Android.
- The back gesture closes the note sheet and Settings through browser history (`AppState.onPopState`); open and close them with `select`/`back` and `openSettings`/`closeSettings`, never by setting the state directly.
- Share → Mirrorleaf: `MainActivity.receiveShare` → `Engine::share_into_note` → the screen opens the note (`open_note` event, `take_pending_open`).
- Android keyboards: ProseMirror on Chrome Android reads Enter from the text change and simulates keys after the keyboard's input (`readDOMChange`, the `deleteContentBackward` workaround). Anything that changes the note in response to the keyboard must allow for that; see the slash menu's `beforeinput` handling in `packages/editor/src/slash.ts`.
- Drive the phone from the Mac with `adb exec-out screencap -p`, `adb shell input tap|text|keyevent` and `adb shell am start -a android.intent.action.SEND …`. Debug builds expose the web view's debugger: `adb forward tcp:9333 localabstract:webview_devtools_remote_<pid>`, then the DevTools protocol on `localhost:9333`. Before tapping, check the app is in front (`dumpsys window | grep mCurrentFocus`): the owner may be using the phone.

## Desktop app and screen (M5, ADR-006)

- The screen is Svelte 5 in `apps/mirrorleaf/src`; the editor (TipTap 3, Yjs, the bridge to Rust) is `packages/editor`. Yjs updates and photo files travel as raw bytes (`open_note`, `edit_note`, `import_photo`), never JSON.
- The engine's screen API is `crates/ffi/src/engine/screen.rs`; the Tauri commands are thin wrappers in `apps/mirrorleaf/src-tauri/src/commands.rs`.
- The main window is built by the app (`"create": false`), so the agent can start at login without one. Closing it asks the screen to send its last edits (`will-close`, then `close_window`).
- Photos show through the `mirrorleaf-blob` protocol (`src-tauri/src/photos.rs`); notes keep `mirrorleaf-blob://<full>?thumb=<thumbnail>`.
- The macOS menu has no Undo or Redo items on purpose: they would catch ⌘Z before the editor's own undo.
- Without screen access, check layouts with a headless browser and a stand-in for `window.__TAURI_INTERNALS__`; Brave's headless mode hangs here, Firefox's `--screenshot` works.

## Working on the 8 GB Mac

- `.cargo/config.toml` caps Cargo at 4 jobs, and dependencies build without debug info.
- Use a physical Android phone, never the emulator (from M4).
- Run `./gradlew --stop` (in `apps/mirrorleaf/src-tauri/gen/android`) before Docker builds or long Rust builds.
- Release builds, all-CPU Android builds and size checks run in CI.

## Versions and dependencies

- Node 24.22 (`.nvmrc`) and pnpm 12.16.1 (`packageManager`).
- pnpm installs only releases at least 7 days old. A dependency that needs an install script must be added to `allowBuilds` in `pnpm-workspace.yaml`, and that needs the owner's approval.
- TypeScript stays on 6.0.x until typescript-eslint supports 7. Yjs stays on 13 and Tauri on 2. iroh-blobs is pre-1.0: pin it exactly.
- Rust has no automatic 7-day rule: pick versions at least 7 days old by hand, and check new `Cargo.lock` entries the same way. Iroh and yrs are pinned exactly (`=`).
- New Rust dependencies must pass `deny.toml` (permissive licences plus MPL-2.0, checked only for the platforms we build). Any advisory exception needs a reason naming the crate that pulls it in.
- The change log is the source of truth: sync moves log rows and never loads Yjs documents. Keep it that way, so the Android background job stays small.
- Formatters stay out of `docs/` and Markdown files.

## Identity and pairing (M9, ADR-009)

- Sync protocol 2 runs on the ALPN `mirrorleaf/sync/2`: builds before M9 (0.2.0) can't sync with M9 builds (0.2.5), so update every device together. One ALPN per protocol version; E11 can serve several.
- Pairing is `crates/net/src/pair.rs` (ALPN `mirrorleaf/pair/1`, SPAKE2, one attempt per code), driven by the engine's `engine/pairing.rs` and the screen's `Welcome`, `JoinFlow` and `AddDevice` components. The CLI's `pair invite` and `pair join` exercise it without a screen.
- The device list is version 2 (`device_list_v2` in `meta`); a Phase 0 list is converted on first open by any device holding the account key. Removed devices never come back: a removed device starts again with a new key.
- A device's name can change only before it joins or starts an account: afterwards it is in the signed list.
- Android asks for notifications and nearby devices in the joining sheet and in Settings (`PermissionsPlugin.kt`), never at app start. QR codes are scanned by `QrScannerPlugin.kt` (Google's code scanner, no camera permission).
- Word codes need the ledger's pairing table (ledger migration 2): rebuild the ledger image after pulling M9.

## Recovery kit and device lifecycle (M10, ADR-010)

- The account key is derived from the 128-bit recovery secret (`AccountKey::from_recovery`, a fixed BLAKE3 context pinned by a test): never change the derivation. A new kit always means a new account key and account ID.
- Sync protocol 3 (`mirrorleaf/sync/3`) adds `Handover`: a device with the replaced key hands the new one to a peer still on the old key, if the peer is on the new list (`session.rs`). M9 builds (0.2.5) can't sync with M10 builds (0.3.0).
- Replacing the key moves the ledger first (`POST /v1/accounts/rekey`, ledger migration 3 `account_moves`); the old key can never register again. Rebuild the ledger after pulling M10.
- The device list travels only in `Hello`, so live sync closes and redials every connection when the list or the account changes (`node.rs`, `DevicesChanged`).
- A removed device knows it (`Account::removed`), stops using the ledger (`SyncNode::ledger` returns `None`) and shows the removed page.
- Signing out erases at the next start (`erase-on-start`: key, database, photo store only), and the app restarts itself (desktop: `AppHandle::restart`; Android: `DocumentsPlugin.restart`).
- Each Kotlin plugin is registered as its own Tauri plugin (`device.rs`): under one Tauri plugin, the last one registered takes every call.
- The kit's PDF and text are made by `mirrorleaf-net::kit`; Android saves files through `DocumentsPlugin.kt` (the engine writes into its own folder, Kotlin copies to the place picked), never through the web view.

## Protection (M11, ADR-011)

- The database is always SQLCipher-encrypted (`crates/core/src/encryption.rs`), with a raw 256-bit key: the Mac Keychain (`desktop.rs`, service `app.mirrorleaf`), Android Keystore wrapping (`DatabaseKey.kt`, `database.key.wrapped`), or a `database.key` file for CLI and Linux devices. The device secret lives in `meta`, not in `device.key`. A plain database is converted on first open (`encrypt_in_place`); never open the owner's real folders with an older build afterwards.
- Never point `mirrorleaf-cli` at the Mac app's folder: its key is in the Keychain (the CLI refuses). To inspect the owner's data, copy the folder and test on the copy.
- The Mac app must be signed with the same identity each build (`APPLE_SIGNING_IDENTITY="Mirrorleaf Local Signing"`), or the Keychain asks after every rebuild.
- Android builds compile OpenSSL from source for SQLCipher; `.cargo/config.toml` sets the Android targets' `RANLIB` to `tools/ndk-ranlib` (needs `NDK_HOME`). Android's own backup of the app's data is off (`allowBackup="false"`).
- Backups are `crates/net/src/backup.rs` (format) and `SyncNode::write_backup` / `read_backup`; the engine's side and the app lock are `crates/ffi/src/engine/protection.rs`. Restoring merges like sync and replaces nothing.
- The app lock covers the screen only; sync carries on. The engine keeps the lock state; the screen reports `screen_left` / `screen_returned` (`visibilitychange`), the Mac app reports the window closing and sleep. The device's own unlock is `auth.rs` (LocalAuthentication on the Mac, `LockPlugin.kt` on Android).
- `deny.toml` ignores RUSTSEC-2026-0459 until a `libsqlite3-sys` release bundles `sqlcipher-src` 420 or later; then upgrade rusqlite and remove the ignore.
