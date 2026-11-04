# M5 Editor and desktop app: setup and test steps

| | |
|---|---|
| **Milestone** | M5 — Editor and desktop app (implementation plan §6) |
| **Devices** | The Mac (the app) and the Ubuntu machine (the Linux build of the app, as the second desktop peer). A phone hotspot for the cross-network rows. |
| **Covers** | M5's "done when" items: a rich-text note and a photo sync between two desktop apps on the LAN and across networks; a concurrent offline edit made through the UI merges; the agent's memory with the window closed. Also the agent's behaviour, flush-on-close and the adaptive layout. |
| **Date written** | 4 November 2026 |

---

## Before you start

### On the Mac

| Tool | Version | Check |
|---|---|---|
| Rust, Node 24.22, pnpm 12.16.1 | as in the README | `cargo --version`, `node -v`, `pnpm -v` |
| Tauri CLI | 2.12.4 | `cargo tauri --version` |
| JS dependencies | from the lockfile | `pnpm install` in the repo |

### On the Ubuntu machine

Download the `.deb` from the CI run's **mirrorleaf-linux-x86_64-deb** artifact (job "Linux app"), then:

```bash
sudo apt install ./Mirrorleaf_*_amd64.deb
```

Then start **Mirrorleaf** from the app launcher.

The `.deb` pulls in WebKitGTK and the tray library. GNOME needs the "AppIndicator" extension to show the tray icon (Ubuntu's own session has it).

### Rules that apply to every step

| Rule | Why |
|---|---|
| Paste commands **without** `# comments` | zsh treats `#` as an argument |
| On the Mac, run the app with `cargo tauri dev` in `apps/mirrorleaf/src-tauri` for tests 1–5 and 8–9; use the release `.app` for test 6 (memory) and test 7's start-at-login step | Development builds print the engine's log; a login item should point at the release app, not a development build |
| The app's data is in `~/Library/Application Support/app.mirrorleaf` (Mac) and `~/.local/share/app.mirrorleaf` (Ubuntu). `mirrorleaf-cli --dir <that folder> status` reads it while the app runs | Quick evidence without the screen |
| To start again from nothing, quit the app (menu-bar icon → Quit Mirrorleaf) and delete that folder | |

---

## 1. Set up the account

1. **Mac:** start the app. Open **Settings** (⌘, or the sliders button above the search field) → **Start a new account**.
2. **Mac:** Settings → **Add a device…**. A ticket appears; **Copy** it and get it to the Ubuntu machine (a note on another device, a message to yourself, or a USB stick). It holds the account key: delete it afterwards.
3. **Ubuntu:** Settings → paste the ticket under **Join with a ticket** → **Join**. Expected: "Joined. Synced with the inviting device via direct…".
4. Both: the status line above the search field reads **Syncing with <the other device>**, and Settings → Devices lists both, the other one "connected, direct".

## Test 1 — A rich-text note syncs on the LAN (done when, item 1)

1. **Mac:** ⌘N. Type a title on the first line, then on the next lines use **Aa** (Title, Heading, Body; bold, italic, underline, strikethrough), `/` → Checklist (tick one item), `/` → Bulleted list, `/` → Quote, `/` → Divider.
2. **Ubuntu:** the note appears in the list within about 2 s with its title and preview. Open it: the formatting, the ticked item and the list match.
3. **Ubuntu:** add a line. **Mac:** it appears in the open note without reopening it.

Record the time from typing to it showing on the other screen.

## Test 2 — A photo syncs on the LAN (done when, item 1)

1. **Mac:** in the note, the photo button (or `/` → Photo) → pick a 12 MP JPEG from the M2 samples; then drag a HEIC file from Finder into the note.
2. **Ubuntu:** both photos show as thumbnails within a few seconds, then sharpen once the full images arrive (opening the note asks for them).
3. Check sizes: `mirrorleaf-cli --dir ~/.local/share/app.mirrorleaf photo list` on Ubuntu shows both, "full image here".

## Test 3 — Across networks (done when, item 1; plan §7.1 rows 2–3)

Put the Ubuntu machine on the phone's hotspot (row 2), then on another Wi-Fi if one is available (row 3). Repeat a short version of tests 1 and 2 (one text edit, one photo). For each row record: the connection type in Settings → Devices (direct or relay), the time to the first text edit, and the time for one photo.

## Test 4 — A concurrent offline edit through the UI merges (done when, item 2)

1. Both apps open on the same note. **Ubuntu:** turn Wi-Fi off.
2. **Mac:** edit the second paragraph (add words in its middle) and add a checklist item. **Ubuntu:** edit the same paragraph (different words in the same place), delete another line, and add a photo.
3. **Ubuntu:** Wi-Fi back on. Within about 30 s (the redial interval) both show the same note: both sets of words in the paragraph (Yjs puts one after the other), the deleted line gone, the new item and photo there. No text typed on either side is lost.

## Test 5 — Flush-on-close

1. **Mac:** type a sentence and press ⌘W within a second of the last key.
2. **Ubuntu:** the full sentence arrives. (The screen sends what it holds when the window closes, and the engine stores it at once; the agent keeps syncing.)
3. **Mac:** type a sentence, then quit from the menu-bar icon straight away. The sentence arrives on Ubuntu before the Mac app exits (it waits up to about 7 s for that).

## Test 6 — The agent's memory with the window closed (done when, item 3)

Use the release `.app` (CI's `.dmg`, or `cargo tauri build --bundles app`).

1. Open the app, open a note with photos, then close the window (⌘W). The Dock icon goes away; the leaf stays in the menu bar.
2. Wait 10 minutes (plan §7.3), with the Ubuntu peer connected.
3. Activity Monitor → Memory: note **Mirrorleaf** and every **com.apple.WebKit.\*** process whose "Responsible process" is Mirrorleaf (View → Columns → enable it if needed). Total them. CPU % should be close to 0.
4. Gate: ≤ 40 MB (the M5 target; the assessment's ceiling is 50 MB).

## Test 7 — The agent

| Step | Expected |
|---|---|
| Close the window | The app leaves the Dock; the menu-bar leaf stays; the status line in its menu says who it is syncing with |
| Menu-bar leaf → Open Mirrorleaf | The window comes back, on the note list |
| Launch the app again from Finder while it runs | No second copy: the existing window comes forward |
| Release build, first start | Settings → This computer → "Open at login" is **unticked**, and System Settings → General → Login Items doesn't list Mirrorleaf (off by default, owner's decision of 4 November 2026) |
| Tick "Open at login" | System Settings → General → Login Items lists Mirrorleaf. Log out and in: the leaf appears with no window |
| Untick it, quit, start again | It stays unticked, and Login Items no longer lists Mirrorleaf |
| Put the Mac to sleep for a minute, then wake it | Before sleep the log shows "the Mac is going to sleep" and the ledger shows the Mac offline (if one is used); after waking, "the Mac woke up" and the devices reconnect within about 30 s |
| Menu-bar leaf → Quit Mirrorleaf, or ⌘Q | The app exits within a few seconds |

## Test 8 — Notes, folders and search

| Step | Expected |
|---|---|
| Start the app and type `pack` without clicking anything | The letters go into the search field; the stream filters to notes whose title or preview matches, matches highlighted; the top match shows beside the stream |
| ↓ / ↑, then Enter | Moves through the matches; Enter puts the cursor in the chosen note |
| Esc, then type `Packing list` and Enter (no note has that title) | A note is created starting with the line "Packing list" and opens with the cursor after it; in a folder chip, it goes into that folder |
| ⌘K (Ctrl+K on Windows) while writing | The cursor goes back to the search field, text selected |
| + chip → name it | A folder chip appears and is selected; ⌘N or Enter on a new title creates the note inside it |
| Right-click a note → Move to → another folder | It moves; under All, its tag shows the folder |
| Right-click a note → Pin Note | It moves to the top with a PINNED tag; the Pinned chip shows it |
| Right-click a folder chip → Rename / Delete Folder… | Deleting asks first, then moves its notes to Deleted |
| Deleted chip → a note | Read-only, with "Restore it to edit"; restoring puts it back (outside any folder if its folder was deleted) |
| Search | Matches titles and previews (full-text search of the whole note is E5) |
| All of the above on the Ubuntu peer | The same folders, pins and trash appear there |

## Test 9 — One layout for desktop, tablet and phone

Drag the Mac window narrower.

| Width | Expected |
|---|---|
| 640 px and wider (desktop, tablet) | The stream and the note side by side (the stream is a little narrower below 960 px) |
| Under 640 px (phone) | The stream fills the window; a note opens as a sheet over it, closed with its ⌄ button, a tap above it, or Esc |

---

## Results

### Checked by Claude on the Mac (4 November 2026)

No screen access was available to Claude, so the window itself wasn't driven; everything under it was.

| Check | Result |
|---|---|
| Rust: clippy (workspace, `-D warnings`), core tests (26, including the yrs ↔ Yjs fixture tests), engine tests (3: the editor bridge batches typing into one change, hears only other devices, and shows the best photo copy held), convergence and loopback tests | Pass |
| JS: svelte-check, tsc, ESLint, Prettier, 20 tests (editor: photo addresses, title and preview, slash menu, Yjs ↔ yrs fixtures; app: dates, note list) | Pass |
| The screen in a headless browser with sample data, at 1280, 834 and 390 px wide, light and dark | Three, two and one pane as designed (after fixing checklist styling found this way) |
| The app on the Mac (development build) joined a throwaway account with a CLI peer; the peer wrote a note and added a 12 MP JPEG and a HEIC | Note received within seconds; both thumbnails received; connection relay, then direct |
| A note written on the app's device reached the peer through the app's live sync | Received, direct |
| Memory, development build, window open | App process 39 MB; WebKit's page process about 43 MB, plus its graphics and network processes |
| Memory, release build (no link-time optimisation), started as at login with `--background`, so no window ever opened; live sync connected to the peer | **14 MB** physical footprint (Activity Monitor's figure) at 30 s, 2 min and 5 min; 0–0.2% CPU; 21 threads; no WebKit processes. Resident size reads 86 MB, almost all of it macOS framework code shared with other apps. |
| The same release process after its window was opened (a second launch, which single-instance turns into "show the window") | App 25 MB, 27 threads; WebKit's page process 51 MB, graphics 12 MB, network 7 MB: about 95 MB in all while the window is open. The second launch exited at once. |
| For comparison: the sync engine alone (release CLI `sync --watch`, idle) | 7.9–8.8 MB |
| Installer size | `.app` 26.8 MiB without link-time optimisation; as a compressed disk image 12.1 MiB (gate 25 MB) |
| Start at login during these tests | Not registered |

### On the Mac and the Ubuntu machine (owner)

| Test | Result | Notes |
|---|---|---|
| 1 Rich text, LAN | | |
| 2 Photo, LAN | | |
| 3 Across networks (row 2, row 3) | | |
| 4 Concurrent offline edit | | |
| 5 Flush-on-close | | |
| 6 Agent memory, window closed | | |
| 7 Agent | | |
| 8 Notes, folders, search | | |
| 9 Layouts | | |
