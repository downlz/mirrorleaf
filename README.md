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
| `crates/ffi` | The engine the apps drive, with its uniffi API for native code |
| `crates/cli` | Headless peer for tests and measurements |
| `crates/uniffi-bindgen` | Generates the engine's Kotlin bindings during the Android build |
| `apps/mirrorleaf` | The Tauri app: the notes screen (Svelte, one adaptive layout for desktop, tablet and phone), the desktop agent, and the Android project with its Kotlin background code |
| `ledger/` | Sync-status service (axum and Postgres) |
| `packages/editor` | TipTap editor and its bridge to Rust |
| `vendor/ndk-context` | A patched copy of `ndk-context` (see its `Cargo.toml`) |
| `tools/` | Measurement scripts: `push-matrix.py`, the push-to-wake matrix (plan §7.2); `mem-trace.py`, the Android app's memory over time (plan §7.3) |

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

## Build the Android app

You need a physical Android phone with USB debugging on; Android 17 is the target. On the Mac:

1. JDK 17 (`brew install openjdk@17`, or Temurin 17) and the Android command-line tools with platform 37 and NDK 29.0.14206865. Set `JAVA_HOME`, `ANDROID_HOME` and `NDK_HOME` in your shell profile.
2. `rustup target add aarch64-linux-android`
3. The Tauri CLI: `cargo install tauri-cli --version 2.12.4 --locked`
4. Cap Gradle's memory: put `org.gradle.jvmargs=-Xmx2g -Dfile.encoding=UTF-8` in `~/.gradle/gradle.properties`.
5. Copy your Firebase project's `google-services.json` to `apps/mirrorleaf/src-tauri/gen/android/app/`. It is gitignored: never commit it. The app ID registered in Firebase must be `app.mirrorleaf`.
6. Build and install:

   ```bash
   cd apps/mirrorleaf/src-tauri
   MIRRORLEAF_DEFAULT_LEDGER=http://<mac address>:8080 cargo tauri android build --debug --target aarch64 --apk --ci
   adb install -r gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
   ```

The build bundles the notes screen first (`pnpm build`), so run `pnpm install` once beforehand. The first build also compiles OpenSSL from source for the encrypted database (M11; it needs `NDK_HOME`, through `tools/ndk-ranlib`). `MIRRORLEAF_DEFAULT_LEDGER` is the ledger a new phone asks about a word code before it belongs to an account (optional: a scanned QR code carries the ledger's address, and the joining sheet has a **Ledger…** field). On first start the app offers **Start a new account**, **Add this phone to my account** (scan the other device's code, or type its words) and **Recover my account** (from M10). Settings shows the devices, adds new ones, and sets the ledger. Pushes from the ledger wake the app in the background; follow it with `adb logcat -s mirrorleaf`. The step-by-step test is in [docs/tests/m4-android-tests.md](docs/tests/m4-android-tests.md). The full Android screen is M6.

## Run the Mac app

Needs the Mac setup above and the Tauri CLI (`cargo install tauri-cli --version 2.12.4 --locked`).

```bash
cd apps/mirrorleaf/src-tauri
cargo tauri dev            # development: the screen reloads from Vite
APPLE_SIGNING_IDENTITY="Mirrorleaf Local Signing" cargo tauri build --bundles app   # a release .app in target/release/bundle/macos/
```

Sign every build with the same certificate: the database key is in the Keychain, which trusts the app by its signature. Make the certificate once in Keychain Access (**Certificate Assistant → Create a Certificate…**, name **Mirrorleaf Local Signing**, Self-Signed Root, Code Signing; then set its Trust to **Code Signing: Always Trust**); `security find-identity -v -p codesigning` should list it. An unsigned or ad-hoc build works too, but the Keychain asks once after each rebuild.

