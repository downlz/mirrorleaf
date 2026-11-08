# E0 ledger readiness: test steps and results

| | |
|---|---|
| **Work** | E0 — Ledger readiness (proposed in `Ledger-Resilience-and-Hosting-Discussion.md`, 6 November 2026). Phase 1 milestone **M8** (ledger ready and reachable), which also adds the FCM 400 fix; this is M8's test record |
| **Machines** | The Mac (OrbStack) for everything below; the phone for test 5 |
| **Covers** | HTTPS through Caddy with no IP addresses logged (H1), request limits and the device cap (H2), a clean stop on SIGTERM (H3), a health check that asks the database (H4) |
| **Date written** | 6 November 2026; M8 results 7–8 November 2026 |

Deploying off the Mac (H7) and an uptime monitor on `/health` moved to Phase 1 M13 (E10, production hosting; owner, 8 November 2026): until then the ledger stays on the Mac, started by hand for tests. **H5 done (owner, 8 November 2026):** the old service-account key was deleted in the Google Cloud console and `ledger/secrets/fcm-service-account-old.json` removed; the ledger uses `fcm-service-account.json`, which delivered the test 5 push. M3 Test 6 A–C (H6) was run on 7 November 2026 (results in [m3-ledger-tests.md](m3-ledger-tests.md)).

---

## What changed

