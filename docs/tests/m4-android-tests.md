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
| 6 — Push-to-wake matrix | ✅ All eight states run on 5–6 November (run 5), 182 trials | Run 5 |

### Run 2 — 4 November 2026: checking the matrix script

`tools/push-matrix.py --device d57d4595 --states background --trials 2 --gap 5`, after commit `b072547`. Same phone and setup as run 1, with `sync --watch` on the Mac and nothing new to sync, so each job only reports to the ledger and exits: these figures time the wake-up, not a data transfer.

| State | Priority | Delivered | Arrived as high | Delivery | Job start | Sync |
|---|---|---|---|---|---|---|
| Screen on, app in the background | normal | 2/2 | – | 0.2 s (max 0.2) | 0.1 s (max 0.2) | 3.2 s (max 5.1) |
| Screen on, app in the background | high | 2/2 | 2/2 | 0.1 s (max 0.1) | 0.1 s (max 0.1) | 1.3 s (max 1.3) |

The script works end to end: test pushes through the ledger, matched on the phone by FCM's message ID, the three timings, the CSV and the summary table. An earlier attempt the same afternoon took 26–32 s per sync because of finding 4.

### Run 3 — 4 November 2026: the automatic matrix, stopped early

`run-matrix` with the six automatic states, with `tools/mem-trace.py` alongside. Stopped by the owner after 16 minutes and 24 trials, all in the first state. It found findings 5 and 6, both now fixed; the matrix is to be run again.

| State | Priority | Delivered | Arrived as high | Delivery | Job start | Sync |
|---|---|---|---|---|---|---|
| Screen on, app in the background | normal | 20/20 | – | 0.1 s (max 10.8) | 0.1 s (max 0.2) | 1.3 s (max 4.2) |
| Screen on, app in the background | high | 2/4 | 2/2 | 13.8 s (max 14.6) | 0.1 s (max 0.1) | 21.1 s (max 39.2) |

The high-priority results reflect finding 5, not how Mirrorleaf's real pushes behave.

**Memory over the run** (one process, which the matrix opened at the start, so the WebView was loaded throughout; debug build):

| Phase | Samples | PSS median | PSS max | Private (PSS anon) median | Threads median |
|---|---|---|---|---|---|
| In a job | 149 | 85.9 MB | 158.9 MB | 37.3 MB | 129 |
| Idle (cached) | 1,818 | 89.4 MB | 160.2 MB | 34.6 MB | 124 |
| Screen open | 10 | 157.9 MB | 161.7 MB | 74.0 MB | 106 |

Private memory, including the part Android swapped out, didn't grow over the 22 jobs (89 MB in the first 2 minutes, 85 MB in the last 2). The thread count did: finding 6.

**Memory snapshots** (`dumpsys meminfo`, debug build):

| Case | PSS | Main parts |
|---|---|---|
| Push-woken job from cold, no screen | 86.0 MB | The Rust library's code 31.0 MB (32.7 MB of unoptimised machine code), Kotlin/Java dex 19.3 MB, ART image 9.0 MB, Rust native heap 7.0 MB, Java heap 4.7 MB; 41 threads |
| Cached after that job | about 42 MB | Android moved most of it to compressed swap; 2.3 MB of private memory stayed in RAM |
| Screen open | about 240 MB, plus 50 MB for the WebView renderer process | Graphics 54 MB, code 66 MB, system 93 MB |

### Run 4 — 4 November 2026: after fixing findings 5 and 6

The same state and trial count as run 3, with the fixes installed (test pushes without a collapse key; one photo store per process), and `tools/mem-trace.py` alongside.

| State | Priority | Delivered | Arrived as high | Delivery | Job start | Sync |
|---|---|---|---|---|---|---|
| Screen on, app in the background | normal | 20/20 | – | 0.2 s (max 2.0) | 0.1 s (max 0.1) | 1.2 s (max 4.9) |
| Screen on, app in the background | high | **20/20** | **20/20** | 0.1 s (max 0.3) | 0.1 s (max 0.1) | 1.2 s (max 3.1) |

