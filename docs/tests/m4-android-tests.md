# M4 Android background spike: setup and test steps

| | |
|---|---|
| **Milestone** | M4 — Android background spike (implementation plan §6) |
| **Devices** | The Mac (CLI peer and the ledger in OrbStack) and an Android phone on Android 17 (a Motorola Edge 60 Pro), on the same Wi-Fi |
| **Covers** | M4's "done when" items: Android test 1, the push-to-wake matrix (plan §7.2), and the fallback when no push can reach a device |
| **Date written** | 4 November 2026 |

---

## Before you start

### On the Mac

| Tool | Version | Check |
|---|---|---|
| Rust with the Android target | stable; `rustup target add aarch64-linux-android` | `rustup target list --installed` |
| JDK 17 | any 17 build (Temurin or Homebrew `openjdk@17`) | `java -version` |
| Android SDK and NDK | platform 37, NDK 29.0.14206865 | `ls $ANDROID_HOME/ndk` |
| Environment | `JAVA_HOME`, `ANDROID_HOME`, `NDK_HOME` set in your shell profile | `echo $NDK_HOME` |
| Tauri CLI | 2.12.4 | `cargo install tauri-cli --version 2.12.4 --locked` |
| Gradle memory cap | `org.gradle.jvmargs=-Xmx2g -Dfile.encoding=UTF-8` in `~/.gradle/gradle.properties` | plan §3.3 |

