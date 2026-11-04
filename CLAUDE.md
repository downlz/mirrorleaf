# Mirrorleaf: notes for Claude Code

Mirrorleaf keeps notes only on the user's own devices and syncs them device to device. Milestones and design are in `docs/implementation-plan.md`; decisions are in `docs/adr/`.

## Rules

- **Never commit or push.** The owner makes every commit by hand. Leave changes unstaged and say when work is ready to commit.
- **Docs need approval.** Don't edit `docs/implementation-plan.md` without the owner's approval; list the exact changes first.
- British spelling in docs and comments.
- Keep all Iroh use inside `mirrorleaf-net` (plan §9).
- The Phase 0 UI is throwaway. The Rust core, the network layer and the ledger are the real foundation.

## Commands

| Task | Command |
|---|---|
| Rust format | `cargo fmt --all` |
| Rust lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Rust tests | `cargo test --workspace` (includes the convergence test and the Iroh loopback tests; no internet needed) |
| Rust dependency check | `cargo deny check` |
| Install JS dependencies | `pnpm install` |
| Lint / format | `pnpm lint`, `pnpm format` (CI runs `pnpm format:check`) |
| Types / tests | `pnpm -r typecheck`, `pnpm -r test` |
| Android APK (arm64, debug) | in `apps/mirrorleaf/src-tauri`: `cargo tauri android build --debug --target aarch64 --apk --ci` |
| Rust lint for Android | `cargo clippy --target aarch64-linux-android -p mirrorleaf-ffi -p mirrorleaf-app -- -D warnings`, with `CC_aarch64_linux_android` and `AR_aarch64_linux_android` pointing at the NDK's `aarch64-linux-android24-clang` and `llvm-ar` |
| Phone logs | `adb logcat -s mirrorleaf` (the engine logs there; the push-woken job has no screen) |

## Android (M4, ADR-005)

- One engine per process (`crates/ffi`). On Android, `MirrorleafApp.onCreate` starts it in every process, before the screen or a push-woken job; Tauri commands and the Kotlin code share it.
- The Kotlin bindings are generated during the Gradle build into `.../java/app/mirrorleaf/generated/` and are not committed.
- Tauri's template targets Java 8: Kotlin helpers compiled for Java 11 (such as WorkManager's `OneTimeWorkRequestBuilder`) don't compile; use the plain builders.
- `vendor/ndk-context` is a patched copy that allows a second initialisation (owner's decision). Don't remove the `[patch.crates-io]` entry.
- The Android build uses the Gradle and Kotlin versions Tauri's template generated (owner's decision); don't upgrade them without asking.

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
