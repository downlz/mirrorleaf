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

**Ubuntu**, either:

- **From CI:** after you commit and push, open the CI run's page on GitHub, download the `mirrorleaf-cli-x86_64-unknown-linux-gnu` artifact, unzip it, then run `chmod +x mirrorleaf-cli`; or
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

---

## Results

Fill in and keep with the M2 notes; these figures go into the Phase 0 report (plan §7.4).

### Import size (test 1)

| # | File | Format | Original size | Output dimensions | Full image size | Quality | EXIF kept | Thumbnail size | ≤ 600 KB? |
|---|---|---|---|---|---|---|---|---|---|
| 1 | | | | | | | | | |
| 2 | | | | | | | | | |
| 3 | | | | | | | | | |
| 4 | | | | | | | | | |
| 5 | | | | | | | | | |
| 6 | | | | | | | | | |
| 7 | | | | | | | | | |
| 8 | | | | | | | | | |
| 9 | | | | | | | | | |
| 10 | | | | | | | | | |

### Transfer (tests 2, 3 and 6)

| Network row | Path (direct / relay) | Thumbnail: delay after `photo add` | Thumbnail transfer time | Full image transfer time | Text edit time (sending side) |
|---|---|---|---|---|---|
| 1 Same Wi-Fi | | | | | |
| 2 Phone hotspot | | | | | |
| 3 Different Wi-Fi | | | | | |
| 4 UDP blocked | | | | | |

### Pass or fail

| M2 "done when" | Result | Evidence |
|---|---|---|
| A photo added on one peer shows as a thumbnail on the other within seconds on the LAN | | Test 2 |
| The full image arrives on request | | Test 3 |
| An interrupted transfer resumes | | Test 7 |
| Ten sample phone photos (12 MP JPEG and HEIC) each import at ≤ 600 KB | | Test 1 |
