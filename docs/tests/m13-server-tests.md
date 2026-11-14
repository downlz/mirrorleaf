# M13 the public server: test steps and results

| | |
|---|---|
| **Work** | Phase 1 milestone M13: E10 (our own relay, the ledger open to anyone, the server), E12 (the beta's metrics), the hosting choice, the ledger API's versioning (E11). Snapshots, compaction and photo clean-up (the rest of E11) come next. |
| **Machines** | The Mac (tests, a test server in Docker), the owner's Hetzner server, the Mac app and the Motorola Edge 60 Pro once the server runs |
| **Decisions** | ADR-013 |
| **Date written** | 15 November 2026 |
| **Status** | Deployed on Mirrorleaf's server (15 Nov 2026) without Firebase's key; not installed on any device |

---

## What changed

| Area | Change |
|---|---|
| Relay | Our own (`relay/`, `mirrorleaf-relay`), on n0's `iroh-relay` 1.4.1, behind Caddy at `relay.mirrorleaf.app`, with its QUIC address check on UDP 7842. Only devices the ledger knows get in. No IP addresses in its logs. |
| Devices | n0's relays and discovery are no longer used. Devices use the relay their ledger names (`https://relay.mirrorleaf.app` built in) and publish nothing anywhere. |
| Ledger | API version 2 (version header, a random value signed with each request; version 1 still served); a request sent twice is refused; the sign-up puzzle; limits per address kept in memory; relay passes for joining devices; accounts never used deleted after 7 days; `block`/`unblock`; the relay's access check, the dashboard and `stats` on the internal port. |
| Metrics | Devices send counts with their status reports; the ledger keeps daily totals only. Settings → **Share anonymous sync statistics** (on); a line on the first screen. |
| Hosting choice | Settings → **Sync service: Mirrorleaf / Devices only**, for all the account's devices. |
| Server | `docker compose --profile https up -d --build` runs Caddy, the ledger, Postgres and the relay, with memory and CPU limits; steps in the README. |

---

## Automated checks (Claude, 15 November 2026)

| Check | Result |
|---|---|
| `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`, Android clippy for the engine and the app | ✅ Clean |
| `cargo test --workspace` with a test Postgres 18 | ✅ 152 pass. New: the relay lets in allowed devices and keeps out a stranger (devices that can reach each other only through it); the puzzle (an answer checks only for its account, device and time); version-2 signatures; limits by address and IPv6 /64; replays; the puzzle getting harder with sign-ups; the ledger: new accounts need the puzzle and joining doesn't, the device's own client solves it, a third sign-up from one address is refused and another address isn't; the relay check for registered devices, word-code lookups and QR passes, passes ending after 10 minutes, a blocked account; a version-1 request served and the same request refused the second time, an unknown version refused; unused accounts deleted after a week; counts reaching the daily totals, catch-up times, versions, the dashboard |
| `cargo deny check` | ✅ ok; 23 new crates (the relay's), all at least 7 days old; duplicate versions 35 → 37 |
| `pnpm lint`, `pnpm format:check`, `pnpm -r typecheck`, `pnpm -r test` | ✅ Clean; 30 tests pass |
| Android debug APK (arm64) | ✅ Builds (the phone maker passed to the engine) |

## A test server in Docker on the Mac (Claude)

A separate Compose project (`m13test`, other ports) with `--profile relay`, so the owner's ledger isn't touched.

| Check | Result |
|---|---|
| Both images build | ✅ after a fix: building both at once collided on Cargo's shared cache (the caches are now locked, so they build in turn) |
| The ledger starts with the M13 settings | ✅ "devices are told to use the relay", internal port on 8081, `GET /v1/info` → `{"api":[1,2],"relay":"http://127.0.0.1:13340"}` |
| The relay starts | ❌ found two bugs, both fixed in the code, not yet re-run: its HTTP client won't start without the system's CA certificates (now in the image); a read-only container can't get Caddy's data mounted on some Docker engines (the mount point is now made in the image) |
| The dashboard | ❌ found a bug, fixed, not yet re-run: without Firebase, nothing installed a TLS provider, so reading the relay's counters crashed the request (the ledger now installs one at start) |
| Load test | Not run yet |

**Stopped there:** the images filled the Mac's disk (368 MB left; Rust's build folder had grown to 80 GB). Docker's virtual disk turned read-only, so the test server, and the owner's own ledger database in the same Docker engine, stopped. 14 GB freed (Rust's incremental build cache); Docker needs a restart by the owner before these checks run again.

## On Mirrorleaf's server (Claude, 15 November 2026)

Hetzner, Frankfurt, Ubuntu 26.04, already running nginx for three other sites. Done over SSH: the code copied to `/opt/mirrorleaf` (no secrets), an nginx site `mirrorleaf` (HTTP first for the certificate check, then HTTPS for both names, request logs off), one certbot certificate for both names (renewal reloads nginx), `.env` with a new database password (readable by root only), `docker compose --profile relay up -d --build`. nginx's configuration was tested before each reload, and the other sites checked before and after each step.

| Check | Result |
|---|---|
| DNS | ✅ Both names → 204.168.198.17, DNS only, no IPv6 record |
| The other three sites | ✅ 200 before, between and after every step |
| Images build on the server, containers start | ✅ Postgres healthy; the ledger: "devices are told to use the relay https://relay.mirrorleaf.app"; the relay: certificate loaded, QUIC address check on UDP 7842. The three bugs the Mac's test server found are confirmed fixed |
| From the internet | ✅ `/health` → ok; `/v1/info` → `{"api":[1,2],"relay":"https://relay.mirrorleaf.app"}`; the relay's `/healthz` and `/ping` → 200; the certificate names both domains; the dashboard port doesn't answer from outside |
| Two throwaway CLI devices against the server | ✅ A created an account (the puzzle, 21 bits, solved within its 1.7 s registration); B joined by word code: its lookup gave it a relay pass, its relay connection was let in ("home is now relay https://relay.mirrorleaf.app/"), it registered; after a fix (below) they synced. Both then left the ledger |
| `mirrorleaf-ledger stats` and the dashboard | ✅ 1 account, 2 devices, the sign-up, the lookup, a device catching up within 1 h, the puzzle's 21 bits |
| IP addresses in the relay's log | ✅ None: only its own listening addresses |

Found and fixed: the CLI's first sync after joining, and its `sync`, dialled devices by their ID alone, which no longer works without a lookup service; they now dial through the relay (`SyncNode::dial_addr`), as the app already did. Two labels corrected (devices with no version "not reported"; `stats` no longer prints a live difficulty it can't know). These three fixes are not on the server yet.

Not done yet: Firebase's key isn't on the server (pushes are only logged until it is); the load test runs only against a ledger started for testing.

## 0.5.0 on the owner's phone and Mac (Claude, 15 November 2026)

| Step | Result |
|---|---|
| Version 0.5.0 (owner); Android debug APK and the signed Mac app built with the server as the default ledger | ✅ |
| Phone: a copy of its app data taken (scratchpad), 0.5.0 installed over 0.4.0 with its data | ✅ |
| Phone moved to `https://ledger.mirrorleaf.app` (the call Settings' "Use this ledger" makes) | ✅ Registered; it created the account on the server, solving the puzzle in 945 ms (debug build) |
| Mac: 0.4.0 quit as ⌘Q does, kept in the scratchpad, 0.5.0 put in Applications and opened | ✅ Same signature, so no Keychain question |
| Phone ↔ Mac app | ✅ "connected to 44587b05 via direct" |
| Mac app moved to the server | ✅ Owner, in Settings. Server: 1 account, 2 devices (Android, macOS), both 0.5.0, none behind. Its first report reached the server only at the 5-minute heartbeat: **fixed** for the next build (a new ledger now gets a report within 30 s) |
| **Across networks** (Claude): phone Wi-Fi off, on Airtel LTE; live sync restarted | ✅ "connected to 44587b05 via direct": the relay set the connection up (its accepts 4 → 5, about 30 KB) and the phone and the Mac hole-punched through. The relay carries connections that can't |
| **Waking the phone through the server** (Claude): app in the background, on LTE; `mirrorleaf-ledger test-push d57d4595 high` on the server | ✅ "push: pull, high priority (sent as high)" within a second, "sync job started (high)", the job ran (2.1 s). Nothing to sync: at that moment the server didn't yet count the Mac as online (the heartbeat above) |
| Phone restored | ✅ Wi-Fi back on, at its home screen. The other three sites on the server: 200 |
| Settings → About shows the version (owner's request, 15 Nov 2026): "Mirrorleaf 0.5.0" instead of "Mirrorleaf Phase 0", from `tauri.conf.json` (`app_version` command) | ✅ On the phone: "Mirrorleaf 0.5.0. Your notes, mirrored on your devices. Nowhere else." Rebuilt and installed on the phone and the Mac (these builds also carry the older-ledger fallback and the prompt first report to a new ledger) |

Found on the phone and fixed: before the switch, the 0.5.0 phone was refused by the Mac's older ledger ("the signature doesn't match"): ledgers before M13 check version-1 signatures only. The app now sends a refused request once more as version 1 (checked with a throwaway CLI device against the Mac's ledger: "the ledger … is older: using its API version 1", registered, then removed). The builds installed today may not include it; they don't need it once on the server.

---

## Tests for the owner

### 1. Deploy the server

Follow the README's "Deploying on a server". **Expect:** `https://ledger.mirrorleaf.app/health` answers `ok`; `https://ledger.mirrorleaf.app/v1/info` names `https://relay.mirrorleaf.app`; `https://relay.mirrorleaf.app/healthz` answers; the relay's log says the QUIC address check is on UDP 7842.

### 2. Move the devices to the server

Install the M13 builds on the Mac and the phone (after test 1 passes). In Settings, set the ledger to `https://ledger.mirrorleaf.app` on each. **Expect:** each registers (the first device of the account solves the puzzle, about a second); Settings → Devices shows the others "connected"; the phone on mobile data and the Mac on Wi-Fi sync, "connected through the relay" or "direct".

### 3. Waking the phone through the new server

Close Mirrorleaf on the phone, edit a note on the Mac. **Expect:** the phone syncs within about 10 s, as before.

### 4. The dashboard

`ssh -L 8081:127.0.0.1:8081 <server>`, then `http://localhost:8081`. **Expect:** accounts, devices by platform, sync speed and wake-ups for the day; no names or IDs.

### 5. Devices only, and the statistics switch

Settings → Sync service → **Devices only** on the Mac. **Expect:** the phone shows the same choice after they next connect; the dashboard's "can be woken by push" goes down by one; the two still sync, through the relay too. Switch back to **Mirrorleaf**. Turn **Share anonymous sync statistics** off on one device: its counts stop.

---

## Results

| Test | Result |
|---|---|
| Automated checks | ✅ Pass (Claude, 15 Nov 2026) |
| Test server in Docker | Three bugs found and fixed; to run again after Docker restarts |
| Server deployed and checked | ✅ Claude, 15 Nov 2026 |
| 1–2 | ✅ Done (server deployed; both devices on it) |
| 3 (an edit on the Mac reaching the closed phone through the server's push rules) | Owner |
| 4–5 | Owner |
