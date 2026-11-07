# Plan: write the Phase 1 implementation plan (`mirrorleaf/docs/phase1-implementation-plan.md`)

## Context

Phase 0 is complete (M0–M7; owner, 6 Nov 2026). The Phase 0 report recommends Go with Android option A. ADR-002 to ADR-007 are Accepted. ADR-008 (E0 ledger readiness) is Proposed: it is built but uncommitted. `implementation-plan.md` §8 lists the Phase 1 epics E1–E14 only as one-line rows ("detailed after the Phase 0 go/no-go"). The owner wants a detailed Phase 1 plan built from three sources: the current code, ADR-001 to ADR-008, and those epics.

**The owner's answers (7 Nov 2026):**
1. **Keep the new plan private:** add it to `.gitignore`, like `implementation-plan.md` and the ADRs.
2. **Treat the pending proposals as approved, and write them in as scope:**
   - E0 ledger readiness;
   - ledger-resilience plan changes 2–5: the load test and replay protection in E10, ledger API versioning in E11, the hosting choice (written up first, then built), and E1 keeping the device's approval;
   - reuse assessment changes 1–2: a content type and a sync class on each change;
   - Iroh's mDNS add-on, for LAN sync without internet.
3. **Organise the plan as epics grouped into ordered milestones with gates.** The milestones continue Phase 0's numbering as **M8–M14**, so commit names ("M8 Implementation") and test records keep the same pattern.

**Not changed:** `implementation-plan.md`, `Feasibility-Assessment.md` and `Local-Notes-Plan.md` stay untouched (repo rule: plan edits need explicit approval). The new plan ends with a list of the edits it implies for those files, for a later "apply".

## Files touched

| File | Change |
|---|---|
| `mirrorleaf/docs/phase1-implementation-plan.md` | **New.** The deliverable (about 550–700 lines) |
| `mirrorleaf/.gitignore` | Add `docs/phase1-implementation-plan.md` below the ADR lines |
| `Local_Notes/memory.md` | §4 files table: one row for the new doc. §7: one state line ("Phase 1 plan drafted, 7 Nov 2026"). This follows the project's habit of recording session state there |

Nothing is committed (repo rule). Style: British spelling, tables, decision-oriented, sources cited as relative links (`adr/ADR-004-ledger-decisions.md`, `tests/…`, `../../Feasibility-Assessment.md`).

## Outline of the new document

**Header table:** date 7 Nov 2026; status "Draft for review"; based on plan §8, the Phase 0 report, ADR-001–008, the ledger-resilience record and the reuse assessment §7; scope: Phase 1 (MVP + private beta), macOS and Android.

### 1. How to read this plan
- It details plan §8 and doesn't replace the rest of `implementation-plan.md` (§3 setup, §5 design, §10 agreements still apply).
- The milestone/epic structure; sizes S/M/L as in plan §1.
- Labels: **Decided** (owner, with date); **Approved scope** (the proposals the owner approved on 7 Nov); **Proposed** (Claude's defaults, to confirm in each milestone's ADR).

### 2. Starting point: what Phase 0 left
- **Built, by area** (from the code survey; line counts: core 2.5k, net 2.0k, ffi 1.8k, proto 0.9k, ledger 3.2k, app Rust 0.9k, screen and editor 2.7k, Kotlin 0.6k; 83 Rust tests and 17 JS tests).
- **Phase 0 baselines to hold:** `.dmg` 10.3 MB; APK 28.5 MB file / 12.7 MB download; agent 14 MB; cold start 0.45–0.49 s (Mac) and 0.54–0.65 s (Android); 0.4–0.9 KB per edit; photos 379–595 KB; Android job 30–36 MB.
- **Gaps that Phase 1 epics start from:**
  - The device list only grows (`crates/proto/src/devices.rs`, no removal).
  - The join ticket carries the raw account secret (`crates/net/src/ticket.rs`).
  - The account secret is stored in plain text in `meta` (`crates/net/src/account.rs`); `device.key` is a 0600 file.
  - The approval is signed in memory and never saved (`crates/net/src/ledger.rs`).
  - Nothing calls `DELETE /v1/devices/{id}` with `RemoveDevice`, and there is no re-key endpoint.
  - No snapshots or compaction: `docs` is never written, and every index edit replays the whole index (`crates/core/src/notes.rs` `edit_doc`).
  - `UpdateNeeded` never reaches the UI.
  - Search covers titles and previews only, done in the screen (`apps/mirrorleaf/src/lib/notes.ts`).
  - No empty trash or permanent delete; no photo GC; no Wi-Fi rule for full images.
  - `last_seen` isn't passed to `DeviceSummary`; there's no "only on this device".
  - `Traffic` is dropped by the engine.
  - No onboarding: permissions are asked at every start in `MainActivity`.
  - Tauri default icons; no signing, updater or release workflow.
  - Workspace and package versions are 0.0.0 while `tauri.conf.json` is 0.2.0.
