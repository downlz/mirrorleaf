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

Setup steps for Android, Java and Tauri will be added in milestones M4 and M5.

## Try the headless peer

`mirrorleaf-cli` is a device without a screen, used for testing sync. Each data folder is one device (`--dir`, default `~/.mirrorleaf-dev`). Build it with `cargo build -p mirrorleaf-cli`, or download a release build from a CI run's **Artifacts** (macOS on Apple silicon, and Linux for Ubuntu 24.04 or newer).

On the first device:

```bash
mirrorleaf-cli init --name laptop
mirrorleaf-cli dev-account new
mirrorleaf-cli note new "Groceries" --text "milk"
mirrorleaf-cli dev-account ticket      # prints a join ticket
mirrorleaf-cli sync --watch            # keep this running
```

On the second device, paste the ticket:

```bash
mirrorleaf-cli init --name ubuntu
mirrorleaf-cli dev-account join <ticket>
mirrorleaf-cli note list
mirrorleaf-cli sync --watch
```

Edits made in another terminal (`note edit <id> --append "eggs"`) reach the other device within seconds. `sync --watch` reports each connection as **direct** or **relay**, with the time and bytes each sync took. `status` shows the devices and how many changes this device holds from each.

> The join ticket contains the account key. Share it only with your own devices. Phase 0 developer accounts are for testing; real pairing comes later.

### Cross-network test

Run the two devices on different networks and note, for each case, whether the connection is direct or relay, and how long a text edit takes to arrive:

| Case | Device A | Device B |
|---|---|---|
| 1 | Home Wi-Fi | Same Wi-Fi |
| 2 | Home Wi-Fi | A laptop tethered to a phone on mobile data |
| 3 | Home Wi-Fi | A different Wi-Fi (another home or an office) |
| 4 | A network that blocks UDP, if available | Any |

Case 4 should still sync, through a relay.

## Licence

There's no licence yet. Mirrorleaf will be open source; until a licence is chosen, all rights are reserved.
