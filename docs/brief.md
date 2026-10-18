# Brief

## Problem

Kit's loop for reporting a UI problem to an agent: take a screenshot, drag or paste it into OpenCode, then dictate what's wrong. It takes three tools, and the "when" and "where" get lost: what he clicked, what moved, what he pointed at while he talked.

## Idea

One global shortcut starts a capture session:

- Records the screen (or one window), the microphone, and the cursor and clicks.
- Lets Kit draw on screen while recording (an overlay: strokes, circles, arrows). Each stroke is timestamped.
- Stops on the same shortcut.

The output isn't the video. It's a small bundle an agent can read efficiently:

- **Transcript** with word timings (local speech-to-text).
- **Events:** clicks, keypresses (no secrets), strokes, and window or app changes, all on one clock.
- **Keyframes:** a few frames picked where something happened (a stroke, a click, a pause in speech, a big visual change), with the strokes and cursor drawn on. Laid out as a contact sheet with timestamps and the words spoken around each frame.
- **Manifest** (`scrawl.json`) tying it together, plus an optional low-res video for when the agent wants more.

Handoff: copy to the clipboard as images plus text, or send straight to an OpenCode session.

## Prior art to study first

- **opencode-ios session-review tool:** `~/code/open-source/opencode-ios/tools/session-review/`. It has an annotation feature that records video, tracks the mouse, and aligns drawings with a recorded transcript. Reuse its ideas and code where possible. Ask Kit which session built it if it isn't obvious.
- **Hex:** Kit's local voice-to-text. There's a published SDK, `@kitlangton/hex` (0.3.0); see `~/code/open-source/hex-sdk-lab` for usage, and `~/code/open-source/hex-worktrees` for the source. Prefer Hex for transcription, as an SDK, binary or shared core. If Scrawl needs things Hex lacks (word timings, streaming, file input), that's a reason to improve Hex rather than add another model. A Hex-native "Scrawl mode" is also an option to weigh. (Superseded October 2026: Scrawl transcribes with Apple's SpeechAnalyzer only; the Hex fallback was removed.)

## Open questions

- Is it a standalone menu-bar app, a Hex feature, or both (a shared capture core)?
- Which native stack: Swift with ScreenCaptureKit plus an overlay window, or something else? Recording and overlays on macOS point to Swift.
- How should keyframes be selected? What is the smallest bundle that still tells the story (target: 3–6 frames)?
- How does the bundle reach OpenCode: attachments in the prompt, a file path, or a plugin or tool the agent calls to "look closer" at a time range?
- Privacy: everything stays local, with no upload by default.

## Constraints

- macOS first. Local only.
- Minimal UI: no copy beyond the labels controls need. Native Liquid Glass where there's chrome.
- Follow the `mac-capacity` skill for builds.
