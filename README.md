# Mirrorleaf

**Your notes, mirrored on your devices. Nowhere else.**

Mirrorleaf is a private notes app for rich text and photos. Notes live only on your own devices and sync directly between them, over your local network or the internet. A small service tracks sync status, never content.

> **Status:** Phase 0 technical prototype. Nothing here is usable yet.

## Repository layout

| Path | What it holds |
|---|---|
| `crates/core` | Storage, Yjs documents, change log, sync engine, photos |
| `crates/proto` | Wire messages and versioning, shared with the ledger |
| `crates/net` | Iroh networking and peer authentication |
| `crates/ffi` | uniffi bindings for native code |
| `crates/cli` | Headless peer for tests and measurements |
| `ledger/` | Sync-status service (axum and Postgres) |
| `packages/editor` | TipTap editor and its bridge to Rust |
| `docs/` | Implementation plan and architecture decision records |

The Tauri app (`apps/mirrorleaf`) arrives in milestone M4, and the measurement scripts (`tools/`) after it.

## Set up on a Mac (Apple silicon)

1. Xcode Command Line Tools: `xcode-select --install`
2. Rust from [rustup.rs](https://rustup.rs), then `rustup component add clippy rustfmt`
3. Node 24.22 from [nvm](https://github.com/nvm-sh/nvm): in this folder, run `nvm install` (it reads `.nvmrc`)
4. pnpm: `corepack enable` (`package.json` pins pnpm 12.16.1)
5. Install dependencies: `pnpm install`
6. Check everything works:

   ```bash
   cargo test --workspace
   pnpm -r test
   ```

Optional: `cargo install cargo-deny --locked`, then `cargo deny check`, to run CI's dependency checks locally.

Android, Java and Tauri come in M4 and M5; see `docs/implementation-plan.md` §3.2.

## Licence

There's no licence yet. Mirrorleaf will be open source; until a licence is chosen, all rights are reserved.
