# M11 protection: test steps and results

| | |
|---|---|
| **Work** | Phase 1 milestone M11: E4 (app lock and encryption at rest) and E9 (backups) |
| **Machines** | The Mac (app and CLI), the Motorola Edge 60 Pro |
| **Decisions** | ADR-011 |
| **Date written** | 10 November 2026 |
| **Status** | **Closed** (owner, 11 November 2026). Version 0.3.5 (Android versionCode 3005). Installed on the phone by Claude; the Mac app is built for the owner to install |

---

## What changed

| Area | Change |
|---|---|
| The database | **Always encrypted** (SQLCipher). Its key is in the Mac's Keychain, in Android Keystore (wrapped), or in a `database.key` file for CLI devices. The device's own secret (`device.key`) moves inside it. An existing database is converted the first time the M11 build opens it. |
| Settings → Backups | "Last backup …" or "No backup yet". **Encrypt backups** (off by default; when off: "This file isn't encrypted: keep it somewhere private, or turn on Encrypt backups."). **Back up now…** saves one file with every note and photo. **Restore from a backup…** adds back anything this device lacks; nothing is replaced. |
| Settings → App lock | **Lock Mirrorleaf** (off by default): unlock with the device's own fingerprint, face, screen lock, Touch ID or Mac password. **Lock**: immediately, or after 1, 5 (default) or 15 minutes away. Changing it asks for the unlock first. Syncing carries on while locked. |
| The main screen | A reminder 30 days after the last backup (or after a new kit), with **Back up now** and **Later** (a week). |
| After a recovery | "Have a backup? … **Restore from a backup…**" |
| Android | The app's data is no longer included in Google's backup of the phone. |
| CLI | `backup export <file> [--encrypt]`, `backup import <file> [--words "…"]` (not while `sync --watch` runs: they open the photo store) |
| Sync format | Unchanged (protocol 3): M10 and M11 builds sync together. **But a converted device can't go back to an M10 build.** |

## Before you start

| Rule | Why |
|---|---|
| **On the Mac, make the signing certificate first** (below) and build the app with it | The Keychain then trusts every build; without it, each rebuild asks for the Keychain once |
| Leave your other devices as they are | Nothing on the wire changed |

### Making the Mac's signing certificate (once)

1. Open **Keychain Access** → menu **Keychain Access → Certificate Assistant → Create a Certificate…**
2. Name **Mirrorleaf Local Signing**, Identity Type **Self-Signed Root**, Certificate Type **Code Signing** → **Create** → **Done**.
3. In the login keychain, double-click the new certificate → **Trust** → **Code Signing: Always Trust** → close (asks for your password).
4. In Terminal, `security find-identity -v -p codesigning` lists `"Mirrorleaf Local Signing"`.
5. Build with it, in `apps/mirrorleaf/src-tauri`:
   `APPLE_SIGNING_IDENTITY="Mirrorleaf Local Signing" cargo tauri build --bundles app`
   The first build asks once to let `codesign` use the key: **Always Allow**.

---

## Automated checks (Claude, 10 November 2026)

