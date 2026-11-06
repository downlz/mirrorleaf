# Phase 0 report: go/no-go

| | |
|---|---|
| **Phase** | 0 — Technical prototype (implementation plan §6–§7; assessment §12) |
| **Milestones** | M0–M7, 31 October to 6 November 2026 |
| **Devices** | MacBook Air M2 (8 GB) running the Mac app and the CLI; a Motorola Edge 60 Pro on Android 17 (near-stock); the Ubuntu machine as a second desktop peer |
| **Written** | 6 November 2026, by Claude, for the owner's decision |
| **Status** | Draft, for the owner to confirm |

---

## 1. Recommendation

**Go for Phase 1, with Android option A (Tauri 2 plus native Kotlin background code).**

Every Phase 0 gate in assessment §12 passes. One of them, the Android download size, passes only if "download" means what a user downloads (about 12.7 MB) rather than the APK file (28.5 MB); that reading needs the owner's confirmation (§3, item 1). Push-to-wake works: every high-priority push woke the closed app and it synced (90 of 90 across seven phone states). Normal-priority pushes don't reach a phone that is locked and dozing, which Phase 1's status screen and push rules have to allow for (§4). Nothing found calls for Phase 2's options to be brought forward.

---

## 2. The gates

### 2.1 The exit gate (assessment §12, Phase 0)

| Gate | Result | Evidence |
|---|---|---|
| Concurrent offline edits merge with no lost text | ✅ Pass | M1 convergence test (3–5 peers, random offline edits, concurrent edits to one paragraph, edit/delete races); M5 test 4 through the UI (owner-confirmed, 6 Nov) |
| macOS and Android sync a rich-text note and a photo over the LAN | ✅ Pass | M6 tests 2.10 and 3.2–3.3, M5 tests 1–2: direct connections on the same Wi-Fi |
| … and across two different networks, one of them mobile | ✅ Pass (owner-confirmed, no timings) | M5 test 3, rows 2 and 3 of plan §7.1 (phone on mobile data; another Wi-Fi), owner-confirmed 6 Nov. The relay path also carried every sync during the M6 tests while the phone's VPN was on (M6 finding 6) |
| The ledger flags and wakes stale devices | ✅ Pass | M3: 10 rule tests, 9 integration tests on Postgres 17.6 and 18.6, every rule confirmed on the deployed ledger; M4 run 1 with real FCM pushes to the phone |
| Every lightweight target passes | ✅ Pass, with one reading to confirm | §2.2 |
| The three Android tests (option A or B) | ✅ All pass: **stay on option A** | §2.3 |
| Push-to-wake measured at normal and high priority | ✅ Measured; high priority reliable | §2.4 |

### 2.2 Lightweight targets (assessment §12, plan §7.3)

All measured on **release builds** unless marked.

| Target | Limit | Measured | Result | Evidence |
|---|---|---|---|---|
| Desktop installer (macOS) | ≤ 25 MB | **10.3 MB** `.dmg` | ✅ | CI, M5 run (4 Nov) |
| Android download, per CPU type | ≤ 25 MB | arm64: **about 12.7 MB** compressed for download; the APK file is **28.5 MB** | ✅ as a download; ❌ as a file | §3, item 1. armv7 and x86_64 from the new CI job once committed |
| Desktop agent, window closed | ≤ 50 MB RAM (plan M5: ≤ 40 MB), ~0% CPU idle | **14 MB**, 0–0.2% CPU, no web view processes | ✅ | M5 test record (Claude, 4 Nov); M5 test 6 with the window opened then closed, owner-confirmed 6 Nov |
| Cold start, desktop | ≤ 1 s | **0.45–0.49 s** to the open note being editable (5 launches); 1.9 s on the first launch of a newly built app, while macOS checks it | ✅ | M7, app-side timestamps |
| Cold start, Android | ≤ 2 s | **0.54–0.65 s** to the note list (5 launches); first frame at 0.26 s | ✅ | M7, app-side timestamps; `am start -W` for the first frame |
| Sync traffic per text edit | ≤ 5 KB | **0.4–0.9 KB** per sync session carrying one typed edit; about 1.5 KB for a new note (3 changes) | ✅ | The core's byte counters (`Traffic`), from the CLI's sync log during the M6 and M7 tests |
| Imported photo | ≤ 600 KB | **379–595 KB** for the 13 M2 samples; a HEIC picked on the phone 431.8 KB | ✅ | M2 test record; M6 test 3.3 |
| Android with the app closed | No running process; a push-woken job exits within seconds | No process, job or service left after a job; jobs took **1.2–4.0 s** | ✅ | M7 (below) |
| Android background job | ≤ 50 MB RAM; Rust core only, never the web view | **30–36 MB PSS** at peak, about 42 threads, no web view | ✅ | M7 (below) |

