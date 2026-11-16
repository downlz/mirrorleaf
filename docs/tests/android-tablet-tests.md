# Android tablets: test steps and results

| | |
|---|---|
| **Work** | Mirrorleaf on Android tablets: the same app as on phones (owner's request, 16 November 2026) |
| **Decisions** | ADR-015 |
| **Date written** | 16 November 2026 |
| **Status** | Checked at tablet sizes in a headless browser; the arm64 APK built and signed; to test on a real tablet |

---

## What changed

| Area | Change |
|---|---|
| Wording | "Your phone or tablet" where either is meant; "this tablet" for the device itself (the lock's message too) |
| Device name | The name in Android's settings (About → Device name), such as "Galaxy Tab S9", instead of a model code such as "SM-X710", for devices joining from now on |
| Everything else | Unchanged: the screen already adapts to a tablet's width, rotation and split screen |

## The screen at tablet sizes (Claude, 16 November 2026)

Headless Firefox with a stand-in for Tauri (a tablet named "Galaxy Tab S9" on an account with the Mac and the phone; harness in the session scratchpad).

| Size | Result |
|---|---|
| Landscape, 1280 × 800 (a 10–11″ tablet) | ✅ Wide layout: the notes and folders on the left, the open note on the right, as on the Mac |
| Portrait, 800 × 1280 | ✅ Medium layout: the same, with a narrower list |
| Settings, portrait | ✅ A centred panel; "this tablet tells other devices…", "this tablet sends counts…" |
| Phone width (split screen) | ✅ The phone layout, unchanged |

Found: three lines said "your phone" where a tablet is meant too, and the lock's message said "this phone"; fixed. All devices show a laptop icon in Settings → Devices (not tablet-specific; later).

## Build

| Build | Result |
|---|---|
| arm64 release APK, signed with the phone builds' key | ✅ `Mirrorleaf_0.5.0_android-arm64.apk` (0.5.0, versionCode 5000): 33.6 MB file, about 14.9 MB to download (limit 40 MB); Android reads it as supporting small to extra-large screens at any density; signature verified (APK Signature Scheme v2, the debug key's SHA-256 `b0e85bf1…`) |

---

## Tests for the owner (an Android tablet)

Install the APK (allow installing from the file manager when Android asks).

1. **Join your account** with a word code from the Mac or the phone (Settings → Add a device). **Expect:** the tablet appears on your other devices under its own name (such as "Galaxy Tab S9").
2. **Landscape:** the notes beside the open note. **Portrait:** the same, with a narrower list. Rotate while editing: the cursor stays where it was.
3. **Split screen** with another app: Mirrorleaf switches to the phone layout; widen it again: back to the two panes.
4. **Keyboard** (if the tablet has one): Ctrl+N makes a note, Ctrl+K or Ctrl+F goes to search, Esc leaves a folder.
5. **Settings → This tablet**: battery guidance for the maker; Settings → App lock uses the tablet's screen lock.
6. **Waking**: close Mirrorleaf on the tablet, edit a note on the Mac: it arrives on the tablet within about 10 s (tablets with Google Play).

---

## Results

| Test | Result |
|---|---|
| Screen at tablet sizes | ✅ Claude, 16 Nov 2026 |
| 1–6 | Owner |
