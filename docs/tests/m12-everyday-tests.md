# M12 everyday use: test steps and results

| | |
|---|---|
| **Work** | Phase 1 milestone M12: E5 (notes features), E6 (sync status), E7 (desktop agent), E8 (Android hardening), finding devices on the same network (mDNS) |
| **Machines** | The Mac (app and CLI), the Motorola Edge 60 Pro |
| **Decisions** | ADR-012 |
| **Date written** | 11 November 2026 |
| **Status** | Version 0.4.0. On the owner's phone and Mac (12 November 2026); phone tests by Claude below |

---

## What changed

| Area | Change |
|---|---|
| Search | Finds words anywhere in a note, by their start ("veg" finds "vegetable"), accents ignored. A note found by its text shows the words around the match. |
| Recently Deleted | "Notes here are deleted for good after 30 days." with **Empty**; a deleted note's menu and banner offer **Delete Permanently…**. Both ask first. |
| Folders | Folders inside folders, option A (owner, 11 Nov 2026): the chips show one level; choosing a folder steps into it, with a breadcrumb back out ("‹ All Notes › Work › Clients") and the chips "All of Clients", its folders and +. The back gesture or Esc goes up one level. **New Folder Inside** and **Move Folder to…** in a folder's menu; a folder shows the notes of the folders inside it, each labelled with where it lives; **Move to…** lists folders by level. |
| Photos | Full images are fetched ahead while the app is open on Wi-Fi (Android) or always (Mac). |
| Settings → Sync | "last seen" for offline devices; "a different version: update both to sync"; "connected through the relay" with a VPN note; "N changes are only on this phone so far"; **Find my devices on this Wi-Fi without internet** (on); when a device is on this network but reached only through the relay, what to change. |
| Main screen | "Mirrorleaf on <device> is a different version and can't sync with this one. Update both to the latest version." |
| Settings → This phone (Android) | Battery guidance for the phone's maker, with **Open** to Android's settings for Mirrorleaf; **Keep syncing while charging on Wi-Fi** (off). |
| Mac | Drop photos or text files on the window (outside a note) or on the Dock icon: they become a new note. The app now asks macOS for the local network with an explanation. |
| Android | Granting Nearby devices takes effect at once; the notification shows the Mirrorleaf leaf. Setup's permission step says a locked, sleeping phone may catch up only when opened. |
| Sync without internet | Each device remembers the others' last addresses on the home network and tries them on every dial, and listens on the same port after a restart (ADR-012 decision 25; added by the owner, 12 Nov 2026), so devices that have connected directly once find each other without internet even where the Wi-Fi drops mDNS. |
| Sync format | Unchanged: M12 builds sync with 0.3.5. The database moves to schema 5 (the search index). |

---

## Automated checks (Claude, 11 November 2026)