**How these were measured (M7, 6 November):**

- **Cold start** is timed from when the operating system started the process (Android: when Zygote forked it; macOS: the kernel's start time) to the screen reporting that it is ready: on the desktop when the open note's editor exists, on a phone when the note list is drawn (a phone opens a note only when tapped). The app logs the time once per run (`startup.rs`).
- **The background job** was measured with the release APK on the phone, locked: five cold push-woken jobs, the app's process sampled every 0.2 s with `dumpsys meminfo`. Two of them carried a real edit from the Mac (3 changes received, direct, in 4.0 s); the others had nothing to fetch. The debug build used 86 MB for the same job, mostly unoptimised code.
- **Traffic** counts our protocol's own bytes, framing included. QUIC's encrypted connection setup isn't counted; for a typical edit the devices are already connected.
- **Android size:** the native Rust library is 26.0 MB of the 28.5 MB APK. Android stores it uncompressed so it can map it straight from the APK; the store compresses the download, and the library compresses to about 10.8 MB.

**Memory with the screen open** has no gate (assessment §12's "honest note"): Android release build, about 148 MB PSS for the app's process plus about 59 MB for its web view renderer; Mac, about 95 MB in all with the window open (M5).

### 2.3 Android tests (assessment §9): option A stays

| Test | Result | Evidence |
|---|---|---|
| 1 — A push wakes the closed app and sync finishes, no screen | ✅ Pass | M4 test 1: about 6 s from an edit on the Mac to the phone, direct, with no screen; and every matrix trial below |
| 2 — Editor and keyboard in Android's WebView | ✅ Pass | M6: typing, selection handles, copy and paste, lists, Backspace across blocks, rotation, the cursor above the keyboard (Claude over adb, 4–5 Nov); finger and glide typing and a second keyboard (owner, 6 Nov). Two Android keyboard issues in the slash menu were found and fixed (M6 findings 1–2) |
| 3 — Photo picker and share sheet | ✅ Pass | M6: Android's photo picker with no permission prompt, HEIC decoded by Android, sharing text, a web page and photos, with the app running or not (Claude and owner) |

### 2.4 Push-to-wake (plan §7.2)

182 trials on the Motorola Edge 60 Pro (M4 test record, run 5), 5–6 November. Delivery is timed from the ledger sending the push to the app receiving it.

| Phone state | Normal priority: synced | High priority: synced | High arrived as high |
|---|---|---|---|
| Screen on, app in the background | 20/20, 0.1 s | 20/20, 0.1 s | 20/20 |
| Screen off, locked overnight (real Doze) | **0/20** within 10 min | 20/20, 0.5 s | 20/20 |
| Battery saver on | 3/20 | 20/20, 0.2 s | 20/20 |
| App set to "restricted" battery use | 3/15 | 10/10, 0.2 s | 10/10 |
| Deep Doze (forced) | 0/3 within 5 min | 10/10, 0.3 s | 10/10 |
| App swiped out of recents | 5/5, 0.4 s | 5/5, 0.1 s | 5/5 |
| After a reboot, app not opened | 5/5 | 5/5 | 5/5 |
| App force-stopped (expected to fail) | 0/2 | 0/2 | – |

- **High priority: 90 of 90** woke the app and synced in the seven states a user can normally be in; none were downgraded. Jobs started within 0.2 s and synced in about 1.2 s.
- **Normal priority** works while the phone is awake, and fails once it is locked and still: Android holds the message for a Doze maintenance window, and even a late one usually can't start a job (M4 finding 7). This is Android's documented behaviour.
- **Force-stopped:** nothing arrives until the user opens the app, as Android intends. The other device's "Open Mirrorleaf on <name> to sync" prompt (M3.1, M4 test 4) is the answer here.
- **By manufacturer:** only one phone was tested (Motorola, close to stock Android). The plan asks for a Samsung or Xiaomi phone too; none was available. Their battery savers are known to be stricter, so this moves to the beta's metrics (E12).
- **FCM's downgrade share:** none of our high-priority pushes arrived downgraded. FCM's own delivery data needs more history than Phase 0 produced; it is an E12 metric.

### 2.5 Milestones

| Milestone | "Done when" | Result |
|---|---|---|
| M0 Foundations | CI passes; README setup works on the Mac | ✅ 31 Oct |
| M1 Sync core | Convergence test; cross-network rows; traffic ≤ 5 KB | ✅ Convergence test and traffic; the cross-network rows passed through M5 test 3 (owner) and M6 |
| M2 Photos | Thumbnail within seconds; full image on request; resume; ≤ 600 KB | ✅ 2 Nov |
| M3 Ledger and push | Stale devices flagged; each rule's push; CLI reports to the deployed ledger | ✅ 3 Nov |
| M4 Android background spike | Android test 1; matrix recorded; no-token fallback | ✅ Test 1 and test 5 on 4 Nov; matrix recorded 6 Nov |
| M5 Editor and desktop app | Note and photo sync on LAN and across networks; concurrent edit through the UI; agent memory | ✅ Owner-confirmed 6 Nov; agent 14 MB |
| M6 Android UI | Android tests 2 and 3 | ✅ Claude 5 Nov, owner 6 Nov |
| M7 Measure and decide | Every gate recorded; recommendation written | ✅ This report |

---

## 3. Decisions for the owner

| # | Decision | Recommendation | Why |
|---|---|---|---|
| 1 | **What the Android size gate measures.** The assessment says "Android download"; plan §7.3 measures "per-ABI split APK size". arm64 is 12.7 MB as a download and 28.5 MB as an APK file. | Measure the download (as the new CI job does), and record the APK file size beside it | The download is what the user waits for and what the store shows. The file is larger only because Android keeps the library uncompressed so it can be mapped without unpacking. If the file size is the gate, the library has to shrink by about 4 MB first (size-optimised compilation, fewer Iroh features). |
| 2 | **iroh-blobs or our own blob protocol** (open since M2, "decide at M7") | **Stay on iroh-blobs through the private beta**; decide before the public beta, as ADR-003 suggests | It passed every photo test and the 50 MB job target. Its one problem, a thread leak per job (M4 finding 6), is fixed by opening the store once per process. Its README still calls the 1.x line "not yet production quality", and moving costs about a week, cheapest before the public beta. |
| 3 | **Confirm ADR-002 to ADR-007** | Confirm, or say what to change | All are still "Proposed". Phase 1 builds on them. |
| 4 | **The periodic catch-up job** (E8, awaiting approval) | Approve as low value, or drop | Android holds periodic jobs in Doze just as it holds normal pushes (finding 7), so it wouldn't help the sleeping phone. It would help a phone that is awake but missed a push. |

---

## 4. Follow-ups for Phase 1

| Follow-up | Where | Source |
|---|---|---|
| Sync status mustn't assume a normal-priority push was acted on; a phone locked overnight catches up when it's opened or gets a high-priority push. Say so during onboarding. | E6, E8 | M4 finding 7 |
| Push-to-wake on Samsung and Xiaomi phones, and FCM's downgrade share, from the beta | E12 | §2.4 |
| The local network permission sometimes needs an app restart, or doesn't apply after an update while showing as granted; check LAN reachability after an update and guide the user | E8 | M4 findings 1 and 4 |
| A VPN on the phone forces relay connections; show it in the connection status | E6 | M6 finding 6 |
| Our own relays and discovery server; n0's public ones are rate-limited | E10 | Memory §6, Iroh 1.0 notes |
| How phones on mobile data reach the ledger (HTTPS hosting) | E10 | Phase 0 ran the ledger on the Mac's LAN |
| LAN sync without internet needs Iroh's mDNS add-on | Open decision | ADR-002 |
| FCM answers 400 for a malformed token; treat it like 404 | E10 | M4 finding 2 |
| Shrink the native library if the size gate is read as the APK file | E13 | §3, item 1 |
| Signing: the release APK in M7 was signed with the debug key for testing only | E13 | M7 |

---

## 5. The open gaps from plan §5.10

| Gap | Answer |
|---|---|
| 1. Heartbeat load | Decided 2 Nov (ADR-004): every 5 minutes, online for 6. Ledger at idle: 2.4 MB, Postgres 22.6 MB (M3) |
| 2. The account's public key | Decided 2 Nov: the account ID is the key |
| 3. The fallback notification can't name devices | Decided 2 Nov: generic text, sent as a string key; shown correctly on the phone (M4 test 3) |
| 4. Android's notification permission | Requested at first start since M4; without it, high-priority pushes still arrive but are downgraded, so onboarding must explain it (E1, E8) |
| 5. Desktop agent over 50 MB | Not needed: 14 MB in the same Tauri process, so the separate agent of §5.7 isn't built |
| 6. HEIC decoders | Done: ImageIO on macOS (M2), ImageDecoder on Android (M6). The Linux CLI syncs HEIC but can't import it |

---

## 6. Phase 1 re-estimate

Phase 0's eight milestones took 7 calendar days (31 October to 6 November), with Claude building and the owner testing and deciding. Phase 1 is broader and more of it depends on people and accounts outside the code.

| Epic | Size | Notes |
|---|---|---|
| E1 Identity and pairing | L | Replaces the developer accounts; QR and word codes; the ledger's device approval exists |
| E2 Recovery kit | M | |
| E3 Device removal and key rotation | M | |
| E4 App lock and encryption at rest | M | SQLCipher changes the SQLite build; check its size cost |
| E5 Notes features | M | Folders, search-or-create and sharing exist; FTS5, trash UI, nested folders remain |
| E6 Sync status and human fallback | M | The fallback exists; badges and onboarding remain |
| E7 Desktop agent hardening | S | Most of it exists (M5) |
| E8 Android hardening | M | Permissions flows; manufacturer guidance |
| E9 Encrypted backup | M | |
| E10 Production infrastructure | L | Hosting, relays, discovery, monitoring; needs the owner's accounts and spending decisions |
| E11 Upgrades and history | L | Protocol negotiation and history compaction are the riskiest code work left |
| E12 Metrics | M | |
| E13 Beta distribution | M | Needs an Apple Developer account and a Play Console account |
| E14 Brand | S, plus the trademark search | The trademark work is the owner's, with a lawyer |

**Estimate:** about 4–6 weeks of build at Phase 0's pace, then the beta itself, which needs several weeks of real use before the success metrics mean anything. The owner's parts set the pace as much as the code: test sessions, the developer accounts, hosting, and the trademark clearance. Signing and store setup should start early, because Apple and Google reviews take days.

---

## 7. What M7 changed

Uncommitted at the time of writing:

- **Release builds work for Android.** The Kotlin bindings are generated from the library's uniffi metadata symbols, which release builds stripped. Android release builds now keep the symbol table but only its global symbols (`build.rs`, `BuildTask.kt`); without this the build failed, and keeping everything would have added 10 MB. The bindings step also read only the arm64 library, which would have broken the other CPU types.
- **CI:** a release APK job for arm64, armv7 and x86_64 with the download size check; the Firebase placeholder moved to `.github/scripts/google-services.sh` for both Android jobs.
- **Cold start timing** (`startup.rs`, the `startup_done` command, the screen's `reportReady`).
- **Android logs** now include the app's own messages (the log filter left them out).
- **`tools/push-matrix.py`:** results saved after every trial, `--resume`, and a clean stop on SIGTERM.
- Test records: M4 run 5 and finding 7; the owner's M5 and M6 results.
