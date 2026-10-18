# Scrawl

Show an agent a problem instead of describing it.

Press a shortcut (`⌥⇧S`) and Scrawl starts recording the screen. Talk, click around, draw on it, and press `⌥⇧S` again to finish. Scrawl records the screen, your voice, and your marks, then packs them into a compact bundle: a contact sheet of annotated keyframes (`contact-sheet.png`), individual full-resolution keyframes (`frames/frame-XX.png`), a time-aligned transcript and interaction timeline (`summary.md`), and a structured manifest (`scrawl.json`).

## Usage

```sh
# Launch the GPUI menu-bar and review app (tap ⌥⇧S to record, hold it to choose what to record)
cargo run -- app

# Generate a realistic multi-frame demo bundle and print its agent summary
cargo run -- demo --out /tmp/demo.scrawl

# Open a bundle (or `latest`) in the native GPUI review window
cargo run -- preview /tmp/demo.scrawl

# Print the agent summary or JSON manifest for a bundle
cargo run -- inspect /tmp/demo.scrawl
cargo run -- inspect /tmp/demo.scrawl --json

# Extract frames in a time window for an agent to look closer
cargo run -- frames /tmp/demo.scrawl --from 0:02 --to 0:07

# Copy contact-sheet.png + summary.md to the clipboard or send to OpenCode V2
cargo run -- send /tmp/demo.scrawl --clipboard
cargo run -- send /tmp/demo.scrawl --session ses_...

# Capture a cabled iPhone for 8 seconds from the terminal (mirror window + native video)
cargo run -- record --iphone --duration 8
cargo run -- devices

# Build a bundle from a ScrawlKit in-app recording (newest pair in ~/Downloads by default)
cargo run -- import
cargo run -- import ~/Downloads/20261013T164000.mp4 --timeline ~/Downloads/20261013T164000.json

# Rebuild an existing bundle with the current transcriber and keyframe rules
cargo run -- reprocess latest --out /tmp/again.scrawl

# Tune auto-crop on a Screen capture, live
cargo run -- autocrop latest --serve --open
```

## Welcome and permissions

On first launch Scrawl opens a welcome window: what it does, how ⌥⇧S works, then each permission
on its own step, then a first capture. A step's check appears by itself once macOS grants it (it
re-reads the grants every 0.5 s), and steps already granted are passed.

| Permission | Used for | |
|---|---|---|
| Screen & System Audio Recording | Recording the screen (ScreenCaptureKit) | required |
| Microphone | Your narration | required |
| Input Monitoring | The event tap that sees clicks and ⇧-drags in every app | required |
| Accessibility | `⌥⇧V`, which posts ⌘V into the app in front | optional |
| Camera | The cabled iPhone mirror (macOS treats it as a camera) | optional |

Speech recognition isn't asked for: SpeechAnalyzer transcribes without it. Captures never prompt
for Screen Recording; without it, ⌥⇧S opens that step instead. After granting Screen Recording macOS
may need Scrawl relaunched; the step offers **Relaunch Scrawl** and the tour resumes after it.
Progress is `"onboarding"` in `~/.config/scrawl/config.json`: once seen, the tour only comes back
from **Welcome to Scrawl…** in the menu, but a required permission that was granted and is gone
again reopens its own step at launch. `SCRAWL_NO_ONBOARDING=1` skips it.

To replay it from scratch:

```sh
tccutil reset All dev.kitlangton.scrawl
jq 'del(.onboarding)' ~/.config/scrawl/config.json > /tmp/c.json && mv /tmp/c.json ~/.config/scrawl/config.json
# then quit and reopen Scrawl
```

Preview every step without the app: `target/debug/scrawl-helper onboarding --render /tmp/onboarding
--icon assets/app-icon.png`.

## Capture Controls