| Check | Result |
|---|---|
| `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`, Android clippy for the engine and the app | ✅ Clean |
| `cargo test --workspace` | ✅ 139 Rust tests pass. New: search (whole notes, prefixes, all words, snippets, title-only matches, FTS5 syntax typed as words, accents, Hindi, small steps, edits, renames, deletions); deleting for good and the purge (incl. an offline device's later edit losing to the deletion); nested folders (inside itself refused, deleting a folder with folders inside, crossed moves from two offline devices, a folder made inside one deleted elsewhere); the engine's search, purge and Empty trash; the reminder; the lock |
| `cargo deny check` | ✅ ok; 35 duplicate crate versions, as before |
| `pnpm lint`, `pnpm format:check`, `pnpm -r typecheck`, `pnpm -r test` | ✅ Clean; 30 tests pass (new: full-text hits in the list, snippets, folder views with inner folders, counts, paths, the indented folder list, "last seen", battery steps) |
| Android debug APK (arm64) | ✅ Builds (`SeedWorker`, the plugin's battery and seeding calls, the multicast lock, the notification icon) |

### Finding devices on the same network, by hand (Claude, 11 November 2026)

`cargo test -p mirrorleaf-net --test lifecycle -- --ignored local_network`: two devices of one account on the Mac, given nothing but each other's IDs (no addresses, no relay, no discovery server).

| Run | Result |
|---|---|
| 1 | ❌ Timed out at 30 s, although each saw the other's announcement within a second: the dial sat on addresses that didn't answer |
| 2–9 | ✅ Found and synced in 4–12 s; the seen list held the other device; stopping ended it |

Found and fixed while testing: the watcher task kept a copy of the mDNS service, so stopping wouldn't have ended the announcements; devices that live sync looked up itself never reached the "seen" list (the service reports only devices nobody asked for).

## The screens in a headless browser (Claude, 11 November 2026)

| Screen | Result |
|---|---|
| Search "garl" (Mac) | ✅ "Vegetable market" with "…Onions and **garlic** left for Sunday…" in place of its preview |
| Recently Deleted (phone) | ✅ The 30-day line and **Empty** |
| Settings → Devices (Mac) | ✅ "connected through the relay", "offline, last seen today at 19:25, 12 changes behind", the on-this-network hint, the toggle, "3 changes are only on this Mac so far" |
| Settings → This phone (Android, Samsung steps) | ✅ "Background sync may be held back" with the steps and **Open**; the charging toggle |
| Notes in nested folders (Mac) | ✅ Labels "Work · Pinned", "Work › Clients › Acme" |
| Option A: inside Work (Mac), inside Clients (phone), back from Clients (phone, dark) | ✅ "‹ All Notes › Work" with "All of Work 2", "Clients 1", "+", notes labelled "Clients › Acme"; "‹ All Notes › Work › Clients" with "All of Clients", "Acme"; the back gesture returns to Work |

## Size (Claude, 12 November 2026)

| Build | M11 | M12 | Limit |
|---|---|---|---|
| Android arm64 release APK, file | 33.9 MB | **35.1 MB** | 40 MB ✅ |
| Android arm64 release APK, download (gzip -9 estimate) | 15.1 MB | **15.6 MB** | 40 MB ✅ |

## On the owner's phone and Mac (Claude, 12 November 2026)

The owner installed the signed 0.4.0 Mac app. Claude installed the 0.4.0 debug APK on the phone over 0.3.5 (data kept; an encrypted copy of its data taken first), and drove the screen through the web view's debugger and adb, checking the app was in front before each tap. The ledger (stopped) was started with OrbStack.

| Step | Result |
|---|---|
| Opening 0.4.0 | ✅ Schema 5; all notes. **The phone reached the Mac app directly** ("connected to 44587b05 via direct"): the relay-only connection of M9 finding 1 is gone with the Mac's Local Network question |
| Search on the real notes (engine) | ✅ "walk" finds "Lake District walks" inside a link and "Walking boots"; "compass", "cash in" (both words), nothing for "garl" or "xyzzy" |
| Search on the screen | ✅ "compass" shows "Packing list M6" with the word marked |
| Folders, option A | ✅ + made "M12 Work" and stepped into it ("‹ All Notes › M12 Work", "All of M12 Work 0", +); + inside made "M12 Clients" and stepped in; **Back went up one level**; the breadcrumb's "All Notes" went up two at once; the top level showed only "M12 Work". Its menu: Rename Folder, New Folder Inside, Move Folder to…, Delete Folder… ("…and the folders inside it…"); both deleted |
| Deleting for good | ✅ Two test notes trashed; in Deleted, "Notes here are deleted for good after 30 days. Empty"; each row's menu "Restore / Delete Permanently…", the confirmation, and both gone from the engine. The owner's two deleted notes untouched (Empty not used) |
| Settings | ✅ The Mac "connected, direct"; **Find my devices on this Wi-Fi without internet** on; This phone: "Background sync may be held back" with the Motorola steps; **Open** reaches Android's App info for Mirrorleaf ("App battery usage") and Back returns |
| Keep syncing while charging | ✅ On while charging on Wi-Fi: the job ran at once ("seeding while charging"; Android's job waits on network and charging); with the app in the background, live sync stayed on and connected to the Mac directly. Off: the job was cancelled and live sync stopped. **Left off** (the default) |
| Nearby devices granted in Settings | ✅ Revoked, the app went through the relay; **Allow** in Settings → "restarted live sync on a new endpoint" → direct, without restarting the app (M4 finding 1) |
| Finding devices on the same network (mDNS) | ❌ **Not on this Wi-Fi.** The Mac app announces itself (seen by macOS's own Bonjour browser), the phone announces too, but neither receives the other's. Measured: of 1,000 multicast packets from the Mac, none reached the phone (its Wi-Fi counted 43 packets, the same as idle), while 1,000 unicast packets did (1,024 counted). **This network doesn't carry multicast between the two**, so on it the devices can't find each other without internet. The code works where multicast passes (the by-hand test). |
| Notification icon | ✅ Packaged (`drawable/ic_stat_mirrorleaf`) and drawn as the notepad and leaf. The live notification wasn't seen: the phone's connection to Google's push service kept dropping (31 reconnects), as on 11 November |

Found and fixed on the phone:

1. **Stopping live sync could take 10–16 s on the phone**, almost all of it closing the network endpoint gracefully (Iroh waits for every connection's goodbye). When the charging job ended with the app in the background, Android froze the app (10 s later) before the stop finished; it finished only when the app next ran. Now the close waits at most 2 s (`CLOSE_WAIT`); the other devices see a time-out and redial. Measured after the fix: stops of 0.8–2.1 s, and the charging job's end completes in 2 s, before the freeze. The stop's log line now gives its parts ("sending, ledger, closing").
2. While Nearby devices was revoked, sending mDNS failed with "Operation not permitted", as it should.

### Remembered home-network addresses (Claude, 12 November 2026)

Added after the mDNS result above, at the owner's request (ADR-012 decision 25).

| Check | Result |
|---|---|
| `cargo fmt`, clippy (Mac and Android), `cargo test --workspace` | ✅ Clean; 141 Rust tests pass. New: `a_remembered_home_address_is_dialled_without_any_lookup` (two devices with no relay, no discovery and no mDNS; one has only the other's remembered address and syncs) and `a_device_listens_on_the_same_port_again` |
| Phone (debug APK, same version 0.4.0), Mac app still the earlier 0.4.0 build | ✅ On connecting: "44587b05 is at 192.168.1.13:58287 on this network", then "connected to 44587b05 via direct" |
| The phone's port across three restarts of the app | ✅ UDP 46493 each time (plus 5353 for mDNS); direct to the Mac each time |
| Without internet | Owner, test 3 (needs the router's internet off). The Mac app needs the new build first, so its port stays the same across restarts |

---

## Tests for the owner

### 1. Search and deleting for good

Type part of a word that appears only inside a note (not its title). **Expect:** the note, with the words around the match. Then delete a test note, open Recently Deleted, **Delete Permanently…** → it leaves the phone and the Mac.

### 2. The Mac: drops and the local network

Drop a photo on the Mac app's note list (not into a note), and another on the Dock icon. **Expect:** a new note with it each time. On first start macOS may ask to let Mirrorleaf find devices on the local network: **Allow**. Settings → Devices then shows the phone "connected, direct" on the same Wi-Fi.

### 3. Sync with the internet off

First, with the internet on, install the new Mac app and open Mirrorleaf on both, so each connects directly once and remembers the other's address. Then: Mac and phone on the same Wi-Fi, the router's internet unplugged (or its WAN off), **mobile data off on the phone** (otherwise Android may send traffic through mobile data when the Wi-Fi has no internet). Quit and reopen Mirrorleaf on both, then edit a note on one. **Expect:** it reaches the other within seconds, "connected, direct". On a network that carries multicast, mDNS also finds devices that have never connected there before.

### 4. The phone's settings

Settings → This phone: the battery line for Motorola (or "Android lets Mirrorleaf sync in the background without limits" if exempt). **Open** reaches Android's page for Mirrorleaf. Turn on **Keep syncing while charging on Wi-Fi**, plug the phone in on Wi-Fi with the app closed. **Expect:** within about 15 minutes, `adb logcat -s mirrorleaf` shows "seeding while charging".

### 5. Nested folders

Make Work, then (from its menu, or + inside it) Clients, then Acme inside Clients; put a note in each. Open Work. **Expect:** the breadcrumb, "All of Work 3", "Clients"; all three notes, the inner ones labelled "Clients" and "Clients › Acme". Step into Clients, then use the back gesture (phone) or Esc (Mac): back in Work. On the phone still on 0.3.5, if any: the folders show flat.

---

## Results

| Test | Result |
|---|---|
| Automated checks | ✅ Pass (Claude, 11 Nov 2026) |
| mDNS by hand | ✅ Pass 8 of 9 (Claude, 11 Nov 2026) |
| Screens in a headless browser | ✅ Pass (Claude, 11 Nov 2026) |
| 1. Search and deleting for good | ✅ On the phone (Claude, 12 Nov 2026) |
| 2. The Mac: drops and the local network | Local network ✅ (the phone connects directly); drops: owner |
| 3. Sync with the internet off | mDNS ❌ on this Wi-Fi (no multicast between the Mac and the phone). Remembered addresses (decision 25): ✅ remembered on the phone (Claude, 12 Nov 2026); the internet-off run: owner |
| 4. The phone's settings | ✅ (Claude, 12 Nov 2026) |
| 5. Nested folders | ✅ On the phone (Claude, 12 Nov 2026); on the Mac: owner |