- **Carry-overs:**
  - E0 is uncommitted; E0 tests 1, 2, 3 and 5 haven't been run.
  - M3 Test 6 A–C hasn't been run.
  - H5: the old FCM key. H7: deploying off the Mac.
  - FCM 400 should be handled like 404 (M4 finding 2).
  - Samsung and Xiaomi phones are untested.
  - The M2 transfer timings per network row were never recorded.

### 3. Decisions carried into Phase 1
One table of constraints that Phase 1 must respect, each with its ADR reference:
- The change log is the source of truth, and the job never loads documents (ADR-002).
- One engine, one node and one photo store per process (ADR-005).
- Pushes carry only their type (ADR-004).
- Direction D, one adaptive screen, and the 6 Nov rule that sync status lives in Settings and the main screen speaks only when the user must act (ADR-006).
- Android size limit 30–40 MB for the file and the download, and CI fails above 40 MB (owner, 6 Nov).
- iroh-blobs through the private beta (ADR-003).
- The E8 catch-up job was dropped (owner, 6 Nov).
- Ledger limits are keyed by device, not IP, with a 10-device cap (ADR-008).
- Caddy for HTTPS, and Tailscale Funnel for phone testing (ADR-008).
- Start at login is off by default until the app proves reliable (ADR-006).
- Tauri's template pins and the patched `ndk-context` (ADR-005).
- Iroh stays inside `mirrorleaf-net`; the 7-day rule and `cargo deny` apply.

### 4. Milestones (the core of the document)

| # | Milestone | Epics | Size | Gate ("done when") |
|---|---|---|---|---|
| M8 | Ledger ready and reachable | E0 (the rest of it), FCM 400 fix | S | E0 tests 1–5 and Test 6 A–C recorded; ledger deployed off the Mac over HTTPS; the phone on mobile data completes a push-to-wake round trip; uptime monitor on `/health`; old FCM key deleted |
| M9 | Identity, pairing and the device-list format | E1, plus the wire-format groundwork (device list v2, content type and sync class, `Hello` app version), the hosting-choice write-up | L | QR (phone scans Mac) and word-code (Mac↔Mac/CLI) pairing with the 4-digit confirmation and approval; onboarding replaces the developer-account Settings; the approval is stored; registration works after a ledger wipe |
| M10 | Recovery and device lifecycle | E2, E3 | M+M | Recovering with the 12 words on a wiped device brings back the account, and notes sync back; a revoked device is refused by peers and the ledger; key rotation reaches every remaining device and the ledger re-keys; sign-out warns about notes that exist only on this device, then wipes |
| M11 | Protection | E4, E9 | M+M | DB encrypted (SQLCipher), keys in the Keychain / Android Keystore, PIN or biometric lock with idle timeout; push-woken job and menu-bar agent still sync while locked; backup export → import restores notes and photos on a recovered account; size limits held |
| M12 | Everyday use | E5, E6, E7, E8, mDNS | L | FTS5 search over whole notes; trash empty/purge; nested folders; Wi-Fi prefetch; status details in Settings (behind by N, last seen, only on this device, relay/VPN); permissions in onboarding; LAN check after update; manufacturer guidance; seed-while-charging (off by default); LAN sync with no internet |
| M13 | Production, history and wire freeze | E10, E11, E12 | L | Ledger on production hosting; our own relays and discovery server in use; load test at 10k users; replay protection; push service split out and the hosting choice in Settings; snapshots and compaction; photo GC; old↔new protocol and ledger-API upgrade tests pass; metrics flowing; **wire format v1 frozen** |
| M14 | Beta | E13, E14, then the beta run | M | Signed and notarised `.dmg` with auto-update; Play internal testing (AAB, Play App Signing); brand icon and name everywhere; testers onboarded; the exit criteria (§7) met over the beta period |

