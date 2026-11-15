# The Windows app: test steps and results

| | |
|---|---|
| **Work** | The desktop app on Windows 11 (x64 and ARM), the same as the Mac app (owner's request, 15 November 2026) |
| **Machines** | GitHub's Windows machines (build), a Windows 11 PC (owner), the Mac and the phone |
| **Decisions** | ADR-014 |
| **Date written** | 16 November 2026 |
| **Status** | Built and checked on the Mac as far as possible; waiting for the first Windows build in GitHub Actions |

---

## What's Windows-specific

| On the Mac | On Windows |
|---|---|
| Database key in the Keychain | Database key protected by Windows (DPAPI) for the signed-in user |
| App lock: Touch ID or the Mac's password | Windows Hello: PIN, fingerprint or face |
| HEIC photos through ImageIO | Through the Windows Imaging Component (needs Microsoft's HEIF Image Extensions and HEVC Video Extensions from the Microsoft Store) |
| Menu bar icon; closing the window keeps syncing | Tray icon (click: open; right-click: menu); closing the window keeps syncing |
| ⌘Q quit, ⌘W close | Ctrl+Q quit, Ctrl+W close |
| Drop files on the Dock icon | Right-click a photo or text file → Open with → Mirrorleaf (never a default app) |
| Sleep and wake | The same, through Windows' notifications |
| Local Network question | Windows Firewall may ask once to let Mirrorleaf use the network |
| `~/Library/Application Support/app.mirrorleaf` | `%LOCALAPPDATA%\app.mirrorleaf`, with `logs\mirrorleaf.log` |

Everything else is the same code as the Mac app: the screen, the editor, sync, search, folders, pairing, recovery, backups, metrics and the server.

## Automated checks (Claude, 16 November 2026)

| Check | Result |
|---|---|
| `mirrorleaf-windows` (DPAPI, Windows Hello, WIC, sleep and wake): clippy for `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`, from the Mac | ✅ Clean |
| The workspace on the Mac (clippy), Android clippy for the engine and the app | ✅ Clean: the Mac and Android builds are unchanged |
| `cargo deny check`, now with Windows on ARM | ✅ ok; duplicate versions 37 → 40 (Windows on ARM's own crates) |
| `pnpm lint`, `format:check`, `typecheck`, `test` | ✅ Clean; 30 tests pass |
| The app crate's Windows code (key, lock, tray, files opened, log file) | Compiles only on Windows: the first GitHub Actions run |

## GitHub Actions (the `Windows` workflow)

| Run | Result |
|---|---|
| (first run after the owner pushes) | |

---

## Tests for the owner (Windows 11 PC)

Download the installer from the workflow run's artifacts (`mirrorleaf-windows-x64-unsigned`, or `-arm64` for an ARM PC), unzip it and run the `.exe`. SmartScreen: **More info → Run anyway**.

### 1. Install and start

**Expect:** it installs without asking for administrator rights; Mirrorleaf opens with the welcome screen; the Mirrorleaf icon is in the tray (maybe under the ^ arrow). The Windows Firewall may ask to allow Mirrorleaf: allow it on private networks.

### 2. Join your account

Welcome → **Add this PC to my account**, with a word code from the phone or the Mac (Settings → Add a device). **Expect:** your notes and photos arrive; Settings → Devices shows the PC; on the Mac and the phone, the PC appears.

### 3. Edit and sync

Edit a note on the PC; it appears on the Mac and the phone within seconds, and the other way round. Add a photo (the photo button, or drag one onto the window outside a note).

### 4. The tray and closing

Close the window (X or Ctrl+W): Mirrorleaf stays in the tray and keeps syncing (edit on the Mac, then open the PC's window from the tray: it's there). Right-click the tray icon → **Quit Mirrorleaf** (or Ctrl+Q in the window): it quits.

### 5. Open with

In File Explorer, right-click a photo → **Open with → Mirrorleaf**. **Expect:** a new note with the photo. Double-clicking photos still opens your usual photo app.

### 6. App lock (needs Windows Hello)

Settings → **Lock Mirrorleaf** on, then leave the window for the set time (or choose Immediately). **Expect:** the lock screen; Windows Hello asks for your PIN, fingerprint or face.

### 7. HEIC photo (optional)

With Microsoft's HEIF Image Extensions installed, add a portrait iPhone photo. **Expect:** it shows upright. (Without the extensions, the app says it can't read it.)

### 8. Sleep

Put the PC to sleep for a minute and wake it: Mirrorleaf reconnects to your other devices by itself.

---

## Results

| Test | Result |
|---|---|
| Automated checks on the Mac | ✅ Claude, 16 Nov 2026 |
| GitHub Actions | |
| 1–8 | Owner |
