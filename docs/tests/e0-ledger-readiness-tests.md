# E0 ledger readiness: test steps and results

| | |
|---|---|
| **Work** | E0 — Ledger readiness (proposed in `Ledger-Resilience-and-Hosting-Discussion.md`, 6 November 2026; **not yet in the implementation plan**) |
| **Machines** | The Mac (OrbStack) for everything below; the phone for test 5 |
| **Covers** | HTTPS through Caddy with no IP addresses logged (H1), request limits and the device cap (H2), a clean stop on SIGTERM (H3), a health check that asks the database (H4) |
| **Date written** | 6 November 2026 |

Left for the owner, outside this record: delete the old FCM key (H5), run M3 Test 6 A–C (H6, in [m3-ledger-tests.md](m3-ledger-tests.md)), deploy off the Mac (H7), and set up an uptime monitor on `/health`.

---

## What changed

| Item | Change |
|---|---|
| H1 | `ledger/Caddyfile` and a `caddy` service (`caddy:2.11.7`, the newest release at least 7 days old) under the Compose profile `https`. Caddy's `http` loggers are dropped, because its error logs and Go's TLS handshake errors name the client's address; certificate messages stay. The ledger's port is published on `127.0.0.1` only, unless `LEDGER_PUBLISH` says otherwise. |
| H2 | `ledger/src/limits.rs`: limits that need no IP address. Each signing device gets 30 requests a minute, counted only after its signature checks out, so nobody can use up another device's allowance. New accounts are limited to 20 a minute across the ledger, counting only accounts it doesn't hold yet. Both rates can be set in `.env`, and 0 turns a limit off. An account holds at most 10 devices (`MAX_DEVICES_PER_ACCOUNT` in `mirrorleaf-proto`), checked under a lock on the account's row. New error codes: `rate_limited` (429) and `too_many_devices` (403). |
| H3 | `main.rs` stops on SIGTERM as well as Ctrl-C and lets requests in progress finish. |
| H4 | `GET /health` runs `SELECT 1`, waiting up to 3 s: it answers `ok` (200) or `database unavailable` (503). |

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
| Automated checks | ✅ Pass (6 Nov 2026, Claude) |
| 1. New ledger, clean stop | Not yet run |
| 2. Health check | Not yet run |
| 3. HTTPS on the Mac | Not yet run |
| 4. Device cap by hand | Optional |
| 5. Phone over mobile data | Not yet run |