Also in §4:
- A dependency diagram, as a Mermaid flowchart.
- **Why this order differs from plan §8's suggested order:**
  - E0 comes first (approved).
  - Wire-format changes sit with the epics that need them (M9, M10, M13), behind one "wire freeze" gate before any tester installs. Changes are free until then, because only the owner's devices exist (ADR-003 note).
  - E9 moves next to E4 because both need the recovery secret's keys.
- **Indicative pace:** 4–6 weeks of build at Phase 0's pace (Phase 0 report §6), then several weeks of beta.
- **Owner's long-lead track, starting at M8:** Apple Developer Program; Play Console; the domain (confirm the `app.mirrorleaf` app ID before the first Play upload); the trademark searches (IP India, USPTO, EUIPO, WIPO; classes 9 and 42); the hosting account and spending; a production Firebase project; release signing keys.

### 5. Epics in detail
One subsection per epic, E0–E14. Each has: **Goal · Starting point (with file refs) · Build · Owner decisions / Proposed defaults · Tests (automated and manual record) · Done when · Lightweight-target risk**.

**E0 Ledger readiness** *(M8, Approved scope)*
- Built: H1–H4, recorded in ADR-008 and `docs/tests/e0-ledger-readiness-tests.md`.
- Left:
  - owner tests 1–5;
  - H5 (old FCM key), H6 (Test 6 A–C), H7 (deploy on the Ubuntu machine or a small VPS);
  - an uptime monitor on `/health`;
  - rebuilding the running container;
  - the FCM 400 fix in `ledger/src/push.rs`;
  - confirming ADR-008.

**E1 Identity and pairing** *(M9)*
- **First-run screens:** Create / Add this device / Recover; device naming.
- **Pairing protocol:** a new ALPN, `mirrorleaf/pair/1`, in `mirrorleaf-net`.
  - The QR code carries the existing device's address and a 128-bit one-time secret.
  - The **word code** is short, so it can't carry an address. **Proposed:** a short code (about 4 words) plus a 10-minute, one-use **rendezvous entry on the ledger** (code hash → node ID, deleted on use), with a PAKE (SPAKE2) so a guessed code can't be brute-forced offline. Magic Wormhole works the same way.
  - Both sides show the 4-digit confirmation; the existing device asks for approval.
- **What the new device receives:** per assessment §8.2, the account key over the encrypted connection. It also **stores its `AddDevice` approval** (plan change 5).
- **Device list v2:**
  - entries gain platform and added-at;
  - a `removed` set (tombstones), so a merge can't bring a revoked device back;
  - an account-settings block (hosting mode, ledger address);
  - a field for the rotation link.
- **Changes gain a content type and a sync class** (reuse changes 1–2). **Proposed:** also a coarse `made_at` time for E12's time-to-sync. These bytes travel device to device only.
- `Hello` gains the app version and platform.
- The developer tickets (`mlt1`) are removed from the app. The CLI keeps them for tests and gains `pair` commands.
- The permission requests move out of `MainActivity.onCreate` into onboarding and pairing (with E8).
- **QR scanning on Android, Proposed:** ZXing (Apache-2.0, works without Play services; needs the camera permission). The alternative is Google's code scanner (no permission, but closed source and needs Play services).
- QR generation: a small Rust crate, checked against size.
- Tests: extend the convergence test with v2 lists; pairing loopback tests (wrong code, expired code, reused code, mismatched confirmation).
- Record: `docs/tests/m9-identity-tests.md`.

**E2 Recovery kit** *(M10)*
- **Proposed:** the 12 words are BIP39-style (128 bits plus checksum), and the **account key is derived from the recovery secret**.
  - Recovering re-creates the same account key, so the ledger's account ID matches.
  - Retiring a kit therefore means rotating the account key (E3). That is the only way an old kit truly stops working, as assessment §8.4 requires.
- Kit contents: a PDF and a text version with the words, QR, fingerprint, date and steps. The PDF generator is chosen for size.
- Shown once; "Generate a new kit" (which rotates the key).
- Recovery flow: words or QR → account restored → the device list rebuilt from the ledger's device IDs and surviving devices → revoke the lost devices.
- Desktop saves through a dialog; Android through the Storage Access Framework in Kotlin, so no bytes pass through the web view (ADR-007).

