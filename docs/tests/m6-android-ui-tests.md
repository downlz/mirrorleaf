# M6 Android UI: setup and test steps

| | |
|---|---|
| **Milestone** | M6 — Android UI (implementation plan §6) |
| **Devices** | The Motorola Edge 60 Pro on Android 17, and the Mac running the app (or `mirrorleaf-cli sync --watch`) as the other device, on the same Wi-Fi |
| **Covers** | M6's "done when": Android test 2 (the editor and keyboard behave well in Android's WebView) and Android test 3 (the photo picker and the share sheet integrate smoothly). Also the direction D screen on a phone. |
| **Decision rule** | If test 2 or test 3 fails, the assessment's rule applies: switch Android to option B (a native Kotlin screen), keeping the Kotlin background code and the Rust engine (plan §6, M6) |
| **Date written** | 4 November 2026 |

---

## Before you start

| Step | How |
|---|---|
| Build and install | In `apps/mirrorleaf/src-tauri`: `cargo tauri android build --debug --target aarch64 --apk --ci`, then `adb install -r gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk` |
| Account | If the phone's app isn't on an account yet: Settings (the sliders button) → Join with a ticket from the Mac app (Settings → Add a device…) |
| Logs | `adb logcat -s mirrorleaf` |
| Keyboards | Gboard (the default). If you can, repeat test 2 with a second keyboard such as SwiftKey or Samsung Keyboard. |
| Photos | Put a few photos on the phone, at least one HEIC (take one with the camera set to HEIF if it offers it, or copy `shelf-christmas-decoration.heic` from the M2 samples with `adb push … /sdcard/Pictures/`) |

---

## The screen on a phone (direction D)

| Step | Expected |
|---|---|
| Open the app | The stream fills the screen under the search field and folder chips; nothing sits under the status bar, the camera cutout or the navigation bar |
| Tap a note | It opens as a sheet over the stream |
| Swipe back (or the back button) | The sheet closes; the app stays open. With no sheet open, back leaves the app as usual |
| Settings → back gesture | Settings closes |
| Type `pack` in the search field | The stream filters, matches highlighted |
| Type a new title, press the keyboard's Enter | A note with that first line opens in a sheet, cursor after the title |
| Long-press a note | Pin, Move to and Delete |
| Long-press a folder chip | Rename and Delete |
| Turn on dark mode | The screen and the bars behind it turn dark together |

## Android test 2 — The editor and the keyboard

| # | Step | Expected |
|---|---|---|
| 2.1 | Open a note, tap at the end, type two sentences with Gboard, using its suggestions and autocorrect | Every word lands once, in order; corrections replace the word, not add to it |
| 2.2 | Glide (swipe) typing a few words | Same as 2.1 |
| 2.3 | Type until the text reaches the keyboard | The view scrolls; the cursor stays visible above the keyboard |
| 2.4 | Long-press a word | It is selected with the system's handles; drag them to widen the selection; Copy, Cut and Paste work from the system menu |
| 2.5 | Select text, then **Aa** → Bold; `/` → Checklist; tick an item | The formatting applies; the slash menu shows above or below the cursor, clear of the keyboard |
| 2.6 | Backspace across a paragraph boundary, and into a checklist item | Paragraphs join as expected; no lost or doubled text |
| 2.7 | Type in a heading and a bulleted list; press Enter at the end of a list item | A new item; Enter on an empty item leaves the list |
| 2.8 | Rotate to landscape and back while typing | The text and cursor stay; the layout adapts |
| 2.9 | Type, then press Home straight away; open the Mac | The last words arrive on the Mac (the screen sends what it holds when the app goes to the background) |
| 2.10 | Edit the same note on the Mac while it is open on the phone | The Mac's words appear in the phone's note without reopening it, and the phone's cursor doesn't jump |

Pass: no lost, doubled or jumbled text in 2.1–2.7 and 2.10; the cursor stays in view; selection and the system's copy and paste work.

## Android test 3 — The photo picker and the share sheet

| # | Step | Expected |
|---|---|---|
| 3.1 | In a note, the photo button (or `/` → Photo) | Android's own photo picker opens; no storage permission is asked for |
| 3.2 | Pick one JPEG | It appears at the cursor within a few seconds; on the Mac the thumbnail appears, then the full image when the note is opened |
| 3.3 | Pick three photos including a HEIC | All three appear; the HEIC is upright; `mirrorleaf-cli photo list` on a peer shows it at most 600 KB |
| 3.4 | Open the picker and cancel | Nothing is added, no error |
| 3.5 | Chrome → a page → Share → Mirrorleaf | A note titled with the page's title, with its address as the text, opens in Mirrorleaf |
| 3.6 | Select text in any app → Share → Mirrorleaf | A note whose first line is the first line shared |
| 3.7 | Photos (or Files) → one photo → Share → Mirrorleaf | A note "Shared photo" with the photo |
| 3.8 | Photos → select three → Share → Mirrorleaf | A note "Shared photos" with all three |
| 3.9 | Swipe Mirrorleaf away from recents, then share text to it | The app starts and opens the new note |
| 3.10 | Check the Mac | The shared notes and their photos sync |

Pass: the picker and the share sheet work in 3.1–3.9 without a crash, permission prompt or lost item.