| Check | Result |
|---|---|
| `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`, Android clippy for the engine and the app | ✅ Clean |
| `cargo test --workspace` | ✅ 131 Rust tests pass. New: the database key and in-place conversion (plain → encrypted, already encrypted, an interrupted conversion restarted from its plain copy); a pre-M11 device folder converted with its identity kept, and the CLI refusing an encrypted folder whose key isn't there (as the Mac app's); the backup format (plain, encrypted, cut short, wrong key, a backup from an earlier account key and its kit's words); a backup restoring notes and photos on another device with and without encryption (photos still staged included); the lock's settings and time away (sleep while the screen shows, a closed window counting from when it closed, Android's passcode screen not counting); the backup reminder (30 days, never backed up, Later, a saved backup) |
| `cargo deny check` | ✅ advisories, bans, licences and sources ok; 35 duplicate crate versions, as before. RUSTSEC-2026-0459 (SQLite before 3.54) ignored with its reason: no SQLCipher build is fixed yet (ADR-011 decision 23) |
| `pnpm lint`, `pnpm format:check`, `pnpm -r typecheck`, `pnpm -r test` | ✅ Clean; 26 tests pass |
| Android debug APK (arm64) | ✅ Builds with OpenSSL from source (after `tools/ndk-ranlib`), Kotlin included (`LockPlugin`, `DocumentsPlugin.open`, `DatabaseKey`) |

## Converting the owner's databases, on copies (Claude, 10 November 2026)

Copies only; read-only commands (`status`, `note list`) through the CLI's open path, which runs the same conversion as the apps with a key file in place of the Keychain or Keystore. Nothing touched the real folders.

| Copy | Before | After |
|---|---|---|
| The Mac app, copied with SQLite's `.backup` | Plain SQLite; `account_secret` and the device list readable in the file | ✅ Opens as **Shahnawaz's MacBook Air (`44587b05`)**, account `57f0beb4`, device list 12, 24 notes (2 in trash), 264 changes, 20 photos. `device.key` gone (inside the database), `database.key` made. The file starts with random bytes; none of `account_secret`, `device_list`, `motorola`, `SQLite format` appears in it |
| The Mac app, raw copy of the database, its 1.7 MB `-wal` and `-shm` (almost everything was in the WAL) | As above | ✅ Same result: the WAL was folded in before converting; no `-wal` or `-shm` left |
| The phone (`run-as` copy of `files/mirrorleaf`, 4.7 MB WAL) | Plain | ✅ Opens as **motorola edge 60 pro (`d57d4595`)**, the same account, 24 notes, 264 changes, 20 photos; no plain text left |

## Backups with CLI devices (Claude, 10 November 2026)

Two throwaway accounts on the Mac. A: a note and one photo (`photo add`, 544.7 KB).

| Step | Result |
|---|---|
| A: `backup export plain` and `backup export enc --encrypt` | First run: **0 photos** (photos added with `photo add` wait in the database until a sync). Fixed (decision 11); then ✅ 5 changes and 2 photos (thumbnail and full image), 567 KB each. The note's text is readable in the plain file and absent from the encrypted one |
| B (another account): `backup import plain` | ✅ Restored: the note and its photo, thumbnail and full image |
| B: `backup import enc` with no words | ✅ Refused: "this backup was encrypted with an earlier recovery kit: type that kit's 12 words" |
| B: with another account's words | ✅ Refused: "these words are for a different account than this backup" |
| B: with A's words | ✅ Restored; nothing new (B already had it) |

## The screens in a headless browser (Claude, 10 November 2026)

| Screen | Result |
|---|---|
| Locked (phone light, Mac dark) | ✅ The logo, "Mirrorleaf is locked", how to unlock, **Unlock**; nothing of the notes on the page |
| Settings → Backups and App lock (phone; Mac with both on) | ✅ As described above; the warning line when not encrypted; the **Lock** menu when on |
| Restore a backup (phone, a backup that needs an earlier kit) | ✅ "Restore the backup from 2 November 2026?", the words field, Restore enabled only with 12 words |
| The reminder (phone light, Mac dark) | ✅ "Your last backup was 31 days ago." with Back up now and Later |

## Size (Claude, 10 November 2026)

| Build | Last measured (M7, 0.2.0) | M11 | Limit |
|---|---|---|---|
| Android arm64 release APK, file | 28.5 MB | **33.9 MB** (native library 31.4 MB) | 40 MB ✅ |
| Android arm64 release APK, download (gzip -9 estimate) | 12.7 MB | **15.1 MB** | 40 MB ✅ |

No release build was measured for M9 or M10, so the growth covers M8 to M11 together; OpenSSL's own share wasn't measured separately. The Mac `.dmg` gains nothing for encryption (SQLCipher uses CommonCrypto there); CI measures it.

## On the owner's phone (Claude, 11 November 2026)

The 0.3.5 debug APK installed over 0.3.0 with its data kept; a plain copy of the phone's data was taken first (session scratchpad). The owner's Mac app wasn't running (still 0.3.0), so the phone had no peer online. Claude checked what was in front before every tap and stopped tapping when the owner started using the phone.

| Step | Result |
|---|---|
| Open the app | ✅ "encrypted this device's database"; `device.key` gone, `database.key.wrapped` (60 bytes) made, the database starts with random bytes; cold start 1.5 s including the conversion (debug build). All notes: 22 plus 2 deleted; 264 changes. `ALLOW_BACKUP` no longer among the app's flags |
| Settings → Backups → **Back up now…** | ✅ Android's picker opened in Downloads with "Mirrorleaf backup 2026-11-11"; saved 8.8 MB (264 changes, 39 photos: 20 thumbnails, 19 full images); "Backup saved."; the app's temporary copy deleted; "Last backup 11 November 2026" |
| **Restore from a backup…** with it | ✅ "Restore the backup from 11 November 2026?" → "Backup restored." (nothing new, as expected); the copy in the app's storage deleted |
| **Encrypt backups** on, back up, restore | ✅ Restored with no words asked (this account's key). The note titles appear in the plain file (4 hits) and not in the encrypted one (0) |
| Settings → **Lock Mirrorleaf** | ❌ then fixed: the manifest lacked `USE_BIOMETRIC`, so Android refused the prompt ("Must have USE_BIOMETRIC permission"), and the checkbox stayed ticked although the lock stayed off. Both fixed and installed (`USE_BIOMETRIC: granted`); the prompt itself is the owner's test 3 |
| A push to the closed app | ✅ The phone's own connection to FCM was dropping ("Heartbeat timeout, GCM connection reset"), so two pushes never arrived and one arrived a minute late. That one started a fresh process with no screen, which **opened the encrypted database through the Keystore** ("engine open for d57d4595") and ran the sync job in 1.35 s (nothing to fetch, the Mac being off) |
| Clean-up | Encrypt backups turned off again (the default); both test backups deleted from Downloads |

---

## Tests for the owner

### 1. The Mac app converts and opens

Make the certificate (above), build, quit Mirrorleaf from the menu bar, replace `/Applications/Mirrorleaf.app`, open it.

**Expect:** your notes, as before. In `~/Library/Application Support/app.mirrorleaf`: no `device.key`; `mirrorleaf.db` no longer starts with "SQLite format 3" (`head -c 16 mirrorleaf.db`). Quit and open again: **no Keychain question**. Notes still sync with the phone.

### 2. The phone converts and still wakes

Claude installs the M11 build on the phone (with your go-ahead).

**Expect:** your notes, as before. With Mirrorleaf closed on the phone, edit a note on the Mac: it reaches the phone as before (the push-woken job opens the encrypted database).

### 3. App lock on the phone

Settings → App lock → **Lock Mirrorleaf** (asks for your fingerprint or screen lock) → Lock **Immediately**. Leave the app and come back.

**Expect:** "Mirrorleaf is locked" and the fingerprint prompt at once. Unlock with the fingerprint; next time, cancel it and use your PIN instead: it stays unlocked afterwards. While locked, a note edited on the Mac arrives (look after unlocking). Then **After 5 minutes away**: away 1 minute, no lock; away 6 minutes (screen off is fine), locked.

### 4. App lock on the Mac

The same in Settings on the Mac (Touch ID or your password).

**Expect:** switching to another app and back doesn't lock. Close the window, wait longer than the chosen time, open it from the menu bar: locked. With the window open, let the Mac sleep longer than the chosen time: locked on waking.

### 5. A backup, and restoring it

On the Mac: Settings → **Back up now…** → save. Settings shows "Last backup 10 November 2026". Turn on **Encrypt backups** and back up again. Copy the encrypted file to the phone, then on the phone: **Restore from a backup…** → pick it → **Restore**.

**Expect:** "Backup restored."; no notes doubled or changed. The plain file opened in a text editor shows bits of your notes' text; the encrypted one doesn't.

---

## Results

| Test | Result |
|---|---|
| Automated checks | ✅ Pass (Claude, 10 Nov 2026) |
| Converting the owner's databases (copies) | ✅ Pass (Claude, 10 Nov 2026) |
| Backups with CLI devices | ✅ Pass (Claude, 10 Nov 2026), after the staged-photo fix |
| Screens in a headless browser | ✅ Pass (Claude, 10 Nov 2026) |
| 1. The Mac app converts | To run when the owner installs the 0.3.5 Mac app |
| 2. The phone converts and still wakes | ✅ Converted; a push-woken job opened the encrypted database (Claude, 11 Nov 2026). Syncing with the Mac after it is installed: owner |
| 3. App lock on the phone | Needs the owner's fingerprint or screen lock; Claude checked only that the permission is now granted |
| 4. App lock on the Mac | Needs Touch ID or the Mac's password: owner |
| 5. A backup, and restoring it | ✅ On the phone, plain and encrypted (Claude, 11 Nov 2026). From the Mac to the phone: owner |
