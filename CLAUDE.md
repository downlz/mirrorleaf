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
| Rust tests | `cargo test --workspace` |
| Rust dependency check | `cargo deny check` |
| Install JS dependencies | `pnpm install` |
| Lint / format | `pnpm lint`, `pnpm format` (CI runs `pnpm format:check`) |
| Types / tests | `pnpm -r typecheck`, `pnpm -r test` |

## Working on the 8 GB Mac

- `.cargo/config.toml` caps Cargo at 4 jobs, and dependencies build without debug info.
- Use a physical Android phone, never the emulator (from M4).
- Release builds, all-CPU Android builds and size checks run in CI.

## Versions and dependencies

- Node 24.22 (`.nvmrc`) and pnpm 12.16.1 (`packageManager`).
- pnpm installs only releases at least 7 days old. A dependency that needs an install script must be added to `allowBuilds` in `pnpm-workspace.yaml`, and that needs the owner's approval.
- TypeScript stays on 6.0.x until typescript-eslint supports 7. Yjs stays on 13 and Tauri on 2. iroh-blobs is pre-1.0: pin it exactly.
- New Rust dependencies must pass `deny.toml`'s licence list (permissive plus MPL-2.0).
- Formatters stay out of `docs/` and Markdown files.