---

## Results

### Checked by Claude on the phone (4–5 November 2026)

Driven from the Mac over adb (screenshots, taps, key and text input through Gboard) and the web view's debugger, on the Moto Edge 60 Pro, Android 17, debug build. The Mac ran `mirrorleaf-cli sync --watch` on the phone's account. Typing used `adb shell input text`, which goes through Gboard's input connection (Gboard's autocorrect showed in the events), but not real finger typing, glide typing or a second keyboard: those stay with the owner.

| Test | Result |
|---|---|
| Screen on a phone | Pass: the stream clear of the status bar, cutout and gesture bar; a note opens as a sheet; photos show through the photo protocol; search-or-create makes a note and opens it with the cursor after the title |
| Back gesture | Pass: closes the sheet and Settings (with the keyboard up, the first back hides the keyboard, as everywhere on Android); with nothing open it leaves the app. Settings' ✕, the sheet's ⌄ and a tap above the sheet also close them, with nothing left behind |
| Dark mode | Pass after a fix (finding 5): the screen and the bars behind it switch together, also while the app runs |
| 2.1 typing | Pass: lines typed reached the Mac's CLI as written |
| 2.3 keyboard and cursor | Pass: typing down to the keyboard scrolls; the cursor stays just above it |
| 2.4 selection | Pass: a long press selects with the system's handles and its Cut, Copy and Select all menu; copy and the paste key work |
| 2.5 formatting and slash menu | Pass after fixes (findings 1–2): the menu shows above the cursor, clear of the keyboard; tapping an item and Gboard's Enter both choose it, repeated for checklists, lists and headings; Aa → Bold on a selection works |
| 2.6 Backspace across blocks | Pass: joins the line onto the list item before it |
| 2.7 lists | Pass: Enter adds an item; Enter on an empty item leaves the list |
| 2.8 rotation | Pass after a fix (finding 3): the layout switches to two panes in landscape and back; text kept, cursor kept. The keyboard closes on rotation; a tap brings it back |
| 2.9 edits sent on Home | Pass: words typed just before Home reached the Mac |
| 2.10 a Mac edit in the open note | Pass: appeared within seconds; the phone's cursor didn't move |
| 3.1 photo button | Pass: Android's photo picker opens, no permission asked |
| 3.2–3.3 picking photos | Pass: one HEIC and two 12 MP JPEGs (about 20 MB) added in 12.9 s (debug build); the HEIC upright at 2048 × 1536, its full image 431.8 KB; all reached the Mac. After a fix (finding 4), photos go at the end when the note has no cursor |
| 3.5 share a web page | Pass: a note titled with the page's title, the address as its text, opened at once |
| 3.6 / 3.9 share text, app not running | Pass: the app started and opened the new note |
| 3.7 share one photo | Pass with a HEIC and with a 12 MP JPEG plus text (5.8 s to the open note, debug build). `adb`'s `am start` can't grant read access the way a gallery app does, so these used the address as the intent's data too |
| 3.8 share several photos | Not run: `am start` can't send a list of photos. Owner's step |
| Sync | All 15 notes and their photos reached the Mac (relay while the phone's VPN was on; direct once it was off, finding 6) |
| Memory, screen open (debug build) | App 264 MB PSS (debug Rust code and graphics), web view renderer about 70 MB more. Release figures are M7's |

### Findings

| # | Finding | What changed |
|---|---|---|
| 1 | On Chrome Android, ProseMirror ignores the keyboard's Enter key press and reads the new line from the text change, so the slash menu never saw Enter: it typed a new line instead of choosing | The slash extension catches the `insertParagraph` input while the menu is open, cancels it and chooses the item; it now runs before the list extensions |
| 2 | Choosing at once lost Gboard's autocorrect change (read by ProseMirror 20 ms later), and ProseMirror's Android workaround then simulated a Backspace that undid the chosen block; another workaround simulated a second Enter | The item is chosen 40 ms after Enter, and an Enter simulated within 200 ms of a choice is ignored |
| 3 | Rotating rebuilt the note view for the other layout and lost the cursor | The cursor of the last closed note is restored when it reopens (with focus if within 2 s) |
| 4 | With no cursor in the note, a picked photo went above the title; touch screens kept the photo button's hover highlight | Photos go at the end when the note isn't focused; hover styles apply only where a pointer hovers |
| 5 | Switching dark mode while the app ran left the strips behind the bars light | `MainActivity.onConfigurationChanged` sets the background and the bars' icon style again |
| 6 | During the tests the phone reached the Mac only through the relay, and couldn't ping it. Cause: a VPN app active on the phone (owner, 5 Nov). With it off, the phone pings the Mac and both ends connect **direct**; an edit on the Mac reached the phone over the direct path | None needed; a VPN on the phone forces relay connections (worth noting for the Phase 0 report) |
| 7 | Notes written by the CLI show "No additional text": the CLI doesn't write previews | None; the CLI is a test tool |

### On the phone (owner)

| Test | Result | Notes |
|---|---|---|
| Screen on a phone | | |
| 2 Editor and keyboard (2.1–2.10) | | |
| 3 Photo picker and share sheet (3.1–3.10) | | |
| Second keyboard (optional) | | |
