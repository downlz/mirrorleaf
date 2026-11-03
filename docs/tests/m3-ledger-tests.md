# M3 ledger and push: deployment and manual test steps

| | |
|---|---|
| **Milestone** | M3 — Ledger and push (implementation plan §6) |
| **Machines** | The Ubuntu machine runs the ledger (Docker) and stands in for an Android phone; the Mac is a desktop device |
| **Covers** | M3's "done when" items: the ledger marks the right devices as stale, sends the right push to the right device at the right priority for every rule, and CLI peers report their status to the deployed ledger |
| **Date written** | 3 November 2026 |

There is no Android app until M4, and no Firebase project yet, so the ledger runs **without FCM**: it decides every push and **logs** it instead of sending it. The logs are where the push rules are checked. The automated tests cover the same rules, plus the FCM client against a fake FCM.

---

## Before you start

| Rule | Why |
|---|---|
| Paste commands **without** `# comments` | zsh treats `#` as an argument |
| Wait **2 minutes** between push tests aimed at the same device | The ledger pushes each device at most once per 2 minutes (plan §5.9). A test run sooner shows no push, which is correct, not a failure. |
| Keep both machines' clocks right (automatic time on) | The ledger refuses requests more than 5 minutes off (`clock_skew`) |
| Stop `sync --watch` before `dev-account ticket` or a one-off `sync` on the same device | As in M2: one process uses a device's photo store at a time. `ledger status` and `ledger wake` are safe while `sync --watch` runs. |

The commands use `mirrorleaf-cli`. Use `./mirrorleaf-cli` in the folder that holds it, or your `ml` alias.

**Device names below:** the Mac is `mac`, the Ubuntu machine is `ubuntu`. The short IDs in the output (8 hex characters) identify them.

---

## 1. Deploy the ledger on the Ubuntu machine

1. Docker, if it isn't there yet (plan §3.4):

   ```bash
   sudo apt update && sudo apt install -y docker.io docker-compose-v2
   sudo usermod -aG docker $USER
   ```

   Log out and back in so `docker` works without `sudo`.

2. Get the M3 code onto the Ubuntu machine (after you commit and push it on the Mac):

   ```bash
   git clone https://github.com/downlz/mirrorleaf.git
   cd mirrorleaf/ledger
   ```

   (Or `git pull` in an existing clone, then `cd ledger`.)

3. Settings:

   ```bash
   cp env.example .env
   openssl rand -hex 16
   ```

   Open `.env` and set `POSTGRES_PASSWORD` to the value printed. Leave `FCM_SERVICE_ACCOUNT` empty.

4. Build and start (the first build compiles the ledger and takes several minutes):

   ```bash
   docker compose up -d --build
   docker compose ps
   curl http://localhost:8080/health
   ```

   **Expected:** both `db` and `ledger` are running, and `curl` prints `ok`.

5. Watch the ledger's log in a terminal of its own; leave it open for the whole test:

   ```bash
   docker compose logs -f ledger
   ```

   **Expected:** `FCM_SERVICE_ACCOUNT isn't set: pushes are only logged`, then `listening on 0.0.0.0:8080`.

6. Find the Ubuntu machine's address on the LAN:

   ```bash
   hostname -I
   ```

   Call it `<ubuntu-ip>`. If the firewall is on (`sudo ufw status`), open the port: `sudo ufw allow 8080/tcp`.

7. From the Mac, check it can reach the ledger:

   ```bash
   curl http://<ubuntu-ip>:8080/health
   ```

---

### Variant: the ledger on the Mac with OrbStack