**E3 Device removal and key rotation** *(M10)*
- **Revoke:** add the device to the `removed` set and send `RemoveDevice` to the ledger (the endpoint exists, nothing calls it); peers refuse it (the `NotOnList` path).
- **Rotate:** a new account key; the old key signs the new one; the new secret goes only to devices in the new list, over sync sessions (a new message type); **ledger re-key endpoint** (`POST /v1/accounts/rekey`, signed by both keys; ADR-004 consequence); a new kit (E2).
- **Sign out:** check "only on this device" (E6), remove itself, wipe the data folder.
- Tests: a revoked peer can't sync or report; a lost device that holds the old key is refused after rotation; rotation while one device is offline.

**E4 App lock and encryption at rest** *(M11)*
- rusqlite `bundled-sqlcipher` (OpenSSL vendored on Android; measure the size against the 40 MB limit).
- A random database key wrapped by the Keychain (`security-framework` is already in the tree) or by Android Keystore (Kotlin, through a uniffi callback).
- The account secret, device key and approval move into the encrypted database or keystore.
- PIN or biometrics with an idle timeout (Touch ID; Android `BiometricPrompt`).
- **Proposed, a decision for the owner (it touches assessment §8.3):** the database key doesn't require the user to unlock, so the push-woken job and the menu-bar agent can sync while the app is locked. The app lock gates the **screen**. The protection is "nothing readable off the device or by other apps", not "nothing readable by the app's own background sync".
- **Photos, Proposed:** stay in iroh-blobs' store under OS encryption (FileVault, Android file-based encryption) until we own the blob store (ADR-003 note). Say so honestly in Settings.