- **First start:** choose **Start a new account**, or **Add this Mac to my account** and type the words another device shows under Settings → Devices → Add a device (or paste its code as text). Both devices show the same four-digit number, and the other device asks before letting this one in. The step-by-step test is in [docs/tests/m9-identity-tests.md](docs/tests/m9-identity-tests.md).
- **Finding and creating:** just start typing: the search field finds notes by any word in them (by its start: "veg" finds "vegetable"; accents ignored), Enter opens the top match, and if no note has that title Enter creates one. ⌘K (Ctrl+K) goes back to the search field from anywhere; Esc in a note does too.
- **Folders:** the chips under the search field (+ adds one). Choosing a folder steps into it: a breadcrumb leads back out (or Esc), and the chips show the folders inside it. Right-click a folder chip for **New Folder Inside**, **Move Folder to…**, rename and delete.
- **Deleting:** deleted notes wait in **Deleted** for 30 days, then go for good; **Empty** there, or **Delete Permanently…** on a note, does it at once, on every device.
- **Dropping files:** photos or text files dropped on the note list (not into a note), or on the Dock icon, become a new note.
- **Notes:** the first line of a note is its title. Type `/` for headings, lists, checklists, quotes, dividers and photos; **Aa** in the toolbar for paragraph styles and bold, italic, underline and strikethrough. Drop or paste photos into a note, including HEIC. Right-click a note or a folder chip (long-press on a touch screen) for pin, move, rename and delete.
- **The agent:** closing the window leaves Mirrorleaf syncing from the menu bar (the leaf icon); its menu shows who it is syncing with and has **Quit Mirrorleaf**. "Open at login" is off until you turn it on in Settings → This computer (it stays off by default until the app has proved reliable). The Mac going to sleep tells the ledger this device is offline; waking it reconnects.
- **Same network, no internet:** Settings → **Find my devices on this Wi-Fi without internet** (on by default) lets devices on one network find each other with no internet. Some Wi-Fi networks don't carry the announcements it uses (multicast); there, devices that have connected directly once still find each other, because each remembers the others' last address on the home network. macOS asks once to let Mirrorleaf use the local network: allow it, or the phone reaches the Mac only through the relay (Settings says so).
- **Data:** `~/Library/Application Support/app.mirrorleaf`. The database is encrypted (M11) with a key kept in the Keychain, so `mirrorleaf-cli` can't open this folder; it refuses rather than touch it.
- **Protection:** Settings → **App lock** (off by default) asks for Touch ID or the Mac's password after the time you choose away. Settings → **Backups** saves every note and photo in one file (**Encrypt backups** is off by default) and restores one.
- **Logs:** `cargo tauri dev` prints the engine's log; set `RUST_LOG` to change what it shows.

CI builds an unsigned Apple silicon `.dmg` (checked against the 25 MB limit) and a Linux `.deb` for the second desktop peer. On Ubuntu, install the `.deb` with `sudo apt install ./Mirrorleaf_*.deb`; the menu-bar icon needs `libayatana-appindicator3-1`. The M5 test steps are in [docs/tests/m5-desktop-tests.md](docs/tests/m5-desktop-tests.md).

## Try the headless peer

`mirrorleaf-cli` is a device without a screen, used for testing sync. Each data folder is one device (`--dir`, default `~/.mirrorleaf-dev`). Build it with `cargo build -p mirrorleaf-cli`, or download a release build from a CI run's **Artifacts** (macOS on Apple silicon, and Linux for Ubuntu 24.04 or newer).

On the first device:

```bash
mirrorleaf-cli init --name laptop
mirrorleaf-cli dev-account new         # starts an account and prints its 12 recovery words
mirrorleaf-cli note new "Groceries" --text "milk"
mirrorleaf-cli pair invite             # shows a QR code, its text, and words; keep it running
```

On the second device, use the words (with a ledger) or the QR code's text (`MLP1…`):

```bash
mirrorleaf-cli init --name ubuntu
mirrorleaf-cli pair join 7-acoustic-planet-river --ledger http://<ledger>:8080
mirrorleaf-cli note list
mirrorleaf-cli sync --watch
```

Both print the same four-digit number, and `pair invite` asks before letting the device in (`--yes` lets it in without asking, for scripts). A code works once, for 10 minutes. `pair invite` keeps syncing until Ctrl-C, so stop any other `sync --watch` for that device first. A phone or a Mac app can join a CLI device's account the same way, or show a code for a CLI device to join.

Edits made in another terminal (`note edit <id> --append "eggs"`) reach the other device within seconds. `sync --watch` reports each connection as **direct** or **relay**, with the time and bytes each sync took. `status` shows the devices and how many changes this device holds from each.

> Phase 0's `dev-account ticket` and `dev-account join` still work between CLI devices, for tests. A ticket contains the account key: share it only with your own devices.

### Recovery kit, removing devices, signing out

```bash
mirrorleaf-cli kit new --out ~/Desktop            # replaces the account key; prints and saves the new kit (PDF and text)
mirrorleaf-cli device remove 0fce                 # removes a device for good (an ID prefix)
mirrorleaf-cli device remove 0fce --replace-key   # it was lost or stolen: also replace the key
mirrorleaf-cli recover "word1 word2 … word12" --ledger http://<ledger>:8080   # on a new device
mirrorleaf-cli sign-out                           # off the account, then this folder is erased
```

The recovery words derive the account key, so a new kit always comes with a new key: the other devices get it the next time they connect to a device that has it, and the old kit stops working (the ledger refuses the old key). `sign-out` refuses while some changes exist only on this device; `--force` signs out anyway.

