# Fork notes

A fork of [kitlangton/scrawl](https://github.com/kitlangton/scrawl) that installs **beside** the
official app instead of over it, so both can run at once and upstream stays easy to pull.

`origin` is this fork (`Avery2/scrawl`), `upstream` is Kit's.

## Run it

```sh
./install-fork.sh          # build, install ~/Applications/Scrawl Fork.app, sign, launch
SCRAWL_NO_LAUNCH=1 ./install-fork.sh   # build and sign only
```

Then grant what the welcome tour asks for (Screen Recording, Microphone, Accessibility) and press
**⌥⇧D** to record. **⌥⇧F** pastes the last capture. Captures land in
`~/Library/Application Support/scrawl-fork/captures/`; the CLI is `~/bin/scrawl-fork`
(`scrawl-fork inspect latest`, `scrawl-fork frames latest --from 0:02`, …).

Toolchain: the crate is edition 2024, so an up-to-date Rust is required — this checkout is pinned
with `rustup override set 1.99.1` (upstream needs ≥ 1.85). `build.rs` compiles `helper/` with
`xcrun swiftc`, so a full Xcode (not just the CLT) has to be selected. `bun` is only for `site/`.

## What the fork changes

Everything here is either outside the Rust sources or a small, commented patch, so
`git fetch upstream && git rebase upstream/main` stays cheap.

| Where | Change | Why |
|---|---|---|
| `install-fork.sh` | own app bundle `Scrawl Fork.app`, own `CFBundleIdentifier` (`dev.averychan.scrawl.fork`), signed with your Apple Development identity | the installed `Scrawl.app` is never touched; a stable signing identity is what keeps Screen Recording / Microphone / Accessibility granted across rebuilds (ad-hoc `-` changes the cdhash every build, which macOS reads as a new app) |
| `install-fork.sh` | no `Sparkle.framework` | a fork can't be signed for Kit's appcast, and it shouldn't update itself over upstream's releases |
| `src/command_line.rs` | link name from `SCRAWL_CLI_NAME` (fork: `scrawl-fork`) | otherwise Settings › Agents in the fork re-points the machine's `scrawl` at the fork binary, which then runs with the *installed* app's config and captures |
| `src/phone.rs`, `src/web.rs` | ingest port from `SCRAWL_INGEST_PORT` (fork: 47778) | the official app owns 47777; a second listener fails with only a log line, and the phone's uploads would go to the other app |
| `src/handoff.rs` (`state_dir`) + `src/inbox.rs`, `src/ui.rs`, `src/settings.rs` | `inbox.json`, `last-mode` and the 0.3.x config migration follow the captures folder actually in use | they used to sit beside the *built-in* folder, so the fork and the installed app shared (and clobbered) each other's iPhone inbox and picker mode |
| `src/settings.rs` (`CapturesDirView`) | Storage shows `default_captures_dir()`, the folder captures actually land in | it showed `"capturesDir"` or the built-in folder while sizing the effective one |

The fork's own config is seeded once, on first install, at `~/.config/scrawl-fork/config.json`:
`"capturesDir"`, record **⌥⇧D** and paste **⌥⇧F** (so the two apps' Carbon hotkeys don't collide —
a losing `RegisterEventHotKey` is only a log line). Delete that file to get the defaults back, but
then set ⌥⇧S/⌥⇧V to something else in Settings unless the installed app is quit.

Shared on purpose: the log (`~/Library/Logs/Scrawl/scrawl.log`), and nothing else that matters.
Still Kit's on the fork: the helper's embedded `helper/Info.plist` bundle id (so a camera prompt
for the iPhone mirror says "Scrawl Helper"), and the agent skill Settings installs.

For iPhone / ScrawlKit captures the phone's destination has to name port **47778** for the fork,
and flip to 47777 for the installed app; the token is per-config, so both are independent.

## Scrawl vs Hex: stacks and shared ground

| | `Avery2/Hex` (Swift fork) | `anomalyco/hex` 2.x | Scrawl |
|---|---|---|---|
| UI | SwiftUI + TCA | Rust + GPUI | Rust + GPUI + AppKit/Metal pill |
| Audio / speech | FluidAudio (Parakeet), Apple | `transcribe-cpp`, local models | Apple `SpeechAnalyzer` via a Swift helper, or Hex 2.2, or a command |
| Build | Xcode project | Cargo | Cargo, and `build.rs` runs `swiftc` on `helper/` |
| Site | — | `site/` | `site/` (Bun + React) |
| Platform | macOS | macOS, Linux beta, iOS SDK | macOS, Windows preview, `Sources/ScrawlKit` (iOS) |

No shared code, but the same lineage: GPUI 0.2.2, `objc2`, `screencapturekit 11`, `cpal`, `hound`
are pinned to the same versions in Scrawl and Hex 2, so ideas move between them more easily than
between Scrawl and the Swift Hex.

**The Swift Hex fork cannot be Scrawl's transcriber.** `src/hex.rs` talks to Hex's desktop local
API v2 (`GET /health`, `/models`, `POST /transcriptions`, discovered through
`~/Library/Application Support/hex/local-api.json`); the Swift app has no such server and keeps its
data under `com.kitlangton.Hex`. Settings › Recording › Transcription › Hex therefore means Hex 2.2+,
not this fork.

## Porting the Hex-fork improvements

Already solved in Scrawl, by a stronger mechanism — don't port:

- *Recover stranded recordings*: Scrawl journals each capture (`unfinished.json`) and rebuilds it
  from a child process on the next launch (`src/recover.rs`), plus `ORPHAN_SETTLE` for video+audio
  pairs with no journal, and retention never deletes a bundle it still owes a build.
- *Fewer dropped words at stop*: the screen session and the audio stop together in one
  `thread::scope` (`src/ui.rs`), so audio keeps running while the video finalizes.
- *Acknowledge the stop / cancelled session*: the Completed pill says Copied / Sent / couldn't
  reach, and Escape is handled explicitly.

Cheap, real gaps worth doing first (all in `src/ui.rs` unless noted):

1. A failed bundle build shows the pill as **"Copied"** with an empty clipboard.
2. A failed `audio.wav` write is discarded (`let _ = std::fs::write` in `src/main.rs`), so a full
   disk silently produces a bundle with no narration.
3. A dead microphone still looks like a recording: `AudioRecorder::start().ok()` and no level or
   state on the pill.
4. Escape `remove_dir_all`s the bundle (`discard_recording`); Hex's lesson is that cancelling is
   about the paste, not keeping the words.
5. ⌘Q during a recording quits mid-capture and keeps nothing.

Real work, in value order: durable audio written while recording (Hex's `DurableAudioWriter`
equivalent) so a crash keeps the narration; the live transcript on the pill (Hex shows the newest
words while you speak — Scrawl's `liveTranscribe` experiment only speeds up the final transcript
and publishes nothing); and honest failure states during a capture.