**E5 Notes features** *(M12)*
- **Trash:** "Delete permanently" and "Empty trash"; **Proposed:** auto-purge after 30 days. Removing the deleted note's rows locally needs E11 compaction.
- **Full-text search:** an SQLite FTS5 index over plain note text, inside the encrypted database. The index is built when the app is open, **never in the push-woken job** (the ≤ 50 MB job target). Search moves from `notes.ts` into the engine.
- **Nested folders:** a `parent` field in the index's `folders` map.
- **Full images:** prefetched on unmetered networks (plan §5.5; today they're fetched only when displayed).
- **Share to Mirrorleaf:** done on Android. **Proposed:** defer a macOS share extension (Tauri doesn't build app extensions); offer drop-onto-window/Dock instead.

**E6 Sync status and human fallback** *(M12)*
- Settings → Devices:
  - behind by N;
  - last seen (pass `last_seen` through `DeviceSummary`);
  - "only on this device", from persisted peer version vectors (new table; also used by E11 compaction and the Devices-only mode);
  - connection direct or relay, with a VPN note (M6 finding 6).
- **The main screen stays quiet** (ADR-006, 6 Nov): it speaks only for the fallback banner, notes at risk (for example before sign-out), and "update needed" (a new `UiEvent`, naming the device).
- An onboarding page sets expectations, including that a locked, dozing phone catches up when it's opened (M4 finding 7).
- Status text for Devices-only mode.

**E7 Desktop agent hardening** *(M12)*
- Mostly exists (`desktop.rs`, `power.rs`).
- Replace live mode's 1-second database check with a signal from the engine's own writes. The CLI keeps polling.
- Measure flush-on-close and quit timing; check the agent at idle over 10 minutes after the window is opened and closed.
- Decide when start at login becomes default-on (owner, at the end of the beta).
- Linux sleep/wake is out of scope: Linux isn't a release target.

**E8 Android hardening** *(M12)*
- Permission flows inside onboarding and pairing, with explanations.
- Restart networking after the local network permission is granted (M4 finding 1); check LAN reachability after an app update and guide the user (finding 4).
- Battery-saver guidance by manufacturer (Samsung, Xiaomi, etc.): guidance screens only, no special permission requests that Play policy restricts.
- "Seed while charging": a `dataSync` foreground service, off by default, within the 6 h per 24 h limit.
- A monochrome notification icon (with E14).
- No catch-up job (dropped).

**E9 Encrypted backup** *(M11)*
- An export file holding the change log (or snapshots) and the blobs, streamed in chunks with AEAD (`ring`, already in the tree).
- **Proposed:** a key that only the recovery secret unlocks. Devices hold only an X25519 **public** key derived from the secret and encrypt to it, so making a backup doesn't need the kit, and restoring does ("kit + latest backup = full restore", assessment §8.4). Old backups stay readable with the old kit.
- Import merges by CRDT into the current or a recovered account.
- Reminders every 30 days and after every new kit.
- Android uses the Storage Access Framework, desktop a save dialog.

**E10 Production infrastructure** *(M13)*
- Hosting: **Proposed:** a small VPS with Postgres in the same Compose stack rather than managed Postgres. The ledger rebuilds itself from devices (ADR-004), so it needs no backups. Owner decision (cost).
- Our own `iroh-relay` instances (regions are an owner cost decision; Asia first) and our own `iroh-dns-server`. Replace `presets::N0` (`crates/net/src/node.rs`) with our own map, staying inside `mirrorleaf-net`; check what address records publish.
- Monitoring and alerts with no IP addresses: uptime, push failures, error rates (P2).
- **Load test at the 10k-user scale** (P1, about 65 requests per second).
- **Replay protection:** a request ID kept for the 5-minute window (P4).
- Production FCM credentials handling; optional per-IP limits at the edge (ADR-008).
- **Hosting choice (approved):** the exploratory write-up in `Local_Notes/` first, then:
  - "Mirrorleaf" as the default;
  - "Devices only" (from E6's last-known state);
  - "Your own server", with push split into a stateless push service.
  - The choice is stored in device list v2's settings block, so it covers the whole account.
- **Metadata reduction:** round counts and keep online times for a shorter period.

**E11 Upgrades and history** *(M13)*
- Version negotiation that keeps older versions working; protocol v1 frozen; the chosen version kept per session.
- **Ledger API versioning** (approved): a version header, with the ledger supporting N−1.
- Update prompts: macOS through the updater (E13); Android through Play.
- **Snapshots:** write `docs.state` regularly. Opening a note = snapshot + newer rows. The index gets a snapshot too, which fixes the replay on every index edit.
- **Compaction:** the safe point is the lowest version vector across the signed list (from the persisted peer vectors). Rows at or below it are deleted, and a device below the safe point receives snapshots (a new wire message, before the freeze).
- **Photo GC** through the named tags (`crates/net/src/blobs.rs`).
- Measure open time for a long-edited note before and after.
- Upgrade tests: a peer on the old build and a peer on the new one sync in both directions.

**E12 Metrics** *(M13)*
All aggregate and with no IP addresses. **Proposed, for the owner:**
- The ledger keeps daily aggregates only, never a per-account history.
- **% of edits on all devices within 1 h and 24 h:** from the origin's counter time against when each device's seen counter passes it. Short-lived rows are deleted once every device has the edit.
- **Time-to-sync p50/p95:** from `made_at` on arrival, reported as histogram buckets.
- **Push-to-wake success by manufacturer:** the job's result plus a manufacturer bucket.
- **Relay share:** `Traffic` by path kind.
- **FCM downgrade share:** FCM's delivery data.
- **Merge anomalies:** testers' reports through a feedback channel.
- Whether metrics are opt-in for testers is an owner decision.

**E13 Beta distribution** *(M14)*
- Developer ID signing and notarisation in CI (secrets only; `.p12` and `.p8` are already gitignored).
- `tauri-plugin-updater` with a signed manifest on GitHub Releases.
- Play internal testing with an AAB, Play App Signing and an upload key in CI.
- A tag-driven release workflow.
- One version source (the workspace and `package.json` are 0.0.0; the app is 0.2.0).
- Privacy policy and Play data-safety form (owner).
- Size checks carried over: `.dmg` ≤ 25 MiB; APK and download ≤ 40 MB, measured with bundletool for the AAB.

**E14 Brand** *(M14)*
- App icon, Android adaptive and monochrome icons, and the notification icon (the menu-bar leaf exists).
- Replace "Mirrorleaf Phase 0" in About.
- Name and tagline across the apps.
- Trademark clearance before the public beta (owner, with a lawyer); store, domain and handle checks.

### 6. Lightweight targets through Phase 1
A table: target, limit, Phase 0 baseline, the milestones that put it at risk, and the CI check.
- SQLCipher and OpenSSL put the APK at risk.
- FTS5 and the job: indexing stays out of the job.
- The QR and PDF libraries affect the bundle and the `.dmg`.
- The updater affects the `.dmg`.
- `made_at`, content type and sync class add a few bytes per change; check against ≤ 5 KB.

Every milestone's test record re-measures the targets it touches; M14 re-runs the full M7 set on release builds.

### 7. Beta and the Phase 1 exit criteria (Proposed, for the owner)
- **Beta:** about 10–30 testers on Mac and Android, including at least one Samsung or Xiaomi phone, for at least 4 weeks.
- **Exit:**
  - no confirmed lost-text merge anomaly;
  - time-to-sync while both are online: p50 ≤ 5 s, p95 ≤ 30 s;
  - high-priority push-to-wake ≥ 95% on stock Android, recorded by manufacturer;
  - FCM downgrade share ≤ 5%;
  - every lightweight target held in each beta build;
  - % of edits on all devices within 24 h **recorded**: this sets the Phase 2 trigger X% (an open decision in assessment §13).

### 8. Testing approach
- Automated:
  - the existing suites;
  - the convergence test extended (v2 lists, revocation, rotation, compaction);
  - pairing loopback tests;
  - ledger integration tests for re-key, replay and versioning;
  - yrs↔Yjs fixtures;
  - old↔new upgrade tests;
  - the load test.
- Manual: one test record per milestone (`docs/tests/m8-…` to `m14-…`; M8 reuses the E0 record).
- Phone driven over adb as in M6, with a check that the app is in front before tapping.

### 9. Decisions table
Every **Proposed** item above in one place, with a recommendation and the milestone it's needed by. Covers:
- the word-code rendezvous and PAKE;
- the QR scanner;
- the account key derived from the recovery secret;
- app lock vs background sync;
- photos at rest;
- the backup key design;
- `made_at`;
- trash auto-purge;
- the macOS share extension;
- VPS vs managed Postgres;
- relay regions;
- the metrics privacy model and opt-in;
- the exit thresholds;
- when start at login becomes default-on;
- the app ID / domain.

Plus the owner's open items: confirm ADR-008; licence, monetisation and iOS timing (after Phase 1).

### 10. Risks specific to Phase 1
- Wire changes after the freeze.
- Rotation edge cases (an offline device, concurrent rotations).
- SQLCipher size and Android OpenSSL.
- PAKE and pairing UX across NAT (relay fallback).
- Store and notarisation review delays.
- Production relay cost.
- iroh-blobs maturity (revisit before the public beta).
- Privacy of metrics.
- The 8 GB Mac with OpenSSL and SQLCipher builds.

### 11. Working agreements
Plan §10, plus:
- never commit or push (repo `CLAUDE.md`);
- one ADR per milestone from **ADR-009** (Proposed until confirmed; each gets its own `.gitignore` line);
- the 7-day rule and `cargo deny`;
- British spelling;
- doc changes need approval.

### 12. Edits this plan implies to other docs (not applied; for a later "apply")
- `implementation-plan.md`:
  - §8: add E0; point to this plan; fold in plan changes 2–5 and reuse changes 1–2;
  - §7.3: Android 30–40 MB;
  - E8: drop the catch-up job;
  - §4 layout: list the new doc.
- Assessment §12 (size) and §8.3 (app lock wording, if the E4 default is accepted); `Local-Notes-Plan.md` to match.

### Appendix
Where every ADR consequence and Phase 0 follow-up lands, as a source → epic/milestone table, so nothing from ADR-001–008, the Phase 0 report §4 or the test records' findings is dropped.

## Verification (after writing)

1. `git -C mirrorleaf check-ignore -v docs/phase1-implementation-plan.md` shows the new `.gitignore` line; `git status --short` shows `.gitignore` modified and no new untracked plan file.
2. Cross-check every figure and file reference in the doc against its source: Phase 0 report §2.2 baselines, the ADR numbers and decisions, and the file paths from the survey. `ls` each repo path the doc cites.
3. Check that every relative link resolves (`adr/…`, `tests/…`, `../../…` for the parent-folder docs).
4. Check that every item in the appendix's source list (ADR consequences, Phase 0 report §4 follow-ups, M4 findings 1/2/4/7, M6 finding 6, ADR-002 mDNS, ADR-003 GC, ADR-004 re-key and nonces) maps to an epic.
5. Read the whole doc once for British spelling and for consistent labels (Decided / Approved scope / Proposed).
6. Final message: one line naming the audience (the owner, as PM, and Claude in later sessions); the link to the file; the Proposed items that need the owner's decisions; and that `implementation-plan.md` and the assessment were not changed (edits listed in §12).
