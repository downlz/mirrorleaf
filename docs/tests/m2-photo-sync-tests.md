# M2 photo sync: manual test steps

| | |
|---|---|
| **Milestone** | M2 — Photos in the core (implementation plan §6) |
| **Devices** | The Mac and the Ubuntu machine, already on one developer account from the M1 test |
| **Covers** | M2's "done when" items, and the photo columns of the cross-network test (plan §7.1) |
| **Date written** | 2 November 2026 |

The automated tests already cover the same behaviour on one machine (`cargo test --workspace`). These steps check it between two real machines and with real phone photos.

---

## Before you start

### Rules that apply to every step

| Rule | Why |
|---|---|
| Paste commands **without** `# comments` | zsh treats `#` as an argument, so the command fails (and a `>` redirect still creates an empty file). |
| Only one Mirrorleaf process per device may use photos at a time | The photo store (`<data folder>/blobs`) can be opened by one process. `photo add`, `photo list`, `photo fetch`, `note …` and `status` are safe while `sync --watch` runs. **Stop `sync --watch` (Ctrl-C) before** `dev-account ticket`, a one-off `sync`, or `photo export`. |
| HEIC photos can be imported **on the Mac only** | The Linux build has no HEIC decoder. Ubuntu still receives and serves photos imported on the Mac. |
| Keep the sample photos **outside the repo** | The repo is public, and the photos carry location data. |

The commands below use `mirrorleaf-cli`. If you set up the `ml` alias, use that instead.

### 1. Put the M2 build on both machines

**Mac** (in the `mirrorleaf` folder):

```bash
cargo build -p mirrorleaf-cli
```

The binary is `target/debug/mirrorleaf-cli`.

Or use CI's Apple silicon build: download the `mirrorleaf-cli-aarch64-apple-darwin` artifact, unzip it into a folder of its own (**not** `target/`, where Cargo would take it for its own build and stop rebuilding), then:

```bash
chmod +x mirrorleaf-cli
xattr -d com.apple.quarantine mirrorleaf-cli
./mirrorleaf-cli status
```

The `xattr` step is needed because macOS marks files downloaded with a browser as quarantined, and silently kills an unsigned quarantined program (exit code 137, no message). zsh only looks for commands on your `PATH`, never in the current folder, so a program in the current folder needs `./` in front, or an alias such as `alias ml="$PWD/mirrorleaf-cli"`.

**Ubuntu**, either:

- **From CI:** after you commit and push, open the CI run's page on GitHub, download the `mirrorleaf-cli-x86_64-unknown-linux-gnu` artifact, unzip it, then run `chmod +x mirrorleaf-cli` (artifacts lose the executable bit) and start it as `./mirrorleaf-cli`; or
- **Build it there:** pull the branch and run `cargo build -p mirrorleaf-cli`.

### 2. Check both devices upgraded

On each machine:

```bash
mirrorleaf-cli status
```

**Expected:** the last line reads `Photos   0 (0 thumbnails and 0 full images here)`. That line only appears in an M2 build, and it shows that the M1 database was upgraded (schema version 2). Your M1 notes are still listed above it.

### 3. Prepare the sample photos

Copy **10 photos taken with a phone camera** to a folder on the Mac, for example `~/mirrorleaf-samples`:

| Kind | How many | How to get them |
|---|---|---|
| HEIC, 12 MP | At least 5 | iPhone: AirDrop to the Mac (AirDrop keeps HEIC). Android: only if the camera is set to HEIF. |
| JPEG, 12 MP | At least 5 | Android camera default, or iPhone with Settings → Camera → Formats → *Most Compatible* |

Include at least one photo taken in **portrait** (phone upright) and one with **location** turned on, so the rotation and EXIF checks below have something to check.

Copy two or three of the JPEGs to the Ubuntu machine too (for test 5).

---

## Test 1 — Import size (M2: "each imports at ≤ 600 KB")

On the Mac, with nothing else running:

```bash
mirrorleaf-cli note new "M2 photo test" --text "sample photos"
mirrorleaf-cli note list
```

Take the first characters of the new note's ID (call it `<note>`), then:

```bash
mirrorleaf-cli photo add <note> ~/mirrorleaf-samples/*
```

**Expected:** one line per photo, for example:

```text
cea66145  IMG_0412.HEIC (2.9 MB) -> 1536×2048, 452.1 KB at quality 85, EXIF 14.2 KB kept; thumbnail 21.3 KB (980 ms)
```

| Check | Pass when |
|---|---|
| Size | Every full image is **≤ 600.0 KB** |
| Dimensions | Long side is **2048** (or the original size, if smaller) |
| Portrait photos | Width < height (for example `1536×2048`): turned upright |
| EXIF | `EXIF … kept` on photos that had it |
| Errors | None. A HEIC that fails here is a bug to report. |

Record each line in the results table at the end.

---

## Test 2 — Thumbnails reach the other device (M2: "within seconds on the LAN")

Both machines on **the same Wi-Fi**.

1. **Ubuntu:** start the watcher with an export folder, so you can see the photos:

   ```bash
   mirrorleaf-cli sync --watch --export ~/mirrorleaf-photos
   ```

2. **Mac**, in one terminal:

   ```bash
   mirrorleaf-cli sync --watch
   ```

   **Expected on the Mac:** a `stored thumbnail of photo … added here` and `stored full image of photo … added here` line for each photo from test 1 (they were waiting to enter the photo store), then `connected … via direct`.

3. **Expected on Ubuntu** within a few seconds: one line per photo, for example:

   ```text
   1a2b3c4d (mac) via direct: thumbnail of photo cea66145, 21.3 KB (21.9 KB read) in 35 ms
   ```

4. **Ubuntu**, in a second terminal:

   ```bash
   ls -l ~/mirrorleaf-photos
   mirrorleaf-cli photo list
   ```

   **Expected:** a `<photo>-thumbnail.jpg` per photo, and every photo listed as `thumbnail here   full image not here`. Open a thumbnail (`xdg-open ~/mirrorleaf-photos/<photo>-thumbnail.jpg`): it is upright and 320 px on its long side.

5. **Live add.** Leave both watchers running. On the Mac, in a second terminal, add one more photo:

   ```bash
   mirrorleaf-cli photo add <note> ~/mirrorleaf-samples/<one photo>
   ```

   **Expected on Ubuntu** within about 3 seconds: a `received 2 changes` line, then a `thumbnail of photo …` line. To measure the delay, note the clock when `photo add` returns and when the Ubuntu line appears (or run the Ubuntu watcher through `ts` from the `moreutils` package to timestamp each line).

**Pass when:** every thumbnail arrives on Ubuntu, and a live-added one within seconds. Full images do **not** move unasked.

---

## Test 3 — Full image on request (M2: "the full image arrives on request")

Both watchers still running.

1. **Ubuntu**, second terminal, pick a photo from `photo list`:

   ```bash
   mirrorleaf-cli photo fetch <photo>
   ```

   **Expected:** `Asked for the full image of …`, then `Arrived after 0.x s.` On the watcher: `full image of photo …, 452.1 KB (… read) in … ms`, and `~/mirrorleaf-photos/<photo>.jpg` appears.

2. **Check the image:** open `~/mirrorleaf-photos/<photo>.jpg`. It is upright, sharp, 2048 px on its long side, and its colours match the original.