Put the Firebase file in place (it's gitignored; never commit it):

```bash
cp ledger/secrets/google-services.json apps/mirrorleaf/src-tauri/gen/android/app/
```

### On the phone

1. Settings → About phone → tap **Build number** 7 times.
2. Settings → System → Developer options → **USB debugging** on. Connect the phone and allow the Mac.
3. `adb devices -l` lists the phone as `device`.
4. To keep the screen on while it's plugged in: `adb shell svc power stayon true` (undo with `adb shell svc power stayon false`). Turn it off again for the Doze tests.

### Rules that apply to every step

| Rule | Why |
|---|---|
| Paste commands **without** `# comments` | zsh treats `#` as an argument |
| Run `./gradlew --stop` in `apps/mirrorleaf/src-tauri/gen/android` before Docker builds | Gradle and Docker together don't fit in 8 GB |
| Grant the app's permissions **before** expecting it to reach the LAN, then restart the app once | On Android 17 the local network permission didn't apply to the app process that was running when it was granted (finding 1 below) |
| Follow the phone with `adb logcat -s mirrorleaf` | The push-woken job has no screen; its log is the evidence |

---

## 1. Build and install

```bash
cd apps/mirrorleaf/src-tauri
cargo tauri android build --debug --target aarch64 --apk --ci
adb install -r gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
```

The first build downloads Gradle and takes about 5 minutes; later builds take seconds to a few minutes. The debug APK is large (about 250 MB) because it keeps the native library's debug symbols.

Open the app. Allow **notifications** and **nearby devices** (Android 17's local network permission). Then force-stop and reopen it once (finding 1).

## 2. Start the ledger and the Mac

```bash
cd ledger
docker compose up -d --build
cd ..
cargo build -p mirrorleaf-cli
./target/debug/mirrorleaf-cli dev-account ticket
./target/debug/mirrorleaf-cli sync --watch
```

`FCM_SERVICE_ACCOUNT` must be set in `ledger/.env` for real pushes. Start `sync --watch` only after printing the ticket.

## 3. Join and register the phone

1. In the app's ticket box, paste the ticket. Typing it with `adb shell input text "<ticket>"` after tapping the box avoids typing 605 characters.
2. Tap **Join**. Expected: "Joined. Synced with the inviting device via … : received N changes", the notes appear, and "Live sync: on".
3. Check the ledger address (the Mac's LAN address, port 8080) and tap **Use this ledger**. Expected: "Registered with the ledger at …".
4. On the Mac, `mirrorleaf-cli ledger status` lists the phone as `android online behind 0`.

---

## Test 1 — A push wakes the closed app (Android test 1, rule 1)

1. On the phone, go to the home screen. On the Mac, `ledger status` shows the phone **offline**.
2. Kill the app's process as Android would: `adb shell am kill app.mirrorleaf`. `adb shell pidof app.mirrorleaf` prints nothing.
3. On the Mac: `mirrorleaf-cli note new "Push test" --text "from the Mac"`.

**Expected:** within seconds, the ledger log shows `push <phone> normal: pull from <mac>`; logcat shows `push: pull, normal priority`, then `background sync: reached ["mac (direct)"] … received 3`. `ledger status` shows the phone `behind 0`. No screen opened.

## Test 2 — The phone delivers when the Mac comes back (rule 2)

1. Stop the Mac's `sync --watch` (Ctrl-C).
2. On the phone, make a note, then go to the home screen and run `adb shell am kill app.mirrorleaf`.
3. Wait 2 minutes since the last push to the phone (the ledger's limit), then start `sync --watch` on the Mac.

**Expected:** ledger `push <phone> normal: deliver 1 changes to <mac>`; logcat `background sync: reached ["mac (direct)"] … sent 1`; the Mac prints `received 1 changes`.

## Test 3 — Opening a device while behind wakes the phone at high priority (rule 3), and the fallback (rule 4)

1. Stop the Mac's `sync --watch`. Make a note on the phone, go home, kill the app.
2. On the Mac: `mirrorleaf-cli ledger wake --wait`.

**Expected:**
- The Mac prints `Woke … (high priority) to deliver 1 changes`.
- Logcat: `push: deliver, high priority (sent as high)` and a sync job; the phone shows "Syncing…" briefly. The Mac is offline, so nothing can be delivered.
- 60–75 s after the wake, the ledger logs `push <phone> high: fallback notification`, and the phone shows **Mirrorleaf — "You have changes waiting to sync. Open Mirrorleaf to send them."** (`adb shell dumpsys notification --noredact | grep -A30 pkg=app.mirrorleaf`).
- At 90 s the Mac prints `Open Mirrorleaf on <phone> to sync.`

## Test 4 — The phone in use shows the prompt for a device no push can reach

1. Stop the Mac's `sync --watch`. On the Mac, edit a note and run `ledger status` (this reports the edit).
2. On the phone, go home, then open the app again.

**Expected:** logcat `behind by N: the ledger answered NoWakeableHolder`, and the app shows **"Open Mirrorleaf on mac to sync."** at once. Then start `sync --watch` on the Mac: the changes arrive within 30 s and the prompt disappears.

## Test 5 — No push token

On a phone without Google Play services (or an emulator image without them), the app shows "Push token: no". Repeat test 4 with the roles swapped: changes made on that phone, the Mac asks with `ledger wake --wait`, and the Mac's prompt appears at once (`NoWakeableHolder`). Opening the app on the phone syncs with every reachable device.

## Test 6 — The push-to-wake matrix (plan §7.2)

Run with `tools/push-matrix.py`, from the repository root. It sends test pushes with `mirrorleaf-ledger test-push`, which skips the ledger's push rules and their 2-minute limit (tests 1–4 cover the rules), and reads the phone's log to time each one:

| Column | Measured from → to |
|---|---|
| Delivery | FCM sending the push → the app receiving it (FCM's clock → the phone's clock; both set from the network) |
| Job start | The app receiving the push → the sync job starting |
| Sync | The job starting → the job finishing |

Before you start: the ledger running with Firebase credentials, the phone registered with a push token, `mirrorleaf-cli sync --watch` running on the Mac, and the phone connected with adb.

```bash
tools/push-matrix.py --device <the phone's device ID> --states background --trials 2 --gap 5
tools/push-matrix.py --device <the phone's device ID> --trials 20
```

The first line is a quick check; the second runs every state at both priorities. Pick states with `--states background,doze,…`. The script sets up each state where adb can, and asks for the rest:

| Phone state | `--states` name | How the script sets it up |
|---|---|---|
| Screen on, app in the background | `background` | Opens the app, then goes to the home screen |
| Screen off for 10 minutes | `screen-off` | Screen off, unplugged (simulated); waits 10 minutes |
| Deep Doze | `doze` | Screen off, unplugged; `dumpsys deviceidle force-idle` before every trial |
| App swiped out of recents | `swiped` | Asks you to swipe it away; later trials kill the process the previous push started |
| Battery saver on | `battery-saver` | `cmd power set-mode 1`, screen off, unplugged |
| App set to "restricted" battery use | `restricted` | The `RUN_ANY_IN_BACKGROUND` app-op denied and the "restricted" standby bucket |
| After a reboot, app not opened | `reboot` | Reboots the phone and asks you to unlock it without opening Mirrorleaf |
| App force-stopped | `force-stopped` | `am force-stop` before every trial (expected to fail) |

It undoes each setup afterwards, and puts back the phone's "stay awake" setting. "Unplugged" is simulated with `dumpsys battery unplug`, because USB power stops Doze; with wireless debugging the phone can be truly unplugged. Each trial goes into a CSV file in `tools/push-matrix-results/` (gitignored), and the script prints a summary table to paste into the results below. Ctrl-C stops early and keeps the trials so far.

Repeat on a Samsung or Xiaomi phone if one is available (plan §7.2), with `--serial` when both are connected.

---

## Results

### Run 1 — 4 November 2026

Run by Claude over adb on the owner's Motorola Edge 60 Pro (Android 17, API 37), with the owner granting the permissions on the phone. Mac and phone on the same Wi-Fi; the ledger on the Mac with OrbStack, sending real pushes through FCM. Debug build. The phone was charging with the screen kept on, so Doze didn't apply.

| Test | Result | Evidence |
|---|---|---|
| Install and start | ✅ | The engine starts in the app's process (`engine open for d57d4595`); push token received; notifications allowed |
| Join | ✅ | 47 changes and 5 notes in 525 ms (first via relay, finding 1) |
| Register with the ledger | ✅ after restarting the app | Before the restart: "Connection reset by peer" (finding 1) |
| **1 — Push wakes the closed app** | ✅ **Pass** | Edit 16:04:41.0 → ledger push 16:04:42.1 (normal) → process started 16:04:43.4 → job started 16:04:43.9 → `reached ["mac (direct)"] … received 3 in 3354 ms` at 16:04:47.3. About 6 s from edit to phone, with no screen. |
| 2 — Rule 2 | ✅ | Mac online 16:11:13 → push `deliver 1 changes` 16:11:18.9 (normal) → job `reached ["mac (direct)"] … sent 1` 16:11:22.8; the Mac received 1 change |
| 3 — Rule 3 | ✅ | Push `deliver 1 changes`, high, arrived as high (not downgraded); expedited job started 0.15 s after the push; finished in 245 ms with the Mac offline |
| 3 — Rule 4 | ✅ | Fallback push 74 s after the wake; the system showed "Mirrorleaf — You have changes waiting to sync. Open Mirrorleaf to send them." on the `fallback` channel, from the app's strings; the Mac's prompt at 90 s |
| 4 — Prompt on the phone | ✅ | `NoWakeableHolder` → "Open Mirrorleaf on mac to sync." at once; cleared when the Mac came online and sent the 2 changes (892 ms, direct) |
| 5 — No push token | ✅ Pass (owner-confirmed, 4 Nov 2026) | Run by the owner; no figures were recorded |
| 6 — Push-to-wake matrix | Not yet run | |

**Memory, first reading (debug build, not the M7 measurement):** the push-started process, cached after its job, held 11.8 MB of private memory (`RssAnon`). Resident memory was 112 MB (peak 160 MB), most of it shared file mappings, including the 191 MB debug native library. The ≤ 50 MB target is checked in M7 on a release build, during the job.

### Findings

1. **The local network permission applied only after the app restarted.** It was granted while the app was running; until the app was restarted, its connections to the Mac's LAN address were reset and Iroh used a relay. After the restart, LAN connections worked and were direct. Phase 1's onboarding should ask before the first sync, or restart networking after the grant.
2. **FCM answers 400 for a malformed token** (the made-up token of the M3 stand-in "ubuntu" device). The ledger forgets a token only on 404, so it keeps trying that one. Small follow-up: treat FCM's invalid-token answer like 404.
3. Closing a job's node while a peer dials it gives that peer "the server refused to accept a new connection" once; the next dial works. Harmless.

### Pass or fail

| M4 "done when" | Result | Evidence |
|---|---|---|
| Android test 1 passes: a push wakes the closed app and sync finishes with no screen open | ✅ Pass (4 Nov 2026) | Test 1 |
| The matrix results are recorded | ⏳ Not yet run | Test 6 |
| With no push token, opening the app syncs with every reachable device, and a device that is behind shows the fallback prompt naming the holder | ✅ Pass (4 Nov 2026; test 5 owner-confirmed) | Tests 4 and 5 |
