# Scrawl

Show an agent a problem instead of describing it.

Press a shortcut (`⌃⌥⇧S`), then talk, click around, and draw on the screen. Scrawl records the screen, your voice, and your marks, then packs them into a compact bundle: a contact sheet of annotated keyframes (`contact-sheet.png`), individual full-resolution keyframes (`frames/frame-XX.png`), a time-aligned transcript and interaction timeline (`summary.md`), and a structured manifest (`scrawl.json`).

## Usage

```sh
# Launch the GPUI menu-bar and review app (global shortcut: ⌃⌥⇧S)
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

# Build a bundle from a TetherKit in-app recording (newest pair in ~/Downloads by default)
cargo run -- import
cargo run -- import ~/Downloads/20261013T164000.mp4 --timeline ~/Downloads/20261013T164000.json

# Rebuild an existing bundle with the current transcriber and keyframe rules
cargo run -- reprocess latest --out /tmp/again.scrawl
```

## Capture Controls

- `⌃⌥⇧S`: Start or finish a capture session from any app
- `⌃⌥⇧V`: Paste both `contact-sheet.png` and `summary.md` into the active app or terminal
- **Normal pointer**: Clicks and scrolls pass straight through to the app underneath
- **Hold `⇧` (Shift) + move**: Draw a persistent red marker stroke (`#ff3b30`) on screen that fades out after release
- **`⌘Z`**: Undo the last stroke
- **`Esc`**: Cancel the active selection or capture

## iPhone

Plug an iPhone in with a cable (unlocked, trusted) and the picker shows **iPhone** next to Area
and Full Screen (also **Record iPhone** in the menu bar). Choosing it opens a floating mirror of the
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
(name, capture `mirror` or `tetherkit`, id, native size, screen points and scale, app and version
for SDK recordings), and the target's `mode` is `device`. Keyframes fit a 1600 px long edge
(736×1600 for a 1320×2868 phone); `scrawl frames` extracts native-resolution frames (via `ffmpeg`
or `scrawl-helper frames`) when you need every pixel.

### TetherKit recordings

[TetherKit](../tether) records inside your own iOS app (ReplayKit video plus a `.json` timeline of
touches, markers, and app events). AirDrop the pair to the Mac and run `scrawl import`:

- Touches become taps and swipes, drawn on keyframes as ripples and finger trails and listed in the
  review timeline. Each tap or swipe is a keyframe candidate, like a click.
- Markers become flags.
- App events (`Tether.event("Done tapped")`) appear in `summary.md` under the keyframe that was on
  screen when they happened (`  · 0:12 app: Done tapped`).

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

- **iPhone:** connect a phone, quit the standalone Tether app if it is running (`pkill -x Tether`),
  open the picker, choose iPhone, talk, draw over the mirror, press M, Finish. Inspect the bundle's
  `summary.md`, `scrawl.json` (`source`, `device`), contact sheet, and frames.
- **TetherKit:** `scrawl import` on a recording and its timeline (or hand-write a timeline for any
  phone video), then check the ripples and trails on frames and the app-event lines in the summary.
- **Transcription:** `scrawl-helper transcribe <wav or movie>` prints words with timings;
  `scrawl reprocess <bundle> --out …` reruns transcription and keyframe selection on a real bundle.
