# M10 recovery kit and device lifecycle: test steps and results

| | |
|---|---|
| **Work** | Phase 1 milestone M10: E2 (the recovery kit) and E3 (removing a device, replacing the account key, signing out) |
| **Machines** | The Mac (app and CLI), the Motorola Edge 60 Pro |
| **Decisions** | ADR-010 |
| **Date written** | 9 November 2026 |
| **Status** | **Closed** (owner, 9 November 2026) |

---

## What changed

| Area | Change |
|---|---|
| Starting an account | The account key now comes from a **recovery kit**: 12 words, shown once. Setup goes on only after the kit is saved (as a PDF or text) or the words are written down. |
| Recovering | The welcome page's **Recover my account**: scan the kit's QR code (Android) or type the 12 words. The account comes back with the same ID; the notes come back from the devices that still have them. Then remove the lost devices and make a new kit, because the old one was used. |
| Settings → Recovery kit | When the kit was made, or "Your account has no recovery kit" in red (accounts made before M10). **Make a new recovery kit…** replaces the account key: the other devices get the new key when they next connect to a device that has it; the old kit stops working. |
| Settings → Devices | **Remove…** on every other device: it stops syncing for good. "Lost or stolen?" also replaces the key. |
| Settings → Sign out | Takes this device off the account, tells the others, and erases its notes and photos; warns first about changes only this device has. |
| A removed device | Shows "This phone was removed" with **Erase it and start again**, and stops reporting to the ledger. |
| Sync format | Protocol 3 (`mirrorleaf/sync/3`): **M9 builds (0.2.5) can't sync with M10 builds (0.3.0)**, so every device needs the M10 build. |
| Ledger | Migration 3 (`account_moves`) and `POST /v1/accounts/rekey`. Rebuild the ledger before replacing a key. |

## Before you start