3. **Check the metadata was kept** (owner's decision, ADR-003):

   ```bash
   sudo apt install libimage-exiftool-perl
   exiftool -Orientation -DateTimeOriginal -Make -Model -GPSPosition ~/mirrorleaf-photos/<photo>.jpg
   ```

   **Expected:** `Orientation: Horizontal (normal)`, plus the original date, camera and (for a photo with location) GPS position.

**Pass when:** the full image arrives within seconds, displays correctly, and keeps its EXIF.

---

## Test 4 — The device holding the photo is offline

Shows that a request waits until a device that has the photo comes online.

1. **Mac:** stop the watcher (Ctrl-C).
2. **Ubuntu:**

   ```bash
   mirrorleaf-cli photo fetch <another photo> --wait 10
   ```

   **Expected:** after 10 s, `Not here yet: it stays asked for…`. `photo list` shows `full image asked for`.

3. **Mac:** start `sync --watch` again.

   **Expected on Ubuntu** within about 30 s of reconnecting: a `full image of photo …` line, and `photo list` shows `full image here`.

---

## Test 5 — The other direction, and HEIC on Linux

1. **Ubuntu**, second terminal (the watcher keeps running):

   ```bash
   mirrorleaf-cli photo add <note> ~/<a JPEG copied from the Mac>.jpg
   ```

   **Expected:** imported with a size line as in test 1; the Mac's watcher shows the thumbnail arriving within seconds.

2. **Ubuntu**, try a HEIC (copy one over first):

   ```bash
   mirrorleaf-cli photo add <note> ~/<a photo>.HEIC
   ```

   **Expected:** an error naming the file, caused by `can't import this photo: HEIC and AVIF photos need the system's decoder, which isn't available here`. Nothing is added to the note.

---

## Test 6 — Photos across networks (plan §7.1)

Repeat test 2 step 5 (a live add) and test 3 (a fetch) for each row, and record the connection type and times. The `via direct` / `via relay` and `in … ms` parts of the watcher lines give the figures.

| # | Mac | Ubuntu |
|---|---|---|
| 1 | Home Wi-Fi | Same Wi-Fi |
| 2 | Home Wi-Fi | Tethered to a phone on mobile data |
| 3 | Home Wi-Fi | A different Wi-Fi (another home or an office) |
| 4 | A network that blocks UDP, if available | Any |

Row 4 should still work, through a relay.

> **Note on M1 timings:** M2 fixed a bug where the *receiving* side's `in … ms` for a sync round included the time spent waiting for the round. If your M1 notes took times from the receiving side, re-measure text edits here too. Sending-side times from M1 were correct.

---

## Test 7 — Interrupted transfer resumes (M2: "an interrupted transfer resumes")

A 600 KB photo moves too fast on a LAN to cut off by hand, so the evidence for this item is the automated test:

```bash
cargo test -p mirrorleaf-net --test loopback an_interrupted_transfer_resumes -- --nocapture
```

**Expected:** `resumed transfer read about 1500000 bytes` and `ok`. The test stops a 2 MB transfer after 500 KB, then fetches again, and only the remaining ~1.5 MB is sent.

**Optional manual check** over a slow link (row 2 of test 6): request several full images with `photo fetch`, and turn the Ubuntu machine's network off part-way. The watcher reports `no connected device could send it yet`. Turn the network back on: the images arrive once the devices reconnect, without starting again from zero.

---

## If something goes wrong

| Symptom | Cause and fix |
|---|---|
| `can't open the photo store … (is another Mirrorleaf process using it?)` | Another process on that device has the store open, usually `sync --watch`. Stop it, or use `sync --watch --export` instead of `photo export`. |
| A `.txt` or ticket file is empty | A `# comment` was pasted with the command. Run it again without the comment. |
| `… no connected device could send it yet (…)` | No connected device holds that blob yet. It is retried every 30 s. Check that the device that added the photo is running `sync --watch`. |
| Photos added on the Mac never reach Ubuntu | Check the Mac's watcher printed `stored … added here` lines. If `photo add` ran while no watcher was running, the next `sync` or `sync --watch` on the Mac stores them. |
| Ubuntu's `status` has no `Photos` line | Ubuntu is still running the M1 build. |
| `zsh: command not found: mirrorleaf-cli` in the folder that holds it | zsh doesn't look in the current folder. Run `./mirrorleaf-cli`, or set an alias. |
| A downloaded Mac build prints nothing and exits (code 137) | macOS quarantine. Run `xattr -d com.apple.quarantine mirrorleaf-cli`. |

---

## Results

Fill in and keep with the M2 notes; these figures go into the Phase 0 report (plan §7.4).

### Import size (test 1)

**Run 1 — 2 November 2026, Mac (CI's Apple silicon build from `d004c4f`).** Note `01a19d2a-42c2`, photos from `~/Documents/Projects/Local_Notes/mirrorleaf-samples/`. Time is the import time per photo.

| # | Photo | File | Format | Original size | Output dimensions | Full image size | Quality | EXIF kept | Thumbnail size | Time | ≤ 600 KB? |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 4229ed85 | 865A1904.JPG | JPEG | 10.1 MB | 2048×1365 | 553.0 KB | 80 | 31.2 KB | 20.9 KB | 566 ms | ✅ |
| 2 | 98d64f14 | 865A1965.JPG | JPEG | 8.5 MB | 2048×1365 | 416.5 KB | 85 | 28.7 KB | 16.6 KB | 491 ms | ✅ |
| 3 | 2c9fc1b3 | 865A1967.JPG | JPEG | 8.2 MB | 2048×1365 | 421.2 KB | 85 | 29.3 KB | 17.3 KB | 485 ms | ✅ |
| 4 | 31f59475 | 865A1981.JPG | JPEG | 8.7 MB | 1365×2048 (portrait) | 516.4 KB | 80 | 31.9 KB | 22.1 KB | 642 ms | ✅ |
| 5 | fd47b17d | 865A1983.JPG | JPEG | 7.6 MB | 2048×1365 | 378.8 KB | 85 | 28.8 KB | 16.4 KB | 484 ms | ✅ |
| 6 | b6bbf374 | 865A1984.JPG | JPEG | 8.0 MB | 2048×1365 | 386.2 KB | 85 | 28.8 KB | 16.3 KB | 486 ms | ✅ |
| 7 | df699e4f | rsz_1rsz_865a1750.jpg | JPEG | 6.5 MB | 2048×1524 | 544.7 KB | 80 | 30.1 KB | 21.7 KB | 280 ms | ✅ |
| 8 | 8af9689d | rsz_865a1750.jpg | JPEG | 6.2 MB | 2048×1680 | 523.4 KB | 75 | 30.1 KB | 24.0 KB | 334 ms | ✅ |

**Run 1 findings**

- All 8 imported at ≤ 600 KB (378.8–553.0 KB) in under 0.7 s each, with their EXIF kept (28.7–31.9 KB) and thumbnails of 16.3–24.0 KB.
- The portrait photo (#4) came out upright (1365×2048).
- Four photos needed a lower quality than the first step (80, or 75 for #8) to fit; none needed shrinking below 2048 px. The EXIF block (about 30 KB each) counts towards the 600 KB.
- Run 1 had no HEIC; see run 2. The `865A….JPG` names and 3:2 shape suggest a dedicated camera rather than a phone, and #7–#8 were resized before import (`rsz_`).

**Run 2 — 2 November 2026, Mac (same build).** HEIC photos, into the same note, from `~/Documents/Projects/Local_Notes/mirrorleaf-samples/2/`. Decoded with macOS ImageIO.

| # | Photo | File | Format | Original size | Output dimensions | Full image size | Quality | EXIF kept | Thumbnail size | Time | ≤ 600 KB? |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 9 | 4cd43a7d | childrens-show-theater.heic | HEIC | 1.3 MB | 2048×1536 | 458.7 KB | 85 | 2.2 KB | 15.4 KB | 283 ms | ✅ |
| 10 | da330abf | classic-car.heic | HEIC | 2.0 MB | 1536×2048 (portrait) | 521.8 KB | 85 | 2.4 KB | 13.2 KB | 139 ms | ✅ |
| 11 | 7e24a773 | greyhounds-looking-for-a-table.heic | HEIC | 1.7 MB | 1536×2048 (portrait) | 570.4 KB | 75 | 2.4 KB | 23.1 KB | 191 ms | ✅ |
| 12 | 62d726bf | old-safe-wall.heic | HEIC | 2.8 MB | 1536×2048 (portrait) | 594.9 KB | 70 | 2.4 KB | 23.5 KB | 233 ms | ✅ |
| 13 | e84835b7 | soundboard.heic | HEIC | 1.2 MB | 1536×2048 (portrait) | 501.3 KB | 85 | 842 B | 24.1 KB | 133 ms | ✅ |

**Run 2 findings**

- All 5 HEIC photos imported at ≤ 600 KB (458.7–594.9 KB), each in under 0.3 s; ImageIO decodes faster than the Rust JPEG path because it scales while decoding.
- The four portrait photos came out upright (1536×2048).
- **Smallest margin so far:** #12 (`old-safe-wall`, a detailed texture) needed quality 70 and landed at 594.9 KB, 5 KB under the limit. Two quality steps (65, 60) and the shrink step are still in reserve, so the limit holds, but detailed photos will be noticeably more compressed than the rest.
- Their EXIF blocks are small (0.8–2.4 KB) compared with a phone's usual 10–30 KB, which suggests these are sample files with most metadata already removed. Check against a HEIC straight from your phone if you want to confirm that location and camera details survive (test 3's `exiftool` step).

**Totals after runs 1 and 2:** 13 photos (8 JPEG, 5 HEIC), all ≤ 600 KB.

### Transfer (tests 2, 3 and 6)

| Network row | Path (direct / relay) | Thumbnail: delay after `photo add` | Thumbnail transfer time | Full image transfer time | Text edit time (sending side) |
|---|---|---|---|---|---|
| 1 Same Wi-Fi | Direct (test 2 run 1) | | | | |
| 2 Phone hotspot | | | | | |
| 3 Different Wi-Fi | | | | | |
| 4 UDP blocked | | | | | |

### Test 2 log

**Run 1 — 2 November 2026, same Wi-Fi.** Mac `09330754` (mac) and Ubuntu `28460260` (ubuntu).

**Mac side (step 2):**

| Check | Result |
|---|---|
| Photos from test 1 entered the photo store when the watcher started | ✅ 26 `stored … added here` lines: a full image and a thumbnail for each of the 13 photos, sizes matching test 1 |
| Connection to Ubuntu | ✅ `connected to 28460260 (ubuntu) via direct`, then `connected from 28460260 (ubuntu) via direct` (each side dialled the other) |
| Changes sent | 29 changes (the note and its 13 photo additions) in **1,032 ms**: 5,970 B out, 327 B in, about 205 B per change |

**Ubuntu side (steps 3–5):** confirmed by the owner on 2 November 2026: every check matched the expected result. Timings weren't recorded.

| Check | Result |
|---|---|
| A `thumbnail of photo …` line for each of the 13 photos, and how long after connecting | ✅ As expected |
| `ls -l ~/mirrorleaf-photos`: 13 `<photo>-thumbnail.jpg` files, no full images | ✅ As expected |
| `photo list`: every photo `thumbnail here   full image not here` | ✅ As expected |
| A thumbnail opened on Ubuntu is upright and 320 px on its long side (check portrait #4 `31f59475`) | ✅ As expected |
| Live add (step 5): delay from `photo add` on the Mac to the Ubuntu thumbnail line | ✅ As expected |

### Tests 3–7

Confirmed by the owner on 2 November 2026: all steps passed and matched the expected results. No figures were recorded, so the transfer table above is still empty apart from the path for row 1.

### Pass or fail

| M2 "done when" | Result | Evidence |
|---|---|---|
| A photo added on one peer shows as a thumbnail on the other within seconds on the LAN | ✅ Pass (owner-confirmed, 2 Nov 2026) | Test 2 |
| The full image arrives on request | ✅ Pass (owner-confirmed, 2 Nov 2026) | Test 3 |
| An interrupted transfer resumes | ✅ Pass (automated test `an_interrupted_transfer_resumes`; owner-confirmed, 2 Nov 2026) | Test 7 |
| Ten sample phone photos (12 MP JPEG and HEIC) each import at ≤ 600 KB | ✅ Pass on size: 13 of 13 (8 JPEG, 5 HEIC; runs 1–2). Caveat: the JPEGs look like camera photos and the HEICs like sample files, not straight from a phone | Test 1 |

**M2 result (2 November 2026): all four "done when" items pass.** Open follow-ups, none blocking M2: transfer timings for each network row (plan §7.1) for the Phase 0 report, and an optional EXIF check on HEIC photos straight from a phone.
