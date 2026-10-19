---
name: scrawl
description: Read Scrawl screen captures. Use when a message contains a Scrawl capture (a line starting `[scrawl 0:12 · …]`, a `.scrawl` bundle path, or a contact-sheet.png from one), or when the user asks you to look at what they recorded, drew, or said in Scrawl.
---

# Reading Scrawl captures

Scrawl records the user's screen, voice, clicks and red ink drawings, then packs them into a `.scrawl` bundle folder. A capture pasted into chat or sent to a session looks like this:

```text
[scrawl 0:09 · OpenCode “session — OpenCode” · /path/to/scrawl-123.scrawl]
Note: Header inset shifts on expand
- #1 (0:01) [clicked “Used 12 tools”] “When I click Used 12 tools this band”
- #2 (0:09) “header jumps down by four pixels, and the row spacing feels too tight.”
```

The first line gives the length, what was recorded, and the bundle path. Each `#N (M:SS)` line is a keyframe with the action at that moment and the words the user said while it was on screen.

## Files in a bundle

- `contact-sheet.png` shows every keyframe in time order (left to right, top to bottom), each with a `#N M:SS` badge. Look at it first.
- `frames/frame-NN.png` is keyframe `#N` at full size. Open the ones the narration points at before you act.
- `summary.md` is the transcript above. `scrawl.json` has the full timeline: strokes, clicks, keyframes, word timings, and app context.
- `recording.mp4` or `recording.mov` and `audio.wav` hold the source video and audio, when they haven't been cleaned up yet.
- A `Cropped to:` line means the keyframes are cropped to where the activity was; the full screens are in the folder it names.

Red strokes, circles and arrows are the user's marks, so treat them as pointing at the problem. Click ripples show where they clicked. The transcript is speech recognition and can mishear names and code terms.

## Looking closer

When the problem is motion (an animation, a jump, a flicker) or happens between keyframes, run:

```sh
scrawl zoom <bundle> --at <M:SS or "a phrase that was said"> [--span 2] [--frames 12]
```

It writes a dense contact sheet of that moment with each frame's time and prints what was said; open the PNG it reports. `scrawl frames <bundle> --from 0:02 --to 0:05 [--fps 4]` extracts plain frames for a time range, and `scrawl inspect <bundle> --json` prints the manifest. `latest` works in place of a bundle path.

The `scrawl` command ships inside the app. If it isn't on `PATH`, use `/Applications/Scrawl.app/Contents/MacOS/scrawl` (or `~/Applications/Scrawl.app/...`). Where there's no Scrawl app, such as a Linux agent host the bundle was copied to, read `frames/` and `scrawl.json` directly. A `.media-pending` file in the bundle means the video is still being copied over.

Don't edit or delete files in a bundle; Scrawl manages them.
