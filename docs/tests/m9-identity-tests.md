# M9 identity and pairing: test steps and results

| | |
|---|---|
| **Work** | Phase 1 milestone M9: E1 identity and pairing, with the wire-format groundwork (device list 2, each change's content type, sync class and time, the app version in `Hello`) |
| **Machines** | The Mac (app and CLI), the Motorola Edge 60 Pro, the Ubuntu machine (Linux app or CLI) |
| **Decisions** | ADR-009 |
| **Date written** | 9 November 2026 |

---

## What changed

| Area | Change |
|---|---|
| First run | A welcome page with **Start a new account**, **Add this … to my account** and **Recover my account** (direction A, owner, 9 Nov 2026). Recovering comes with the recovery kit in M10. |
| Adding a device | Settings → Devices → **Add a device…** shows a QR code and, when the device has a ledger, a word code (`7-acoustic-planet-river`). Both work once, for 10 minutes. The new device scans the code (Android) or types the words, both devices show the same four-digit number, and the existing device asks **Allow “…” to join?** Only then does the new device receive the account. |
| Android permissions | Notifications and nearby devices are asked in the joining sheet (and after starting an account), each with a one-line reason, no longer at every start. Settings shows **Notifications are off** or **Nearby devices are off** with **Allow**. |
| Developer tickets | Gone from the app. The CLI keeps `dev-account ticket` and `join` for tests and gains `pair invite` and `pair join`. |
| Sync format | Protocol 2 on a new connection name (`mirrorleaf/sync/2`): **0.2.0 builds can't sync with M9 builds (0.2.5)**, so every device needs the M9 build. Each device converts its device list the first time the M9 build opens it; nothing needs pairing again. |
| Ledger | Migration 2 adds pairing numbers (`POST`, `GET`, `DELETE /v1/pairing`). Rebuild the ledger image before using word codes. |

## Before you start

| Rule | Why |
|---|---|
| Rebuild the ledger: `docker compose up -d --build` in `ledger/` | Word codes need its pairing table. (Done on the Mac's ledger on 9 Nov 2026.) |
| Put the M9 build on every device you test with | 0.2.0 and M9 can't sync with each other |
| Build the Android APK with `MIRRORLEAF_DEFAULT_LEDGER=http://<mac address>:8080` | A phone with no account yet looks word codes up there. A scanned QR code carries the ledger's address itself. |

---

## Automated checks (Claude, 8–9 November 2026)

| Check | Result |
|---|---|
| `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings` (and for Android: the engine and the app) | ✅ Clean |
| `cargo test --workspace` with Postgres 18.6 | ✅ 104 Rust tests pass. New: 5 pairing tests over loopback (a scanned code joins and then syncs; a typed word code; a wrong code is refused and ends the code, so even the right one fails afterwards; “Don't allow”; no invitation showing); the ledger's pairing numbers; device list 2 (a removed device stays removed, the better entry wins on both sides, a Phase 0 list converts to the same signed list everywhere, the rotation link); unknown content types pass through without touching documents; the error wording |
| `cargo deny check` | ✅ advisories, bans, licences and sources ok; no new duplicate crate versions (35 before and after) |
| `pnpm lint`, `pnpm format:check`, `pnpm -r typecheck`, `pnpm -r test` | ✅ Clean; 26 tests pass |
| Android debug APK (arm64) | ✅ Builds, Kotlin included (`QrScannerPlugin`, `PermissionsPlugin`) |

## Pairing between CLI devices over the internet (Claude, 8 November 2026)

Two and then three throwaway CLI devices on the Mac, with a throwaway ledger (migration 2, log-only), through n0's relays.

| Step | Result |
|---|---|
| “mac” runs `pair invite --yes`; “phone” runs `pair join 1-rifle-major-idea --ledger …` | ✅ Both printed **2624**; joined; “phone” registered with the ledger. (Found a CLI bug: `pair invite` stopped once the pairing finished instead of carrying on syncing. Fixed.) |
| “desk” joins with the QR code's text (`MLP1…`, 382 characters) | ✅ Both printed **3782**; joined; synced straight away, **direct**, receiving the note made before |
| “phone” syncs afterwards | ✅ Received the note, direct |

## Converting a Phase 0 device (Claude, 8 November 2026)

On a **copy** of the owner's Mac CLI device folder (`~/.mirrorleaf-dev`), with `status` only (no network), then deleted.

| Check | Result |
|---|---|
| The device list | ✅ Converted to version 2 at the same list version (4): mac, ubuntu, Shahnawaz's MacBook Air, motorola edge 60 pro, names intact |
| The database | ✅ Schema 2 → 3; all 252 changes read as Yjs updates, time unknown; the ledger approval stored |

## The screens in a headless browser (Claude, 9 November 2026)

The built screen in Firefox's headless mode, with a stand-in for the app's engine answering with example data, at phone size (412 × 880) and Mac size (1120 × 740), light and dark.

| Screen | Result |
|---|---|
| Welcome page, phone and Mac | ✅ As mockup A: the logo, “Mirrorleaf”, the tagline, the three choices (“Add this phone…” on the phone, “Add this Mac…” on the Mac) |
| Start a new account | ✅ “Name this phone” prefilled, Start and Cancel |
| Add this phone | ✅ Scan the code, the words field, “Your other devices will call it …” with Change, Connect, Ledger… |
| Check this number | ✅ 4 8 2 1, centred on the phone, “Waiting for your other device” |
| Settings → Add a device (Mac, phone) | ✅ The QR code dark on white in both themes, the words, “Works once · 9:10 left”, Show the code as text |
| Allow “…” to join? | ✅ The number, Don't allow and Allow |
| Settings with both permissions off (phone) | ✅ “This phone”: Notifications are off / Nearby devices are off, each with Allow |

## On the owner's Mac and phone (Claude, 9 November 2026)

The owner installed the M9 release build on the Mac (`/Applications/Mirrorleaf.app`, built 9 Nov 00:47; both new connection names in the binary). Claude installed the M9 debug APK on the Motorola Edge 60 Pro over its 0.2.0 build with `adb install -r` (data kept), and drove the phone over USB, checking the app was in front before every tap. Both builds still say version 0.2.0 (see finding 3).

### Test 1: the upgrade keeps everything

| Step | Result |
|---|---|
| Phone opens the M9 build | ✅ The notes (19, plus 2 deleted), not the welcome page. Log: “converted the account's device list to version 2”. Cold start 1,705 ms (debug build, conversion included); 1,053 ms on the next start |
| The Mac CLI device “mac” opens the M9 build | ✅ Converted: list version 4, the same 4 devices, 21 notes, 252 changes |
| “mac” makes a note | ✅ Sent straight to the Mac app (525 ms) and the phone (551 ms); on the phone at once |
| The phone makes a note (typed in the search field, Enter) | ✅ Sent to the Mac app (3 changes); “mac” then received the 3 changes from the Mac app; the ledger shows the Mac app and the phone 0 behind |
| The Mac app's own window | Owner to confirm it opened on the notes (Claude can't see the Mac's screen). It synced with every device, so it has its account |

### Test 2, with the phone inviting: a new CLI device types the words

| Step | Result |
|---|---|
| Phone: Settings | ✅ No developer tickets; **Add a device…** at the top; the same devices |
| Add a device… | ✅ QR code, `1-blind-napkin-more`, “Works once · 9:57 left”, Show the code as text |
| A new CLI device “M9 test CLI” types `1 Blin napk MORE` (spaces, mixed case, four-letter words) | ✅ Accepted; both showed **6292**; the phone asked **Allow “M9 test CLI” to join?** |
| Allow | ✅ Phone: “M9 test CLI joined. Your notes are syncing to it now.” CLI: joined, registered with the ledger the phone passed on (`http://192.168.1.13:8080`), received all 256 changes **direct in 1.0 s** |
| The new device syncs with everyone | ✅ The Mac app accepted it and sent it the photo thumbnails (direct); device list 5 reached “mac” |

“M9 test CLI” stays on the account: removing a device comes with M10 (E3). Its folder is in Claude's session scratchpad.

### Test 4: a wrong code, “Don't allow”, the time running out (phone inviting)

| Step | Result |
|---|---|
| A new CLI device types `1-because-rigid-river` (last word changed) | ✅ CLI, after 3.2 s: “the code didn't match; make a new one on your other device”. Phone: “A device tried a wrong code, so this code stopped working. Make a new one.” with **Make a new code** |
| The right words afterwards | ✅ Refused: “no device is showing a code starting with 1: check the number, or make a new code” |
| A new CLI device joins with the right words; **Don't allow** on the phone | ✅ Both showed **8087**. CLI: “your other device didn't allow this one to join”; it has no account. Phone: “You didn't allow the device to join.” (shown in red, finding 2) |
| A code left showing for 10 minutes (`1-mammal-letter-child`, from 08:27:25) | ✅ At 08:37:49 the phone showed “This code has run out.” (not in red) with **Make a new code**; the words were refused: “no device is showing a code starting with 1” |

### Test 5: permissions on a phone that's already set up

| Step | Result |
|---|---|
| Both permissions turned off (`pm revoke`), app opened | ✅ No permission prompt at start |
| Settings | ✅ **This phone**: “Notifications are off · Your other devices can't wake this phone to sync.” and “Nearby devices are off · Sync goes over the internet, even on the same Wi-Fi.”, each with **Allow** |
| Allow (each) | ✅ Android's own dialog for that permission only; after allowing, the row goes. After nearby devices, the ledger answered with no restart (M4 finding 1 didn't recur). Both permissions are granted again, as before the test |

### Findings

| # | Finding | State |
|---|---|---|
| 1 | **The phone reaches the Mac app only through the relay**, on the same Wi-Fi, even after a fresh start with nearby devices allowed. The phone reaches the CLI devices on the same Mac directly, both ways. So the Mac app's side is blocked, most likely by macOS's Local Network permission: the app is ad-hoc signed, so the rebuilt copy counts as a new app. Sync works, just through the relay | Owner to check System Settings → Privacy & Security → Local Network → Mirrorleaf |
| 2 | The inviter's “You didn't allow the device to join.” is shown in the error colour, although it was the person's own choice | Fixed (owner, 9 Nov 2026): `AddDevice.svelte` shows it in the normal text colour after **Don't allow**; other endings stay red. Checked on the phone with the 0.2.5 build (08:55): grey text, and the joining device refused as before |
| 3 | Both M9 builds still say **0.2.0**, the same as the builds they can't sync with, so `Hello`'s app version can't tell them apart | Fixed (owner, 9 Nov 2026): M9 is **0.2.5** (`tauri.conf.json`; Android versionCode 2005) |
| 4 | The CLI's `sync` summary names a device added since its last run by its ID (`c07fe7c2`), because it prints the ledger's reply before taking the newer device list | Minor, CLI only |

---

## Tests for the owner

### 1. The upgrade keeps everything

Install the M9 build on the Mac (quit the running Mirrorleaf from the menu bar first, then open `target/release/bundle/macos/Mirrorleaf.app`) and on the phone.

**Expect:** both open on their notes, not on the welcome page. Settings → Devices lists the same devices as before. An edit on one reaches the other.

### 2. Add a device: the Ubuntu machine types the words

1. On the Ubuntu machine, install the M9 Linux app (or run `mirrorleaf-cli init --name ubuntu-m9` with a new `--dir`).
2. On the Mac: Settings → Devices → **Add a device…**
3. On the Ubuntu machine: **Add this computer to my account**, type the words (or `mirrorleaf-cli pair join <words> --ledger http://<mac address>:8080`).

**Expect:** both show the same four-digit number; the Mac asks **Allow “…” to join?**; after **Allow**, the Mac says it joined, and the Ubuntu machine shows your notes within seconds.

### 3. Add a phone by scanning (optional: clears the phone's copy)

Only if you are happy to clear the phone's copy of the notes, which the Mac also holds: Android Settings → Apps → Mirrorleaf → Storage → **Clear storage**. Then open Mirrorleaf on the phone.

**Expect:** the welcome page. **Add this phone to my account** asks first for notifications and nearby devices (each with a reason), then shows **Scan the code**. Scanning the Mac's code needs no camera permission. The same number on both; **Allow** on the Mac; the notes arrive on the phone.

### 4. A wrong code, “Don't allow”, and the time running out

| Step | Expect |
|---|---|
| Type the words with one word changed | The joining device: “The code didn't match; make a new one on your other device.” The Mac: “A device tried a wrong code, so this code stopped working. Make a new one.” |
| Join, then press **Don't allow** on the Mac | The joining device: “Your other device didn't allow this one to join.” |
| Leave a code showing for 10 minutes | “This code has run out”, with **Make a new code** |

### 5. Permissions on a phone that's already set up

In Android Settings, turn off Mirrorleaf's notifications. Open Mirrorleaf → Settings.

**Expect:** “This phone: Notifications are off” with **Allow**, which asks Android again. No permission prompt when the app starts.

---

## Results

| Test | Result |
|---|---|
| Automated checks | ✅ Pass (Claude, 8–9 Nov 2026) |
| CLI pairing over the internet | ✅ Pass (Claude, 8 Nov 2026) |
| Converting a Phase 0 device | ✅ Pass (Claude, 8 Nov 2026) |
| Screens in a headless browser | ✅ Pass (Claude, 9 Nov 2026) |
| 1. The upgrade keeps everything | ✅ Pass on the phone and the Mac CLI device; Mac app syncs (Claude, 9 Nov 2026). Owner to confirm the Mac app's window |
| 2. Ubuntu types the words | ✅ Pass with the phone inviting and a new Mac CLI device typing (Claude, 9 Nov 2026). Ubuntu and the Mac app as inviter not yet run |
| 3. A phone scans (optional) | Not yet run |
| 4. Wrong code, Don't allow, time running out | ✅ Pass with the phone inviting (Claude, 9 Nov 2026); finding 2 |
| 5. Permissions in Settings | ✅ Pass (Claude, 9 Nov 2026) |
| Findings 1–4 | 1 for the owner; 2 and 3 fixed; 4 minor |
