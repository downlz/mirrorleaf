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
| Android APK (arm64, debug) | in `apps/mirrorleaf/src-tauri`: `cargo tauri android build --debug --target aarch64 --apk --ci` |
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