- **Tap `⌥⇧S`** (release within 0.25 s): record the display under the pointer straight away,
  auto-cropped (see below). Tap it again to finish; a tap within 0.4 s of the start is ignored, so a
  double press doesn't leave an empty capture. `⌃⌥⇧S` also works, as does your own shortcut if you
  set one in the review window. As it starts, the island opens onto the modes (Area · Window ·
  **Screen** · iPhone when one is cabled) for about 3.5 s, or until you click, scroll or draw;
  hovering keeps them. Click Window or Area to retarget (the first seconds are dropped and the
  picker takes over: click a window or drag an area), iPhone to record the phone instead, or just
  carry on.
- **Hold `⌥⇧` after `S`**, like `⌘Tab`: after 0.25 s the switcher appears on **Screen**. Each
  further `S` steps Screen → Window → iPhone → Screen; Window highlights the window under the
  pointer. Dragging with the mouse selects an area instead. Release `⌥⇧` to record what's
  selected; `Esc` cancels. With no iPhone cabled, iPhone says so and releasing records nothing.
- **Choose What to Record…** in the menu bar: the full picker (Area, Window, Screen, iPhone).
- `⌥⇧V`: Paste both `contact-sheet.png` and `summary.md` into the active app or terminal (`⌃⌥⇧V` also works)
- **Normal pointer**: Clicks and scrolls pass straight through to the app underneath
- **Hold `⇧` (Shift) + move**: Draw a persistent red marker stroke (`#ff3b30`) on screen that fades out after release
- **`⌘Z`**: Undo the last stroke
- **`Esc`**: Cancel the active selection or capture

While picking a target, the island shows the modes **Area**, **Window**, **Screen**, and **iPhone**
(when one is cabled). It opens on the mode you used last:

- `1`–`4` jump to a mode; `Tab`/`→` and `⇧Tab`/`←` step through them; `Space` toggles Area and Window
- Area: drag a region. Window: click a window. Screen and iPhone: `⏎`, `S`, `⌥⇧S`, **Record**, or
  clicking the selected mode again starts recording (`⏎`/`⌥⇧S` record the full screen from Area and
  Window too)

## Auto-crop (Screen captures)

Instead of picking an area up front, record the whole screen: tap `⌥⇧S`, or choose **Screen**.
Area and Window captures aren't cropped.
Scrawl works out where you were gesturing and crops each keyframe to it. While a Screen capture
records, Scrawl logs:

- **`windows`** in `scrawl.json`: the on-screen windows, front to back (id, app, bundle ID, title,
  bounds in capture points, layer, frontmost), including the menu bar and the Dock's strip. A
  snapshot is taken at the start, when a click, ink stroke or shortcut starts, when the frontmost
  app changes, at most every 500 ms while the cursor moves and every 2 s otherwise, and stored only
  when the list changed. Each snapshot costs about 0.5 ms. Titles stay in `scrawl.json`; they never
  go into `summary.md` or the clipboard.
- **`cursor`**: the cursor at up to 15 Hz while it moves, as `[t, x, y]` (normalized), so dwell
  counts.

When the capture finishes, each keyframe gets the slice of the timeline halfway to its neighbours,
and `src/autocrop.rs` weighs what happened in it: ink, clicks, drags and scrolls first, then where
the cursor rested and what changed on screen (a cursor wandering far from the ink is ignored). The
crop is then:

- **the window** holding the activity (two at most, for an arrow from one to the other), including
  a menu it opened and the touched part of the menu bar;