| Item | Change |
|---|---|
| H1 | `ledger/Caddyfile` and a `caddy` service (`caddy:2.11.7`, the newest release at least 7 days old) under the Compose profile `https`. Caddy's `http` loggers are dropped, because its error logs and Go's TLS handshake errors name the client's address; certificate messages stay. The ledger's port is published on `127.0.0.1` only, unless `LEDGER_PUBLISH` says otherwise. |
| H2 | `ledger/src/limits.rs`: limits that need no IP address. Each signing device gets 30 requests a minute, counted only after its signature checks out, so nobody can use up another device's allowance. New accounts are limited to 20 a minute across the ledger, counting only accounts it doesn't hold yet. Both rates can be set in `.env`, and 0 turns a limit off. An account holds at most 10 devices (`MAX_DEVICES_PER_ACCOUNT` in `mirrorleaf-proto`), checked under a lock on the account's row. New error codes: `rate_limited` (429) and `too_many_devices` (403). |
| H3 | `main.rs` stops on SIGTERM as well as Ctrl-C and lets requests in progress finish. |
| H4 | `GET /health` runs `SELECT 1`, waiting up to 3 s: it answers `ok` (200) or `database unavailable` (503). |
| FCM 400 (M8; M4 finding 2) | FCM answers 400 `INVALID_ARGUMENT` for a token that was never valid, and the ledger used to forget a token only on 404, so it kept pushing to it. It now forgets a token on a 400 that **names the token**: a field violation on `message.token`, or FCM's "not a valid FCM registration token" message (`token_rejected` in `ledger/src/push.rs`). Any other 400 is a fault in the message and would hit every token, so the token is kept and the failure logged, as before. An `UNREGISTERED` error code counts whatever the status. The 400 that FCM sent the running ledger on 7 November 2026 for the made-up token of a stand-in device has exactly this shape. |
| FCM access token (M8, found by test 5) | The ledger reused its Google access token until 5 minutes before expiry, timed with a monotonic clock. That clock stops while the machine sleeps (the Mac, and OrbStack's VM on it), so after a night the ledger still used a token from the day before and FCM answered 401 `ACCESS_TOKEN_EXPIRED`: the push was lost. Now the expiry is kept by the wall clock, and a 401 from FCM drops the token, fetches a new one and sends once more (`Fcm::send_message` in `ledger/src/push.rs`). A server that never sleeps could still meet a 401 (a revoked key, a token ended early), so the retry stays useful in production. |

**Not done as the discussion record proposed:** Caddy has no rate limiting without a custom build with a third-party plugin, so the limits live in the ledger and are keyed by device keys, not IP addresses. Per-IP limits at the edge stay an option for E10.

---

## Automated checks (Claude, 6 November 2026)

| Check | Result |
|---|---|
| `cargo fmt`, `cargo clippy -p mirrorleaf-ledger -p mirrorleaf-proto --all-targets -D warnings` | ✅ Clean |
| Unit tests | ✅ 15 pass: 10 rules, 1 push, 4 new limiter tests |
| Integration tests against Postgres 18 (a throwaway container on the Mac) | ✅ 12 pass, 3 of them new: `health_answers_only_while_the_database_does`, `an_account_holds_at_most_ten_devices`, `requests_are_limited_per_device_and_new_accounts_per_ledger` (including forged signatures not using up a device's allowance) |
| `caddy validate` on the Caddyfile | ✅ Valid |
| Caddy logs no addresses | ✅ After a 502 (ledger absent) and a TLS handshake error, no line names an address. **Control:** without `exclude http`, the same requests log `remote_ip`. |
| Ledger behind Caddy (`LEDGER_DOMAIN=localhost`) | ✅ `/health` 200 over HTTP/2; the ledger's own answers come through; Caddy's `Server` header is removed |
| SIGTERM | ✅ The ledger logged "shutting down", then "stopped", and exited with status 0 within 0.04 s |
| `docker compose build ledger` | ✅ Built (74 s); not started |
| `docker compose config` | ✅ The default stack is `db` and `ledger` on `127.0.0.1:8080`; `--profile https` adds `caddy` |

## Automated checks for M8 (Claude, 7 November 2026)

| Check | Result |
|---|---|
| `cargo fmt`, `cargo clippy -p mirrorleaf-ledger --all-targets -D warnings` | ✅ Clean |
| Unit tests | ✅ 16 pass. New: `only_answers_naming_the_token_reject_it` (FCM's malformed-token answer, the field violation or the message alone, a 400 for a fault in the message, `UNREGISTERED`, other statuses, an answer that isn't JSON) |
| Integration tests against Postgres 18.6 (a throwaway container on the Mac) | ✅ 12 pass. `the_fcm_client_sends_http_v1_messages_and_drops_dead_tokens` now also has the fake FCM answer 400 for a malformed token: the token is forgotten and the device in use hears "no wakeable holder" at once. The 500 case still keeps its token |
| Ledger image | ✅ Rebuilt with the fix (22 s with cached dependencies) |
| Access-token fix (8 Nov 2026) | ✅ clippy clean; 16 unit and 12 integration tests pass on Postgres 18.6. The fake FCM now numbers its access tokens and can refuse one with 401 `ACCESS_TOKEN_EXPIRED`: the client fetches a second token and the push is sent once, on the second try |

---

## Before you start

| Rule | Why |
|---|---|
| Paste commands **without** `# comments` | zsh treats `#` as an argument |
| Run `./gradlew --stop` before `--build` | The image compiles Rust inside the container (8 GB Mac) |
| **The ledger is now on this machine only by default.** If the phone reaches it over Wi-Fi (`http://<mac IP>:8080`), add `LEDGER_PUBLISH=8080` to `ledger/.env` before `docker compose up` | Otherwise the phone shows the ledger as unreachable after the restart |

---

## 1. The new ledger on the Mac

```bash
cd ledger
docker compose up -d --build
curl -s http://127.0.0.1:8080/health
docker compose stop ledger
docker compose logs ledger | tail -3
docker compose start ledger
```

**Expect:** `ok`; the stop takes a second or two, not 10; the log ends with `shutting down: finishing requests in progress` and `stopped`.

## 2. Health check when the database stops

```bash
docker compose stop db
curl -s -w ' %{http_code}\n' http://127.0.0.1:8080/health
docker compose start db
curl -s -w ' %{http_code}\n' http://127.0.0.1:8080/health
```

**Expect:** `database unavailable 503`, then `ok 200` once Postgres is back (a few seconds).

## 3. HTTPS on the Mac

```bash
docker compose --profile https up -d
curl -sk https://localhost/health
docker compose logs caddy | grep -c remote_ip
```

**Expect:** `ok`, and `0`. (`-k` because the certificate comes from Caddy's local authority, which the Mac doesn't trust unless you add it.)

## 4. The device cap

With a CLI device registered, the eleventh device on one account is refused with `too_many_devices`. The automated test covers this; to see it by hand, register ten throwaway CLI devices (`--dir` per device) on one developer account, then an eleventh.

## 5. The phone over mobile data (needs a tunnel)

Uses **Tailscale Funnel**, which ends TLS on the Mac, so Tailscale sees only encrypted traffic. Cloudflare Tunnel and ngrok end TLS on their own servers and would see the ledger requests.

1. Install Tailscale on the Mac and turn on Funnel for it in the Tailscale admin console.
2. `tailscale funnel --bg 8080`: the ledger is then at `https://<mac name>.<tailnet>.ts.net`.
3. On the phone, with Wi-Fi **off**: Settings → Ledger → the `https://…ts.net` address → **Use this ledger**.
4. On the Mac: `mirrorleaf-cli ledger register https://<mac name>.<tailnet>.ts.net`
5. Edit a note on the Mac with the app on the phone closed.

**Expect:** both devices show as registered in `mirrorleaf-cli ledger status`; the edit reaches the phone through a push, as in M4 Android test 1.

Turn it off afterwards with `tailscale funnel reset`.

---

## Results

| Test | Result |
|---|---|
| Automated checks | ✅ Pass (6 Nov 2026, Claude; M8 additions 7 Nov 2026) |
| 1. New ledger, clean stop | ✅ Pass (7 Nov 2026, Claude): `/health` answered `ok`; `docker compose stop ledger` took 0.32 s; the log ended with `shutting down: finishing requests in progress` and `stopped`; exit status 0 |
| 2. Health check | ✅ Pass (7 Nov 2026, Claude): `database unavailable 503` while Postgres was stopped (answered in 0.26 s), `ok 200` one second after it started again |
| 3. HTTPS on the Mac | ✅ Pass (7 Nov 2026, Claude): `ok` over HTTPS (HTTP/2, no `Server` header); an unsigned request reached the ledger and got its own 401; Caddy's 19 log lines (start-up and certificate messages) name no address, including after a TLS handshake with the wrong name |
| 4. Device cap by hand | Not run (optional; `an_account_holds_at_most_ten_devices` covers it) |
| 5. Phone over mobile data | ✅ Pass (8 Nov 2026, Claude with the owner; run below), after fixing the access-token bug it found |

**How Claude ran tests 1–3 (7 November 2026).** Not on the owner's ledger: on a throwaway Compose project (`docker compose -p m8test`, its own database volume) published on `127.0.0.1:18080`, with `FCM_SERVICE_ACCOUNT` left empty (log-only), so the owner's database, Firebase key and `.env` were untouched. The commands are the ones above with `-p m8test` and port 18080. The throwaway project was removed afterwards (`down -v`).

**Seen on the owner's ledger while starting OrbStack (7 November 2026).** It was still running the image from before E0 (container from 6 November, 18:50) and logged `FCM answered 400` for device `28460260`, the M3 stand-in "ubuntu" device with a made-up token: the case the M8 fix handles. Claude then rebuilt the owner's image and restarted the ledger on it (`docker compose up -d`; `.env`, the Firebase key and the database unchanged): `/health` answered `ok`, FCM on, still published on port 8080. The ledger forgets that token at its next push to it.

### Test 5, run 1 — 8 November 2026

Claude drove the phone over USB (adb); the owner turned on Tailscale Funnel (`https://shahnawazs-macbook-air.tail8c974e.ts.net` → `127.0.0.1:8080`; the safety check doesn't let Claude open a port to the internet) and unlocked the phone. The phone: Motorola Edge 60 Pro, Mirrorleaf 0.2.0 release build, **Wi-Fi off, on Airtel LTE**, charging over USB. The owner's ledger with real FCM.

| Step | Result |
|---|---|
| `/health` through Funnel from the Mac | ✅ `ok 200` in about 1.5 s (Tailscale's Funnel servers and back); a valid public certificate for the `ts.net` name. The very first request timed out while Tailscale fetched the certificate |
| Phone: Settings → Ledger → the `https://…ts.net` address → Use this ledger | ✅ The ledger logged `device d57d4595 registered`; the phone showed online, with a push token and notifications allowed |
| Mac: `mirrorleaf-cli ledger register https://…ts.net` | ✅ Registered |
| App on the phone closed (Home, then `am kill`: not force-stopped, so FCM can still start it) | ✅ No process; the ledger showed the phone offline |
| **Attempt 1:** a note from the Mac CLI (`sync --watch` running) | ❌ The ledger sent rule 1's push (normal priority), but **FCM answered 401 `ACCESS_TOKEN_EXPIRED`**: the access token had been fetched at the previous push, 24.7 hours earlier, and the ledger's monotonic clock hadn't counted the hours the Mac slept. Fixed (above), rebuilt, restarted |
| **Attempt 2:** another note from the Mac CLI, 23:18:37 IST | ✅ Push sent at 23:18:38.9 (normal priority); Android started the closed app for it at 23:18:39.2; the push arrived at 39.7 (FCM sent it at 39.4); the sync job started at 39.7; it reached **both Mac devices directly** over mobile data and received 6 changes (both test notes) in 6.8 s; the job finished at 23:18:46.5. **About 9 s from the edit on the Mac to the note on the closed phone**, with the ledger reached only over HTTPS |

**Also seen:** the ledger forgot the "ubuntu" stand-in's made-up push token on 7 November (`forgetting 28460260's push token: FCM refuses it`): the M8 FCM 400 fix working against real FCM.

**Afterwards:** the owner turned Funnel off. The Mac CLI device is back on `http://localhost:8080`. Claude turned the phone's Wi-Fi back on and switched it back to `http://192.168.1.13:8080` ("Use this ledger" saves an address only once it has registered there, so this needs the phone on home Wi-Fi): the phone showed "Registered with the ledger at http://192.168.1.13:8080." and the ledger logged it. Two test notes, "E0 test 5" and "E0 test 5, second edit", stay in the dev account.
