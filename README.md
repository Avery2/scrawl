# Scrawl

Show an agent a problem instead of describing it.

Press a shortcut (`⌃⌥⌘S`), then talk, click around, and draw on the screen. Scrawl records the screen, your voice, and your marks, then packs them into a compact bundle: a contact sheet of annotated keyframes (`contact-sheet.png`), individual full-resolution keyframes (`frames/frame-XX.png`), a time-aligned transcript and interaction timeline (`summary.md`), and a structured manifest (`scrawl.json`).

## Usage

```sh
# Launch the GPUI menu-bar and review app (global shortcut: ⌃⌥⌘S)
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
```

## Capture Controls

- `⌃⌥⌘S`: Start or finish a capture session from any app
- **Normal pointer**: Clicks and scrolls pass straight through to the app underneath and are logged with the target element's Accessibility label
- **Hold `⇧` (Shift)**: Point with a fading 1-second red laser trail without intercepting clicks
- **Press `D`**: Toggle drawing mode on the overlay canvas (`perfect-freehand` ink with white halo, classifying circles, underlines, lines, and arrows)
- **`⇧`-click (in draw mode)**: Drop a numbered pin (`1`, `2`, …) on the control underneath
- **`⌘Z`**: Undo the last stroke or pin
- **`Esc`**: Exit drawing mode, or cancel the active capture