- or, when that window is most of the screen (maximized, full screen), **the activity inside it**
  with some context, its edges moved into nearby white space and past the ends of any lines of
  text they cross (up to the window's edge), so they don't cut words in half;
- or the previous crop, for a keyframe with only talk.

Neighbouring crops that mostly overlap become one, so the sheet doesn't jitter. A crop covering
most of the screen isn't worth it, and the keyframe stays whole.

The bundle then has:

- `frames/frame-XX.png` and `contact-sheet.png`: cropped from the full-resolution frames (Screen
  keyframes are decoded from the 2560 px wide recording), with the ink and badges redrawn at the crop's scale;
- `frames-full/frame-XX.png` and `contact-sheet-full.png`: the uncropped versions;
- one line in `summary.md`, e.g. `Cropped to: Ghostty + Safari (full frames in frames-full/)`;
- `autocrop` in `scrawl.json`: each keyframe's crop in points, its kind and reason, and the
  changed regions that fed it.

`scrawl zoom` crops its cells like the nearest keyframe (`--full` for the whole screen).
`scrawl reprocess <bundle> --out <dir>` re-runs it on an existing Screen capture from its video.
To turn it off, or change a setting, use `~/.config/scrawl/config.json`:

```json
{ "autocrop": false }
{ "autocrop": { "padding": 24, "minW": 480 } }
```

### Tuning

`scrawl autocrop <bundle|latest> --serve --open` opens a playground that runs the same Rust crop as
you move its sliders: each keyframe with its windows, activity, cursor heat, outliers and crop, the
cropped frame, and the cropped contact sheet. **Copy settings** gives the JSON for the config.
Without `--serve` it writes `autocrop/index.html` with the presets only.

The settings were tuned on simulated captures with hand-labelled ideal crops: one maximized
terminal, two windows side by side, an arrow from one window to another, a tiny badge in a huge
window, a menu-bar menu, talk-only keyframes, a dragged window, overlapping windows, thirty seconds
hopping between three windows, a stray cursor, a full-screen app, and a scrolling page.
`scripts/autocrop-corpus.mjs` renders them with Playwright (from `site/node_modules`), `scrawl
fixture` builds each into a real bundle through the capture pipeline, `scrawl autocrop-eval` scores
the crops (coverage of the ink and clicks, overlap with the ideal, context, legibility in the sheet,
stability; `--sweep` searches the settings), and `scripts/autocrop-gallery.mjs` renders a gallery to
judge them by eye:

```sh
bun scripts/autocrop-corpus.mjs /tmp/corpus
for s in /tmp/corpus/*/scenario.json; do
  scrawl fixture "$s" --out "/tmp/bundles/$(basename "$(dirname "$s")").scrawl"
done
scrawl autocrop-eval /tmp/bundles --json /tmp/scores.json   # add --sweep to search
bun scripts/autocrop-gallery.mjs /tmp/gallery /tmp/bundles --scores /tmp/scores.json
```

`autocrop-eval --export src/autocrop-corpus.json` refreshes the corpus the unit tests score.

## iPhone

Plug an iPhone in with a cable (unlocked, trusted) and the picker shows **iPhone** next to Area,
Window, and Screen (also **Record iPhone** in the menu bar). Choosing it opens a floating mirror of the
phone on the right of the screen and starts recording:

- The phone's own video is recorded natively (1320×2868 at up to 60 fps on an iPhone Pro Max) with
  your narration from the Mac's microphone, into the bundle as `recording.mov`. Keyframes are
  decoded from that video, never from a screenshot of the mirror.
- The laser, ink, pins, and clicks work over the mirror exactly as over a window. The mirror's
  on-screen rect is the capture target, so everything you draw is stored in phone-screen
  coordinates and lands in the right place on the phone keyframes. Move or resize the mirror
  freely; the mapping follows it.
- Press **M** with the mirror focused to flag a moment ("look here"); flags always get a keyframe
  and show as `⚑` in the summary.
- Finish from the island as usual. Unplugging the phone finishes the capture with what was recorded.

Phone bundles are ordinary bundles: `scrawl.json` adds `"source": "iphone"` and a `device` object
(name, capture `mirror` or `scrawlkit`, id, native size, screen points and scale, app and version
for SDK recordings), and the target's `mode` is `device`. Keyframes fit a 1600 px long edge
(736×1600 for a 1320×2868 phone); `scrawl frames` extracts native-resolution frames (via `ffmpeg`
or `scrawl-helper frames`) when you need every pixel.

### ScrawlKit (iOS)

`ScrawlKit` (`Sources/ScrawlKit`) is a Swift package (iOS 18+) for recording inside your own iOS app:
video of the app with narration, touches and freehand red strokes, markers, and app events on the
same clock, sent straight to Scrawl on your Mac.

Add this repo as a Swift package dependency (product `ScrawlKit`) and attach `.scrawl()` to your root
view:

```swift
import ScrawlKit

WindowGroup { ContentView().scrawl() }   // three-finger long press starts and stops
Scrawl.event("Done tapped")              // optional: what the app did, on the timeline
```

The host app needs `NSMicrophoneUsageDescription` in its Info.plist for narration (both capture
methods use the microphone). Set `Scrawl.shared.narration = false` to record without it.

`Scrawl.shared.captureMethod` picks how the video is made:

- **`.appFrames`** (default): ScrawlKit draws the app's own windows into an H.264 `.mp4` at 30 fps
  and 2× (880×1912 on a 440×956-point phone), with the microphone recorded alongside. There is no
  screen-recording prompt; the microphone asks once and iOS remembers the answer. If it's denied, the
  recording is video only and the timeline says `"audio": false`. The strip and ink are left out of
  the video (Scrawl on the Mac draws touches and strokes from the timeline), but so is
  anything the app doesn't draw itself: system alerts and permission prompts, and some layers (video
  players, camera previews, some Metal or backdrop effects) may be blank or look different. The
  system keyboard is drawn by another process, so its keys can't be captured: a plain panel labelled
  "Keyboard" stands in for it, following it as it slides (taps on the keyboard never reach the app,
  so they aren't on the timeline either). Sending the app to the background finishes and sends the
  recording.
- **`.replayKit`**: ReplayKit's in-app screen recording, the exact pixels on screen with the strip
  and the keyboard in them. iOS asks "Record screen and microphone?" unless you allowed it in the
  last ~8 minutes, and again after every reinstall.

The timeline records which was used (`"capture": "appFrames"` or `"replayKit"`). With either, the
context records the keyboard: `keyboard: visible` if it was up at the start, and
`keyboard → visible` / `keyboard → hidden` when it comes and goes.

On paired devices, the three-finger trigger turns off iOS's three-finger text-editing gestures in
the app (UIKit's `editingInteractionConfiguration` answers `.none` for the app's windows), because
the same press would otherwise open the Undo · Cut · Copy · Redo bar when a text field is focused.
The cost is that three-finger undo, redo, cut, copy, and paste don't work in that app on a paired
phone. Set `Scrawl.shared.suppressesSystemEditingGestures = false` to keep them. Unpaired devices
are unaffected.

While recording, a slim strip sits on the right edge (drag its dot to move it up or down or over to
the left edge; it remembers where you left it). From top to bottom: the recording dot (hold it to see
the elapsed time), Draw (tap to turn drawing red strokes on or off), Discard (with a few seconds to
Undo), and Send. Doing the three-finger trigger again also sends. The strip moves like liquid (the
"Liquid morph" variant of `site/public/prototype/strip`): it stretches out of a droplet, pours into
the button you tap to become a status circle, buds the "Sent to …", "Discarded · Undo" or "Mac didn't
answer · Share" bubble off on a neck, and stretches as you throw it. On iOS 26 it's Liquid Glass and
the neck is the glass itself blending; Reduce Motion cross-fades instead.

Drawn strokes are smooth and fade on their own:

- **Sampling.** Every coalesced touch is a sample, at the touch's own timestamp, so a busy main
  thread doesn't make strokes jagged. A one-euro filter (one cutoff from the 2D speed) takes out
  jitter as they come in, and a centered smoothing takes out the rest behind the tip without lag.
- **Look.** The line is a centripetal Catmull-Rom curve, 4.6 pt wide when drawn slowly and down to
  55% when fast (from speed, never pressure, like the Mac's ink), with tapered ends.
- **Drawing.** Finished stretches go into layers that never change, so each touch redraws only the
  live tip, which runs ahead through iOS's predicted touches. Predicted points are dropped on the
  next touch and never recorded.
- **Fading.** A stroke fades out (300 ms, ease-out) 1.5 s after the pen last lifted. Putting the pen
  down again before then holds every stroke on screen, so a circle and then an arrow fade together.
- **Timeline.** It gets the smoothed, unpredicted points, plus `fadeAt` (when the stroke began
  fading) on each stroke that faded during the recording. On keyframes the Mac draws a stroke from
  when it began until 1.2 s after it ended, or with the rest of its mark.

Touches aren't drawn on screen in either capture method. Each touch goes on the timeline (position,
phase, and the label of the element under it) and Scrawl on the Mac draws taps and swipes from it.

ScrawlKit stays completely inert on devices that haven't been configured (no gesture recognizers,
overlay window, network calls, files, or background `URLSession`). To pair your iPhone with this Mac, print a setup link (and
terminal QR code) and open it on the phone:

```sh
scrawl link --scheme updown [--session ses_…]
# updown://scrawl?url=http%3A%2F%2F100.64.0.12%3A47777&url=http%3A%2F%2F192.168.1.20%3A47777&token=…
```

`scrawl link` reads (or generates with mode `0600`) `ingestToken` in
`~/.config/scrawl/config.json` and defaults to this Mac's Tailscale and LAN IPs on port `47777`.
Opening `<scheme>://scrawl?reset=1` on the phone clears its Keychain config (`dev.kitlangton.scrawl`).
For cabled USB fallback via `devicectl` (`Documents/Scrawl`), list the app bundle IDs in
`~/.config/scrawl/config.json` under `"usbApps": ["com.kitlangton.UpDown"]`.

You can also AirDrop the `.mp4` and `.json` pair to the Mac and run `scrawl import`:

- Touches become taps and swipes, drawn on keyframes as ripples and finger trails and listed in the
  review timeline. Each tap or swipe is a keyframe candidate, like a click.
- Markers become flags.
- App events (`Scrawl.event("Done tapped")`) appear in `summary.md` under the keyframe that was on
  screen when they happened (`  · 0:12 app: Done tapped`).

#### Sending to an HTTPS endpoint

A setup link can point at a hosted endpoint instead of a Mac, for phones that aren't on your LAN or
tailnet (for example `ledger://scrawl?url=https%3A%2F%2Fproblems.example.com%2Fapi%2Fscrawl%2Fledger&token=pbcap_…`).
It speaks the same protocol as the Mac: `GET {url}/health` and `POST {url}/ingest` with a bearer
token, where the body is a 4-byte big-endian timeline length, the timeline JSON, then the MP4. When
any configured URL is `https`, recordings go through a durable outbox instead of the LAN path
(plain `http` URLs keep the direct POST plus USB fallback):