Used for the first M3 test run (owner's decision, 3 November 2026), instead of the Ubuntu machine. The same `compose.yaml` runs in OrbStack's Docker on the Mac:

```bash
cd ledger
cp env.example .env
docker compose up -d --build
```

Set `POSTGRES_PASSWORD` in `.env` as in step 3 above. The ledger log is `docker compose logs -f ledger` on the Mac. Everything else is the same, except the addresses:

| Device | Registers with |
|---|---|
| The Mac | `http://localhost:8080` |
| The Ubuntu machine | `http://<mac-ip>:8080`, where `<mac-ip>` is printed by `ipconfig getifaddr en0` on the Mac |

From Ubuntu, check `curl http://<mac-ip>:8080/health` prints `ok` first. If it doesn't, allow incoming connections for OrbStack in macOS's firewall (System Settings → Network → Firewall → Options).

To stop the ledger and give the memory back: `docker compose down` in the `ledger` folder (the database is kept), then quit OrbStack if nothing else uses it.

## 2. Put the M3 CLI on both machines

As in the M2 test doc: build on the Mac (`cargo build -p mirrorleaf-cli`), and on Ubuntu download the `mirrorleaf-cli-x86_64-unknown-linux-gnu` artifact from the M3 CI run (or build it there). On the Mac, a downloaded build also needs `xattr -d com.apple.quarantine mirrorleaf-cli`.

**Check:** `mirrorleaf-cli status` now has a `Ledger   none (…)` line on both machines.

---

## Test 1 — Devices report to the deployed ledger (M3: "CLI peers report their status")

1. **Mac:**

   ```bash
   mirrorleaf-cli ledger register http://<ubuntu-ip>:8080
   ```

2. **Ubuntu**, registering as a stand-in for an Android phone with a made-up push token, so the ledger makes push decisions for it:

   ```bash
   mirrorleaf-cli ledger register http://localhost:8080 --platform android --push-token test-ubuntu --notifications
   ```

3. **Expected** on each: `Registered with … as macos` (or `android`), then one line per device, for example:

   ```text
     09330754  mac              macos    offline  behind 0      this device
     28460260  ubuntu           android  offline  behind 0      last seen 0 min ago
   ```

   The ledger log shows `device … registered` twice.

4. **Mac:** `mirrorleaf-cli status` shows `Ledger   http://<ubuntu-ip>:8080 (as macos)`.

**Pass when:** both devices register and appear in each other's `ledger status`.

---

## Test 2 — Online, heartbeat and going offline

1. **Mac:** `mirrorleaf-cli sync --watch`.

   **Expected:** `Reporting to the ledger at …`, then a line such as `ledger: mac online, behind 0 · ubuntu offline, behind 0`.

2. **Ubuntu** (no watcher running there yet): `mirrorleaf-cli ledger status`.

   **Expected:** `mac` is `online`.

3. **Mac:** stop the watcher with Ctrl-C.

   **Expected:** `ledger: reported going offline`. On Ubuntu, `ledger status` shows `mac` `offline` straight away.

4. *(Optional, slow)* Start `sync --watch` on the Mac, then pull the Mac's network cable or turn Wi-Fi off **without** stopping the watcher. `ledger status` on Ubuntu keeps showing `mac online` for up to 6 minutes, then `offline`. That is the heartbeat window (ADR-004).

---

## Test 3 — Staleness (M3: "marks the right devices as stale")

1. Neither machine runs `sync --watch`.
2. **Mac:** add a note, then report:

   ```bash
   mirrorleaf-cli note new "Ledger test" --text "made on the mac"
   mirrorleaf-cli ledger status
   ```

   **Expected:** `ubuntu` is `behind 3` (the note, its paragraph and its index entry); `mac` is `behind 0`.

3. **Ubuntu:** `mirrorleaf-cli ledger status` shows the same numbers from its side.
4. Sync them: start `sync --watch` on both; within a few seconds both `ledger:` lines show `behind 0`. Stop both watchers (Ctrl-C).

**Pass when:** the device missing changes is the one shown behind, by the right count, and both return to 0 after syncing.

---

## Test 4 — Push rules (M3: "the right push to the right device at the right priority")

Keep the ledger log open (step 1.5). Each expected line below appears there; `<ubuntu>` and `<mac>` stand for the short IDs. Wait 2 minutes between the rules.

### Rule 1: new changes wake an offline phone (normal priority)

1. Ubuntu runs no watcher (it stands for a phone in a pocket).
2. **Mac:** `mirrorleaf-cli sync --watch`, then in a second Mac terminal:

   ```bash
   mirrorleaf-cli note new "Rule 1" --text "wake the phone"
   ```

**Expected in the ledger log** within about 30 s:

```text
push <ubuntu> normal: pull from <mac>
```

Stop the Mac's watcher afterwards.

### Rule 2: a device coming online behind wakes the phone that holds its changes (normal)

1. **Ubuntu** (still no watcher): make a change and report it, as a phone does before going back in the pocket:

   ```bash
   mirrorleaf-cli note new "Rule 2" --text "written on the phone"
   mirrorleaf-cli ledger status
   ```

2. Wait 2 minutes, then **Mac:** `mirrorleaf-cli sync --watch`.

**Expected:**

```text
push <ubuntu> normal: deliver 3 changes to <mac>
```

Leave the Mac's watcher running for rule 3.

### Rule 3: opening the app while behind wakes the holder at high priority

1. **Mac**, second terminal (the watcher keeps running, the Mac is still behind):

   ```bash
   mirrorleaf-cli ledger wake
   ```

**Expected:** the Mac prints `Woke … (high priority) to deliver 3 changes.`, and the log shows:

```text
push <ubuntu> high: deliver 3 changes to <mac>
```

2. Run `ledger wake` again at once. **Expected:** `… was already woken in the last two minutes.` and no new push.

### Rule 4: an unanswered wake-up gets the fallback notification

Do nothing on Ubuntu for about 75 seconds (a real phone would have delivered the changes; this one can't).

**Expected in the log**, 60–75 s after the rule 3 push:

```text
push <ubuntu> high: fallback notification
```

It appears **once**.

### No push when none would help

1. **Ubuntu:** `mirrorleaf-cli sync --watch` (now the "phone" is online and delivers the changes).
2. **Mac:** `mirrorleaf-cli ledger wake`.

**Expected:** `Nothing to fetch: this device is up to date.` (or, if it hasn't synced yet, `… holds the changes and is online`), and no push in the log.

3. Stop both watchers. On the Mac, `ledger wake` while it is up to date also says nothing is needed.

---

## Test 5 — The automated tests

On the Mac, against a Postgres database (the Supabase one used while building M3, or any Postgres 17 or 18):

```bash
export MIRRORLEAF_TEST_DATABASE_URL='postgresql://…'
cargo test -p mirrorleaf-ledger
```

**Expected:** 10 rule tests and 9 integration tests pass. In the connection string, a `#` in the password must be written `%23`. CI runs the same tests against its own Postgres on every push.

| Test | Shows |
|---|---|
| `the_ledger_marks_the_right_devices_as_stale` | Staleness counts, online and offline, minute-precision presence |
| `rule_1_…`, `rule_2_…`, `rules_3_and_4_…` | Each rule: target, priority, kind, the 2-minute limit, the 60 s fallback once |
| `wake_requests_answer_without_a_push_when_none_helps` | Up to date, holder online, no wakeable holder, normal priority without notification permission |
| `the_fcm_client_sends_http_v1_messages_and_drops_dead_tokens` | The real FCM client against a fake FCM: access token, message format, forgetting a dead token |
| `devices_register_report_and_go_offline_through_the_client` | The CLI's client and `sync --watch` against the ledger |
| `registration_…`, `devices_can_be_removed_…` | Signatures, approvals, clock skew, removal, 90-day pruning |

---

## If something goes wrong

| Symptom | Cause and fix |
|---|---|
| `can't reach the ledger at …` | Wrong address, the ledger isn't running (`docker compose ps`), or the firewall blocks port 8080 (`sudo ufw allow 8080/tcp`) |
| `clock_skew` | A machine's clock is more than 5 minutes off. Turn on automatic time. |
| `this device doesn't hold the account key` | Only devices that joined with a developer ticket can register in Phase 0 |
| An expected push doesn't appear | The 2-minute limit (wait and repeat), the "phone" is online (a watcher is running on Ubuntu), or it lacks nothing. `ledger status` shows who is behind. |
| `docker compose up` fails on `POSTGRES_PASSWORD` | `.env` is missing or the password is empty. Use letters and digits only. |
| The ledger was rebuilt with an empty database | Nothing to do: each device registers again on its next report |

To stop the ledger: `docker compose down` in the `ledger` folder. The database is kept in a Docker volume; `docker compose down -v` deletes it too.

---

## Results

### Deployment

| Check | Result |
|---|---|
| `docker compose up -d --build` succeeded (first build time) | ✅ 3 Nov 2026 on the Mac with OrbStack (variant above): image built and both containers started in 2 min 21 s. Image 112 MB; at idle the ledger uses 2.4 MB of memory and Postgres 22.6 MB. |
| `curl …/health` from the Mac prints `ok` | ✅ From `localhost` and from the LAN address (`192.168.1.10`). A smoke test with two throwaway devices registered, reported and triggered a logged high-priority push; both were removed afterwards, leaving the database empty. |

### Tests 1–4

| Test | Expected | Result |
|---|---|---|
| 1 Register both devices | Both listed by `ledger status` on each | |
| 2 Online / going offline | `mac` online while watching, offline at once after Ctrl-C | |
| 3 Staleness | `ubuntu behind 3` after the Mac's note; 0 after syncing | |
| 4 Rule 1 | `push <ubuntu> normal: pull from <mac>` | |
| 4 Rule 2 | `push <ubuntu> normal: deliver 3 changes to <mac>` | |
| 4 Rule 3 | `push <ubuntu> high: deliver 3 changes to <mac>`; then "already woken" | |
| 4 Rule 4 | `push <ubuntu> high: fallback notification`, once, 60–75 s later | |
| 4 No push needed | "up to date" or "holder online", no push | |
| 5 Automated tests | 19 pass | |

### Pass or fail

| M3 "done when" | Result | Evidence |
|---|---|---|
| Integration tests show the ledger marks the right devices as stale | ✅ Pass (3 Nov 2026, against Postgres 17.6 and 18.6) | `the_ledger_marks_the_right_devices_as_stale`; test 3 |
| For every rule in §5.9, it sends the right push to the right device at the right priority | ✅ Pass in the automated tests (3 Nov 2026) | Rule tests and `the_fcm_client_…`; test 4 to confirm on the deployed ledger |
| CLI peers report their status to the deployed ledger | | Tests 1–2 on the Ubuntu deployment |
