# Scrawl — Agent Instructions

Scrawl is a macOS shortcut-triggered (`⌃⌥⇧S`) screen, voice, cursor, and freehand drawing capture tool built in Rust + GPUI that compiles recordings into compact `.scrawl` bundles (`contact-sheet.png`, `frames/frame-XX.png`, `summary.md`, `scrawl.json`, `recording.mp4`, and `audio.wav`) in `~/Library/Application Support/scrawl/captures/`.

## Continuous Output Review & Proactive Recommendations

- **Always inspect real capture artifacts**: Whenever working on Scrawl or after Kit tests a capture, inspect the latest bundles in `~/Library/Application Support/scrawl/captures/` (both `summary.md` / `scrawl.json` and visual `contact-sheet.png` / `frames/*.png` via `read`).
- **Proactively recommend improvements**: Continuously look for and surface concrete improvements whenever you spot issues in the output or UX—such as redundant or noisy text in `summary.md`, cluttered or low-density `contact-sheet.png` layouts, unnecessary keyframes (e.g. static frames cut on speech pauses), ghost strokes or misaligned annotations, slow startup or post-processing latency, or clipboard/paste friction.

## Design & Architecture Principles

- **Division of labor between Images and Text**:
  - **Images (`frames/*.png` and `contact-sheet.png`)**: Keep images purely visual. Each keyframe shows the screenshot, active red tldraw stroke(s) for that moment, click ripple/cursor, and a compact `#N · M:SS` corner badge. `contact-sheet.png` is a tight grid of those labeled frames with no rasterized transcript headers or caption boxes.
  - **Text (`summary.md` & Clipboard)**: Put the transcript, `[#N · M:SS]` keyframe index, and absolute file paths (`Contact sheet:`, `frames/frame-XX.png`, `Video:`, `Bundle:`) in plain text (`NSPasteboardTypeString`) without repeating the transcript multiple times, so `⌘V` pastes cleanly into any terminal (Ghostty / OpenCode TUI) or GUI composer (OpenCode Desktop).
- **Keyframe selection (`src/keyframes.rs`)**:
  - Only cut keyframes on real visual or input events (`Mark` strokes drawn while holding `⇧`, `Click`/`Drag` inside the capture target, `Laser` points, or settled `SceneChange` transitions)—never cut extra keyframes on speech pauses alone over a static screen.
- **Fast builds & local workflow**:
  - Keep `Cargo.toml` and `Cargo.lock` stable and build/test with `taskpolicy -c utility cargo build -j 3 --offline` and `taskpolicy -c utility cargo test -j 3 --offline -- --test-threads=2`.
  - Keep everything local on `main` unless asked otherwise.
  - **Always install and launch via `./scripts/install-app.sh` (`~/Applications/Scrawl.app`)**: Never run `target/debug/scrawl` directly as the background daemon because `hivemind clean` purges `target/` directories, which unlinks the binary on disk and causes macOS `tccd` to revoke Screen Recording and Microphone access. `./scripts/install-app.sh` copies the binary into `~/Applications/Scrawl.app`, signs it with Kit's Developer ID (`dev.kitlangton.scrawl`) so TCC permissions persist across rebuilds, and restarts the app.