| Over 40 jobs in one process | Run 3 (before the fix, 22 jobs) | Run 4 (after) |
|---|---|---|
| Threads before each job | 103 at the start, rising by about one per job to 130 | 101–106, levelling off at 110–111 from job 20 to job 40 |
| `iroh-blob-store` threads at the end | 25 | 8 (the store's own pool) |
| Private memory, including the part swapped out (first 2 min → last 2 min) | 89 → 85 MB | 90 → 81 MB |

**Both fixes confirmed:** every high-priority push arrived, as high, and the thread count stays flat. Memory doesn't grow from job to job. (Memory figures are higher than in the cold-job snapshot because the matrix opened the app first, so the WebView stayed loaded in the process.)

**Memory, first reading (debug build, not the M7 measurement):** the push-started process, cached after its job, held 11.8 MB of private memory (`RssAnon`). Resident memory was 112 MB (peak 160 MB), most of it shared file mappings, including the 191 MB debug native library. The ≤ 50 MB target is checked in M7 on a release build, during the job.

### Run 5 — 5–6 November 2026: the full matrix (M7)

Run by Claude overnight: `tools/push-matrix.py` from 23:02 on 5 November, with the phone locked, unplugged as far as Android could tell (`dumpsys battery unplug`, so Doze applied) but charging, the owner's VPN off, and the Mac running `mirrorleaf-cli sync --watch` under `caffeinate`. Debug build; nothing new to sync, so the sync column times the wake-up and the ledger report. Each trial waited up to 10 minutes (5 minutes for the morning's Doze trials, 2 for force-stopped).

At 08:12 the run was on its fourth state, because every normal-priority push held back by Android used the whole 10 minutes. At the owner's request it was stopped there, and the remaining states were run with fewer trials: restricted at high priority (10), Doze (10 high, 3 normal) and force-stopped (2 each), finishing at 08:51. 162 trials in all; results in `tools/push-matrix-results/m7-matrix-20261105-2302.csv` (gitignored).

| State | Priority | Delivered | Synced | Arrived as high | Delivery | Job start | Sync |
|---|---|---|---|---|---|---|---|
| Screen on, app in the background | normal | 20/20 | 20/20 | – | 0.1 s (max 0.2) | 0.1 s (max 0.1) | 1.2 s (max 2.8) |
| Screen on, app in the background | high | 20/20 | 20/20 | 20/20 | 0.1 s (max 0.2) | 0.1 s (max 0.1) | 1.2 s (max 2.0) |
| Screen off (locked overnight, in real Doze) | normal | 2/20 | **0/20** | – | 457 s (max 570) | – | – |
| Screen off (locked overnight, in real Doze) | high | 20/20 | 20/20 | 20/20 | 0.5 s (max 0.7) | 0.1 s (max 0.2) | 1.2 s (max 1.9) |
| Battery saver on | normal | 5/20 | 3/20 | – | 0.5 s (max 592) | 0.5 s (max 13.6) | 1.2 s (max 8.6) |
| Battery saver on | high | 20/20 | 20/20 | 20/20 | 0.2 s (max 0.5) | 0.1 s (max 0.2) | 0.2 s (max 9.1) |
| App set to "restricted" battery use | normal | 4/15 | 3/15 | – | 0.3 s (max 478) | 0.1 s (max 0.2) | 1.2 s (max 1.6) |
| App set to "restricted" battery use | high | 10/10 | 10/10 | 10/10 | 0.2 s (max 0.7) | 0.1 s (max 0.7) | 1.2 s (max 2.0) |
| Deep Doze (forced) | normal | 0/3 | 0/3 | – | – | – | – |
| Deep Doze (forced) | high | 10/10 | 10/10 | 10/10 | 0.3 s (max 0.6) | 0.1 s (max 0.1) | 1.2 s (max 1.7) |
| App force-stopped | normal | 0/2 | 0/2 | – | – | – | – |
| App force-stopped | high | 0/2 | 0/2 | – | – | – | – |
| App swiped out of recents | normal | 5/5 | 5/5 | – | 0.4 s (max 0.4) | 0.1 s (max 0.1) | 1.2 s (max 1.3) |
| App swiped out of recents | high | 5/5 | 5/5 | 5/5 | 0.1 s (max 0.1) | 0.1 s (max 0.1) | 1.2 s (max 2.9) |
| After a reboot, app not opened | normal | 5/5 | 5/5 | – | first 10.4 s, then under 0.1 s* | 0.1 s (max 0.2) | 1.3 s (max 4.5) |
| After a reboot, app not opened | high | 5/5 | 5/5 | 5/5 | under 0.1 s* | 0.0 s (max 0.0) | 1.1 s (max 1.2) |

The last two states need the owner, so they were run on 6 November around 10:25–10:40 with the owner present: the owner restarted the phone and unlocked it without opening Mirrorleaf, then later opened it and swiped it away from recents. Both ran on the **release build** (M7), with the phone awake and unlocked, which is why normal priority did well here; later trials in each state follow the first (the script stands in for later swipes by killing the process, as Android does). *Just after the restart the phone's clock was slightly ahead of the Mac's, so delivery times read as 0; the job and sync columns are timed on the phone alone. A system component (most likely WorkManager rescheduling its jobs) started the app's process about a minute after boot, without the app being opened and without a sync.

**What it shows:**