| Rule | Why |
|---|---|
| Rebuild the ledger: `docker compose up -d --build` in `ledger/` | Replacing the key needs migration 3. (Done on the Mac's ledger, 9 Nov 2026.) |
| Put the M10 build on every device | M9 and M10 can't sync with each other |

---

## Automated checks (Claude, 9 November 2026)

| Check | Result |
|---|---|
| `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`, Android clippy for the engine and the app | ✅ Clean |
| `cargo test --workspace` with Postgres 18 | ✅ 120 Rust tests pass. New: the recovery key derivation (pinned), replacement lists and their link, recovered devices taking names and settings from the list they meet, the kit's words (forgiving input, checksum), its QR code, text and PDF, dates; `only_here`; the ledger moving an account once (wrong link, a second replacement, the old key registering again, a device of the new key joining); **5 loopback tests**: replacing the key with a lost device left out, a device offline during the replacement getting the key from another device in either direction, a removed device refused and told, recovering with the kit and the old kit refused after a new one, signing out with the unsent-changes check |
| `cargo deny check` | ✅ advisories, bans, licences and sources ok; 35 duplicate crate versions, as before |
| `pnpm lint`, `pnpm format:check`, `pnpm -r typecheck`, `pnpm -r test` | ✅ Clean; 26 tests pass |
| Android debug APK (arm64) | ✅ Builds, Kotlin included (`DocumentsPlugin`) |

## The whole lifecycle with CLI devices (Claude, 9 November 2026)

Throwaway devices on the Mac, a throwaway ledger from this build (port 18090, no push), n0's relays.

| Step | Result |
|---|---|
| “mac” starts an account (`dev-account new`) | ✅ Prints its 12 recovery words; `status` shows “Kit made 9 November 2026” |
| “phone” and “lost” join by word code | ✅ Joined, direct |
| “mac”: `kit new --remove <lost> --out kit/` | ✅ New account `a99aaa35`; the kit printed and saved as PDF (16 KB) and text. The PDF renders as designed: words in three columns, QR code, steps, safety notes, the code to type |
| “phone” syncs with “mac” | ✅ Took the new key: now account `a99aaa35`, device list 5 |
| “lost” syncs | ✅ Refused: “this device was removed from the account”; its `status` says so; the ledger refuses it too (“this account's key was replaced…”) |
| A new device: `recover "<new words>"` | ✅ “Recovered account a99aaa35”; reached “mac”, took its device names and the note, in about 20 s (the wait was the offline “phone”) |
| Another new device: `recover "<old words>"` | ✅ Refused: “this recovery kit was replaced by a newer one: use the newest kit you made” |
| The recovered device removes “phone” | ✅ “mac” learnt it; “phone” is refused and knows it was removed |
| Sign out with an unsent note | ✅ Refused: “1 change(s) exist only on this device”. After `sync`: signed out, told 1 device, folder erased; the ledger removed its row |

Found and fixed while testing: a refusing device closed the connection before the other had read its `Hello` (a removed device sometimes never learnt why: now a 2-second grace, ADR-010 decision 17); a device removed while the key was replaced couldn't tell it had been removed (decision 15); a removed device still reported to the ledger and registered again (it no longer uses the ledger); signing out counted its own refusal as “not told”.

## The screens in a headless browser (Claude, 9 November 2026)

| Screen | Result |
|---|---|
| Your recovery kit (phone, Mac, dark) | ✅ 12 numbered words (three columns on the Mac, two on the phone), account and date, Save the PDF, Save as text, “I've written the words down”, Done only after one of them |
| Recover my account (phone) | ✅ Scan the kit's code, the words field, the device's name, Recover, Ledger… |
| Your account is back (phone) | ✅ The other devices with Remove (“Unknown device (28460260)” for one the ledger knows by ID only), Make a new recovery kit, Later |
| Settings (phone) | ✅ Remove… on each other device; Recovery kit (made date, or red when none or used); Sign out of this phone… |
| Remove a device (phone, Mac) | ✅ Remove / Cancel; “Lost or stolen?” with Remove and make a new recovery kit |
| Sign out (phone) | ✅ “3 changes exist only on this phone…” with Sign out anyway |
| This phone was removed | ✅ Erase it and start again |

## On the owner's phone and Mac CLI device (Claude, 9 November 2026)

The M10 debug APK on the Motorola Edge 60 Pro (data kept); the owner's ledger rebuilt to migration 3; the Mac CLI device “mac” on the M10 CLI. The owner's Mac app was still the M9 build, so it couldn't connect (“peer doesn't support any known protocol”): it stands in for a device that is off during the key replacement. Claude checked the app was in front before every tap.

| Step | Result |
|---|---|
| The phone opens the M10 build | ✅ Its notes; Settings: Remove… on every other device, “Your account has no recovery kit” in red |
| Remove “M9 test CLI”, then “ubuntu” | ✅ Gone from the list (“Removed M9 test CLI.”); the ledger logged both removed |
| Make a new recovery kit… → Make the new kit | ✅ The ledger: “an account moved to a new key”. The kit: 12 words, account `8a50a30c`, made 9 November 2026 |
| Save the PDF | ✅ Android's own picker opened in Downloads with “Mirrorleaf recovery kit 8a50a30c.pdf”; saved (16,954 bytes), renders as designed; the app's temporary copy was deleted. Done closed the kit; Settings: “Made 9 November 2026”, account `8a50a30c` |
| “mac” syncs | ✅ Took the new key from the phone: account `8a50a30c`, device list 8, kit made 9 November 2026, without the two removed devices |
| A new CLI device recovers with the 12 words through the owner's ledger | ✅ “Recovered account 8a50a30c”; reached the phone; all 24 notes; the real device names |
| It signs out | ✅ Told the phone, erased its folder; the ledger removed it; the phone no longer lists it |

Found and fixed: the three Kotlin plugins were registered under one Tauri plugin, so every call went to the last one (here, the permission check to `DocumentsPlugin`: “No command status found”). In M9 that meant **Scan the code** would have reached the permissions plugin; M9's scanning test wasn't run. Each is now its own Tauri plugin (ADR-010, `device.rs`).

**Left on the phone:** the kit's PDF in Downloads. The words passed through Claude's screenshots, so the owner should make a new kit of their own (test 1) and then delete this PDF.

### Quitting on the Mac (owner's observation, 9 November 2026)

Quitting from the menu bar took 3–5 s. Two fixed waits caused most of it (1 s for the window, 1.5 s for live sync). Now the window and both icons go at once, and the rest waits only for what is needed (ADR-010 decisions 20–21). Measured in the engine: going offline with nothing to send, 1.5 s → about 90 ms; closing the network endpoint about 90 ms (1.4 s only within the first seconds after starting). The owner checks how it feels.

---

## Tests for the owner

### 1. Your Mac app takes the new key, then your own kit

Install the M10 Mac app (quit Mirrorleaf from the menu bar, replace `/Applications/Mirrorleaf.app`, open it).

**Expect:** within a minute it connects to the phone and takes the new key: Settings shows **Account 8a50a30c** and “Made 9 November 2026”, as on the phone. A note made on one reaches the other.

Then make **your own** kit: Settings → **Make a new recovery kit…** → **Make the new kit** → **Save the PDF** (a Save dialog) → keep it offline → **Done**. The phone takes this key too. Then delete the earlier kit's PDF from the phone's Downloads: its words passed through Claude's screenshots, and your new kit has retired it.

### 2. Recover on a new device

On the Ubuntu machine or a spare phone: **Recover my account**, type the words (or scan the PDF's code).

**Expect:** “Your account is back”, your notes arrive; remove it again afterwards from Settings on the Mac (Remove…), and make a new kit if you used your real one.

### 3. Sign out

Optional, on a device you can set up again: Settings → **Sign out of this …** → the app restarts on the welcome page; the other devices no longer list it.

---

## Results

| Test | Result |
|---|---|
| Automated checks | ✅ Pass (Claude, 9 Nov 2026) |
| CLI lifecycle | ✅ Pass (Claude, 9 Nov 2026) |
| Screens in a headless browser | ✅ Pass (Claude, 9 Nov 2026) |
| On the owner's phone and Mac CLI device | ✅ Pass (Claude, 9 Nov 2026): removals, the account's first kit (replacing its key), the PDF saved through Android's picker, “mac” taking the new key, recovery with the words, sign-out |
| 1. The Mac app takes the new key; your own kit | ✅ The M10 Mac app took the new key (account `8a50a30c`, kit dated 9 November 2026, all 24 notes; Claude, read from its database, 9 Nov 2026). The earlier kit's PDF deleted from the phone; “mac” removed from the account (owner). **The owner's own kit**, made on the Mac: account `57f0beb4`; the phone (now 0.3.0), closed while it was made, took the new key from the Mac app one second after opening (“took the account's new key from 44587b05: now account 57f0beb4”), reconnected three times as the lists settled, then stayed connected |
| 2. Recover on a new device | Not yet run |
| 3. Sign out | Not yet run |