### Backups

```bash
mirrorleaf-cli backup export ~/Desktop/notes.mirrorleaf-backup            # every note and photo, not encrypted
mirrorleaf-cli backup export ~/Desktop/notes.mirrorleaf-backup --encrypt  # encrypted with a key from the account key
mirrorleaf-cli backup import ~/Desktop/notes.mirrorleaf-backup            # adds what this device lacks; replaces nothing
mirrorleaf-cli backup import old.mirrorleaf-backup --words "word1 … word12"   # encrypted before a newer kit: that kit's words
```

Both open the photo store, so stop `sync --watch` for that device first. A CLI device keeps its database key in `database.key` beside the database (the apps use the Keychain or Android Keystore); keep the two together.

### Photos

Importing a photo resizes it to at most 2048 px and 600 KB (JPEG), and makes a 320 px thumbnail. Its EXIF data, including the location, is kept in the full image. HEIC photos can be imported on macOS only; the Linux build can't decode them, but it syncs them like any other photo.

On the first device, while `sync --watch` runs in another terminal:

```bash
mirrorleaf-cli photo add <note> IMG_0001.HEIC IMG_0002.jpg
```

The other devices fetch the thumbnails within seconds. Full images move only when asked for:

```bash
mirrorleaf-cli photo list
mirrorleaf-cli photo fetch <photo>
```

`photo fetch` needs `sync --watch` running on the same device. To see what arrived, start the watcher with an export folder:

```bash
mirrorleaf-cli sync --watch --export ~/mirrorleaf-photos
```

Every photo the device holds is saved there, and each new one as it arrives (thumbnails as `<photo>-thumbnail.jpg`). Photos are kept in the data folder's `blobs` folder, which one process can use at a time, so `photo export` works only while `sync --watch` isn't running.

### Cross-network test

Run the two devices on different networks and note, for each case, whether the connection is direct or relay, how long a text edit takes to arrive, and how long a photo's thumbnail and full image take:

| Case | Device A | Device B |
|---|---|---|
| 1 | Home Wi-Fi | Same Wi-Fi |
| 2 | Home Wi-Fi | A laptop tethered to a phone on mobile data |
| 3 | Home Wi-Fi | A different Wi-Fi (another home or an office) |
| 4 | A network that blocks UDP, if available | Any |

Case 4 should still sync, through a relay.

## The ledger (sync status)

The ledger is a small service that knows which devices an account has, how many changes each holds, which are online, and how to wake a phone. It never sees note content, titles or photos. Every request is signed with the device's key.

Run it with Docker Compose (Postgres 18 is included):

```bash
cd ledger
cp env.example .env
docker compose up -d --build
```

Set `POSTGRES_PASSWORD` in `.env` first. Without a Firebase service-account key (`FCM_SERVICE_ACCOUNT`), the ledger decides pushes and only logs them.

The ledger is published on this machine only (`127.0.0.1:8080`). For a phone on the same Wi-Fi during development, set `LEDGER_PUBLISH=8080` in `.env`. To serve it over HTTPS, set `LEDGER_DOMAIN` to a domain whose DNS points at the machine (ports 80 and 443 open) and start Caddy with it:

```bash
docker compose --profile https up -d --build
```

Caddy gets and renews the certificate itself (with `LEDGER_DOMAIN=localhost` it uses its own local certificate authority, for testing on this machine) and logs no IP addresses. The ledger limits each device to 30 signed requests a minute and new accounts to 20 a minute across the ledger, and an account to 10 devices. The two rates can be changed in `.env`; 0 turns a limit off, for example while every device registers again after the database was replaced. `GET /health` answers `ok` only while the database does, for an uptime monitor. `docker compose stop` lets requests in progress finish.

Then register each device:

```bash
mirrorleaf-cli ledger register http://<ledger-host>:8080
mirrorleaf-cli ledger status
```

From then on, `sync` and `sync --watch` report to it: when changes are made (at most every 30 seconds), every 5 minutes while online, and when going offline. `ledger wake` asks it to wake the device holding what this one lacks. With `--wait`, it then waits as the app does: if that device's changes haven't arrived within 90 seconds, or no push can reach it, it prints "Open Mirrorleaf on <name> to sync". A CLI device can stand in for a phone with `ledger register <url> --platform android --push-token <any text> --notifications`, so the ledger makes push decisions for it.

The step-by-step test is in [docs/tests/m3-ledger-tests.md](docs/tests/m3-ledger-tests.md).

## Licence

There's no licence yet. Mirrorleaf will be open source; until a licence is chosen, all rights are reserved.
