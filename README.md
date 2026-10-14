# Scrawl

Show an agent a problem instead of describing it.

Press a shortcut (`⌥⇧S`), then talk, click around, and draw on the screen. Scrawl records the screen, your voice, and your marks, then packs them into a compact bundle: a contact sheet of annotated keyframes (`contact-sheet.png`), individual full-resolution keyframes (`frames/frame-XX.png`), a time-aligned transcript and interaction timeline (`summary.md`), and a structured manifest (`scrawl.json`).

## Usage

```sh
# Launch the GPUI menu-bar and review app (global shortcut: ⌥⇧S)
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
```

## Capture Controls

- `⌥⇧S`: Start or finish a capture session from any app (`⌃⌥⇧S` also works)
- `⌥⇧V`: Paste both `contact-sheet.png` and `summary.md` into the active app or terminal (`⌃⌥⇧V` also works)
- **Normal pointer**: Clicks and scrolls pass straight through to the app underneath
- **Hold `⇧` (Shift) + move**: Draw a persistent red marker stroke (`#ff3b30`) on screen that fades out after release
- **`⌘Z`**: Undo the last stroke
- **`Esc`**: Cancel the active selection or capture

While picking a target, the island shows the modes **Area**, **Window**, **Screen**, and **iPhone**
(when one is cabled):

- `1`–`4` jump to a mode; `Tab`/`→` and `⇧Tab`/`←` step through them; `Space` toggles Area and Window
- Area: drag a region. Window: click a window. Screen and iPhone: `⏎`, `S`, `⌥⇧S`, **Record**, or
  clicking the selected mode again starts recording (`⏎`/`⌥⇧S` record the full screen from Area and
  Window too)

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
  anything the app doesn't draw itself: the system keyboard, system alerts and permission prompts,
  and some layers (video players, camera previews, some Metal or backdrop effects) may be blank or
  look different. Sending the app to the background finishes and sends the recording.
- **`.replayKit`**: ReplayKit's in-app screen recording, the exact pixels on screen with the strip in
  them. iOS asks "Record screen and microphone?" unless you allowed it in the last ~8
  minutes, and again after every reinstall.

The timeline records which was used (`"capture": "appFrames"` or `"replayKit"`).

While recording, a slim strip sits on the right edge (drag its dot to move it up or down or over to
the left edge; it remembers where you left it). From top to bottom: the recording dot (hold it to see
the elapsed time), Draw (tap to turn drawing red strokes on or off), Discard (with a few seconds to
Undo), and Send. Doing the three-finger trigger again also sends. On iOS 26 the strip is Liquid
Glass and morphs between states (it shrinks to a status circle while sending, and the "Sent to …" or
"Discarded · Undo" bubble grows out of it); Reduce Motion cross-fades instead.

Touches aren't drawn on screen in either capture method. Each touch goes on the timeline (position,
phase, and the label of the element under it) and Scrawl on the Mac draws taps and swipes from it.

ScrawlKit stays completely inert on devices that haven't been configured (no gesture recognizers,
overlay window, or network calls). To pair your iPhone with this Mac, print a setup link (and
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
word was spoken. Hex's local API is the fallback when SpeechAnalyzer is unavailable (set
`SCRAWL_TRANSCRIBER=hex` to force it). The word timings drive two things:

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
CoreMediaIO/AVFoundation iPhone capture with the mirror window, and AVFoundation movie decoding.
Scrawl runs it as a subprocess (JSON on stdout; `stop`/`cancel` on stdin for the mirror). Run it
directly to test a piece in isolation:

```sh
target/debug/scrawl-helper devices
target/debug/scrawl-helper transcribe audio.wav
target/debug/scrawl-helper frames recording.mov --out-dir /tmp/f --at 1000,2000 --max-edge 1600
```

## Testing

```sh
taskpolicy -c utility cargo test -j 3 --offline -- --test-threads=2
./scripts/install-app.sh
```

- **iPhone:** connect a phone, open the picker, choose iPhone, talk, draw over the mirror, press M,
  Finish. Inspect the bundle's `summary.md`, `scrawl.json` (`source`, `device`), contact sheet, and
  frames.
- **ScrawlKit:** `scrawl import` on a recording and its timeline (or hand-write a timeline for any
  phone video), then check the ripples and trails on frames and the app-event lines in the summary.
- **Transcription:** `scrawl-helper transcribe <wav or movie>` prints words with timings;
  `scrawl reprocess <bundle> --out …` reruns transcription and keyframe selection on a real bundle.