1. **Every high-priority push woke the closed app and it synced: 90 of 90** in the seven states a user can be in (all but force-stopped), none downgraded to normal, delivered in under 0.7 s.
2. **Normal-priority pushes are reliable only while the phone is awake.** Once it is locked and still, Android holds them for its Doze maintenance windows. The few that arrived late (6–10 minutes) mostly didn't get to run a job ("delivered, but no job started": Android doesn't let a normal-priority message start an expedited job in Doze). The normal pushes that synced in battery saver and restricted mode were the first few, before the phone dozed off.
3. **A force-stopped app receives nothing,** as Android intends, until the user opens it again. Only the other device's "Open Mirrorleaf on your phone" prompt helps then.

This confirms the push rules (plan §5.9): a high-priority push with a visible notification whenever someone is waiting for the edit; normal priority only as a cheap nudge, which a sleeping phone may not act on until it is next opened or gets a high-priority push. Finding 7.

### Findings

1. **The local network permission applied only after the app restarted.** It was granted while the app was running; until the app was restarted, its connections to the Mac's LAN address were reset and Iroh used a relay. After the restart, LAN connections worked and were direct. Phase 1's onboarding should ask before the first sync, or restart networking after the grant.
2. **FCM answers 400 for a malformed token** (the made-up token of the M3 stand-in "ubuntu" device). The ledger forgets a token only on 404, so it keeps trying that one. Small follow-up: treat FCM's invalid-token answer like 404.
3. Closing a job's node while a peer dials it gives that peer "the server refused to accept a new connection" once; the next dial works. Harmless.
4. **Local network access can stop working while the permission still shows as granted.** After the APK was reinstalled (`adb install -r`), the app's connections to the LAN were reset: the ledger was unreachable, and Iroh reached the Mac only through a relay. The phone's shell reached the ledger, the app-op showed `ACCESS_LOCAL_NETWORK: allow`, and a force-stop didn't help. Revoking and granting the permission again (`pm revoke`, then `pm grant`) fixed it at once. A second reinstall straight afterwards didn't reproduce it, so it is intermittent. With finding 1, this suggests Android 17 (or Motorola's build) doesn't always apply the permission to the app's network access. Effects: without the LAN, sync falls back to the internet or a relay (assessment risk #9), but a ledger on the LAN becomes unreachable, and a job that can't reach the ledger dials every device, waiting 20 s for each offline one. Follow-ups: watch for it after Play Store updates in the beta; in Phase 1, check LAN reachability after an update and show the user how to fix it; the job could stop dialling devices the ledger last saw offline even when the ledger can't be reached.
5. **FCM throttles collapsible messages: a burst of 20 per app per device, then one every 3 minutes** ([FCM throttling and quotas](https://firebase.google.com/docs/cloud-messaging/throttling-and-quotas)). The ledger's pushes all use the collapse key `sync`, so the matrix's 20 normal trials used up the burst: the 20th was 10.8 s late, and 2 of the following 4 high-priority pushes didn't arrive within 120 s. Real use stays far below the limit (at most one ordinary push per device every 2 minutes). **Fixed:** `test-push` now sends without a collapse key; the rules' pushes keep it, so an unreachable phone is still sent only the latest wake-up.
6. **A thread leaked with every push-woken job.** Over 22 jobs the process went from 103 to 130 threads, and the `iroh-blob-store` threads from 3 to 25. Each job opened the photo store and closed it at the end; iroh-blobs' file store runs its own thread pool, and closing it leaves one thread behind (it drops the pool from one of the pool's own threads). On a phone woken all day this would grow without limit. **Fixed:** the engine opens the photo store once per process and every job's node shares it (`PhotoStore`, `SyncNode::bind_with_store`); a new loopback test checks that a shared store keeps working after the nodes using it close. Confirmed on the phone in run 4.
7. **Normal-priority pushes don't reach a dozing phone in useful time** (run 5). Locked overnight, 0 of 20 led to a sync within 10 minutes; 2 arrived after 7–10 minutes and ran no job. This is Android's documented behaviour ([FCM message priority](https://firebase.google.com/docs/cloud-messaging/android-message-priority)), not a fault. Nothing to fix in M4; for Phase 1 (E6, E8): the "behind" status and the fallback prompt must not assume a normal-priority push has been acted on, and a periodic catch-up job would be held in Doze too (the one proposed for E8 was dropped, owner, 6 Nov 2026).

### Pass or fail

| M4 "done when" | Result | Evidence |
|---|---|---|
| Android test 1 passes: a push wakes the closed app and sync finishes with no screen open | ✅ Pass (4 Nov 2026) | Test 1 |
| The matrix results are recorded | ✅ Recorded for all eight states (run 5, 5–6 Nov 2026) on one stock-like phone; no Samsung or Xiaomi phone was available | Test 6, run 5 |
| With no push token, opening the app syncs with every reachable device, and a device that is behind shows the fallback prompt naming the holder | ✅ Pass (4 Nov 2026; test 5 owner-confirmed) | Tests 4 and 5 |