- **Saved before sending.** On Send, the framed payload and a small metadata file (target,
  `recordedAt`, attempts, last error) are written to `Application Support/ScrawlOutbox`. They're
  protected until the phone's first unlock, so uploads can run while it's locked, and excluded from
  backup. The recording never goes to `Documents/Scrawl` (it's staged in `tmp`) and isn't added to
  `Scrawl.shared.recordings`. An entry is deleted only after a 2xx.
- **Background upload.** Uploads are file uploads on a background `URLSession`
  (`dev.kitlangton.scrawl.outbox.<bundle ID>`), so they survive the app being suspended or killed
  and never load the movie into memory. The session is recreated whenever the app is configured, and
  it picks up uploads still in flight from an earlier launch.
- **Retries.** A 401 or 403 (the link was revoked), a 413 (too large) and any other 4xx except 408
  and 429 (the server rejected these exact bytes) are permanent: the entry stays on disk, marked
  failed, with Share. Anything else (offline, timeouts, 408, 429, 5xx) is retried
  with backoff (15 s, doubling up to 30 min), and immediately when the network comes back, the app
  comes to the foreground, or it launches. An entry never uploads twice at once. The server dedupes
  on person and `recordedAt`, so resending after a lost reply is safe. Opening a new setup link
  retries the entries that failed with the old one.
- **Strip.** While the upload runs on a working network (`NWPathMonitor` says `.satisfied`), the
  strip stays up as a small status circle with a progress ring, saying "Sending…" however long it
  takes. Touches on it reach the app. With no bytes going out for 30 seconds it says "Still
  sending…", never "Queued". When the upload lands it turns into "Sent to …" with a success haptic.
  "Queued · sends when online" (with a success haptic, since the recording is safe on the phone) is
  only for no network or a failed attempt waiting to retry; it leaves after a few seconds, and if a
  retry starts while it's up it goes back to "Sending…". If the upload lands after the strip has
  gone (a new recording took over, it was queued, or the app was in the background), the strip comes
  back briefly with "Sent to …", as soon as the app is in front. Permanent failures say "Scrawl link
  expired · Ask Kit for a new one", "Too large to send" or "Couldn't send", with Share, which shares
  copies of the recording and leaves the entry queued.
- **Revoked links.** The `/health` check at the start of a recording also goes to the HTTPS
  endpoint. A 401 there shows "Scrawl link expired" beside the strip, but the recording carries on
  and is kept in the outbox, marked failed, until the link is renewed.

The endpoint rejects bodies over 95 MB, so HTTPS recordings are smaller and capped:

| Capture | Video | Audio | Per second | Cap | At the cap |
| --- | --- | --- | --- | --- | --- |
| `.appFrames` to a Mac | 6 Mbps | 96 kbps | ≈ 0.76 MB | none | — |
| `.appFrames` to HTTPS | 3 Mbps | 96 kbps | ≈ 0.39 MB | 3:00 | ≈ 70 MB |
| `.replayKit` to HTTPS | not settable (est. 8–12 Mbps) | | ≈ 1–1.5 MB | 0:45 | ≈ 45–68 MB |

3 Mbps at 880×1912 and 30 fps is about 0.06 bits per pixel. That's plenty for app UI, which is mostly
static between scrolls and transitions, and the 25 MB of headroom covers the encoder overshooting its
average. Ten seconds before the cap, the strip shows "Length limit · Sending in 10 s" with a warning
haptic, then sends. A payload that's still over the limit is never uploaded: it's marked "Too large
to send" and kept for Share.

Apps with an app delegate can optionally forward background session events, so iOS can relaunch a
killed app to finish reporting uploads. Without this, finished uploads are picked up at the next
launch:

```swift
func application(_: UIApplication, handleEventsForBackgroundURLSession identifier: String,
                 completionHandler: @escaping () -> Void) {
  Scrawl.handleBackgroundURLSessionEvents(identifier: identifier, completionHandler: completionHandler)
}
```

#### App context

The app can say which screen it's on and add its own key/values, so the agent doesn't have to guess
from pixels. ScrawlKit sends only what the app passes in, plus its bundle ID, name, version, and build.

```swift
SettingsView().scrawlScreen("Settings › Experimental")   // while visible; restores the previous on disappear
SessionView().scrawlContext("session", session.id)       // follows the value; nil leaves it to enclosing views
Scrawl.screen("Onboarding")                              // or set either one directly (nil clears)
Scrawl.context("server", "studio")
```

The most recently appeared screen that is still visible wins, so pushed screens, sheets, and tabs
work. The timeline gets `shortVersion`, `context` (values at the start), `contextChanges`
(`[{"t", "key", "value"}]`, `null` when cleared), and `screens` (`[{"t", "name"}]`, the first at `0`).
The bundle keeps them as `context`, `contextChanges`, and `screens` spans (`{t0, t1, name}`), and
`summary.md` (and the prompt queued into OpenCode) shows them:

```
[scrawl 0:25 · iPhone · OpenCode 1.4 (354) · /…/20261014-101500.scrawl]
Screen: Sessions · server: studio · session: ses_3f…
- #1 (0:01) “So here.”
- #3 (0:08) ⟨Settings › Experimental⟩ [tapped “Back”] “Go back.”
  · 0:09 server → mini
```

### Headless ingest server (`scrawl serve`)

On an always-on Mac (such as a Mac Studio reachable over Tailscale), run `scrawl serve` to accept
authenticated ScrawlKit uploads, build `.scrawl` bundles, and route them straight into local
OpenCode sessions without running the GUI app:

```sh
scrawl serve [--port 47777] [--usb]
```

To run `scrawl serve` under `launchd`, copy `scripts/scrawl-serve.plist` into `~/Library/LaunchAgents`
and load it:

```sh
cp scripts/scrawl-serve.plist ~/Library/LaunchAgents/dev.kitlangton.scrawl.serve.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/dev.kitlangton.scrawl.serve.plist
```

## Transcription

Narration is transcribed on device by Apple's SpeechAnalyzer (macOS 26), which reports when every
word was spoken. The word timings drive two things:

- **Text per keyframe.** Each keyframe's text runs from the start of the clause in progress when
  its moment began (the action, the transition, or the spoken cue) until the next keyframe, so the
  transcript is split at sentence and clause boundaries, in order, with nothing repeated or lost.
- **Spoken cues.** Saying "now", "here", "look", or "watch" is a deliberate cue: if the screen
  changes around the word, that settled view becomes a keyframe; otherwise the frame as you said it.
  A cue only cuts a keyframe if its image differs from every keyframe already chosen, so talking
  over an unchanged screen still produces one frame. "This", "that", "these", "those", "see", and
  "there" count only when they coincide with a change, because they are usually filler. Speech
  pauses never cut keyframes.

## Helper

`scrawl-helper` is a small Swift executable built by `build.rs` from `helper/` and shipped inside
`Scrawl.app` next to `scrawl`. It owns the Swift-only or unbound Apple frameworks: SpeechAnalyzer,
CoreMediaIO/AVFoundation iPhone capture with the mirror window, AVFoundation movie decoding, and the
SwiftUI welcome window (`onboarding`).
Scrawl runs it as a subprocess (JSON on stdout; `stop`/`cancel` on stdin for the mirror). Run it
directly to test a piece in isolation:

```sh
target/debug/scrawl-helper devices
target/debug/scrawl-helper permissions
target/debug/scrawl-helper transcribe audio.wav
target/debug/scrawl-helper frames recording.mov --out-dir /tmp/f --at 1000,2000 --max-edge 1600
```

## Testing

```sh
taskpolicy -c utility cargo test -j 3 --offline -- --test-threads=2
./scripts/install-app.sh
```

- **iPhone:** connect a phone, hold `⌥⇧` and press `S` three times, release, talk, draw over the mirror, press M,
  Finish. Inspect the bundle's `summary.md`, `scrawl.json` (`source`, `device`), contact sheet, and
  frames.
- **ScrawlKit:** `scrawl import` on a recording and its timeline (or hand-write a timeline for any
  phone video), then check the ripples and trails on frames and the app-event lines in the summary.
  `swift test` runs the outbox tests on the Mac against a `URLProtocol` stub of the ingest endpoint
  (persisting, retry classification, resends, deletion only on 2xx, and what the strip says through
  slow uploads, no network, and retries).
- **Transcription:** `scrawl-helper transcribe <wav or movie>` prints words with timings;
  `scrawl reprocess <bundle> --out …` reruns transcription and keyframe selection on a real bundle.
