# Linux

Read this before changing `src/capture/linux.rs`, `src/ui/linux.rs`, `src/linux/`, `src/ink_view.rs`, `src/speech.rs`, or anything behind a `platform::` call in `src/capture.rs` and `src/ui.rs`. `docs/windows.md` explains the shared shape (one GPUI UI, per-OS shells); Linux is the third shell beside macOS and Windows.

## X11 everywhere, Wayland around it

GPUI is built with its `x11` feature only, and `linux::prepare_process` (the first thing `main` does) hides `WAYLAND_DISPLAY` from it, so on Wayland every Scrawl window is an XWayland window (`SCRAWL_NATIVE_WAYLAND=1` opts out, for experiments). That keeps one window model on both: Scrawl's floating windows (pill, picker, ink overlay) are override-redirect X windows it places itself in global pixels (`linux::x11::float`, `place`), with an empty input shape to let clicks through (`set_click_through`). `float` waits for the window manager to finish mapping and then unmanaging the window before it takes it over; otherwise GNOME on Wayland keeps drawing it at its old frame's offset.

GPUI 0.2.2's X11 window panics on `HasWindowHandle`, so `x11::window_id` finds a GPUI window's X id itself (the newest `_NET_WM_PID` window it hasn't seen, cached per `WindowId`).

Global coordinates are root-window pixels; X11 has one scale for the screen (`x11::ui_scale`), so `MAX_CAPTURE_SCALE` is 1 as on Windows.

## What each session can do

| | X11 | GNOME on Wayland | KDE Plasma on Wayland (written, not yet tested) |
|---|---|---|---|
| Shortcuts | `XGrabKey` | GNOME custom keybindings running `scrawl shortcut record\|paste` (GNOME 48+: the GlobalShortcuts portal) | GlobalShortcuts portal |
| Recording a display or area | `ximagesrc` | `org.gnome.Mutter.ScreenCast.RecordArea`, no dialog | ScreenCast portal, asks once (restore token in `~/.local/share/scrawl/portal.json`) |
| Recording a window | `ximagesrc xid=` | X11 apps by id; other windows through the portal's picker | the portal's picker |
| Clicks, keys, ⇧ ink | the X server (pointer and key state, polled) | the input devices (`/dev/input`, needs the `input` group) | same |
| Pointer position | the X server | XWayland while it's over X11 windows; an absolute pointing device (tablet, touchscreen, VM or remote desktop) from the input devices; the overlay takes the pointer while ⇧ is held | same |
| App in front | `_NET_ACTIVE_WINDOW` | AT-SPI (`linux::atspi`; `mutter-x11-frames` means an X11 app, asked of XWayland) | AT-SPI |
| Paste | XTEST `Ctrl+V` | RemoteDesktop portal, asks once | same |

### Clicks and ink on Wayland

Wayland tells no app where the pointer is over another app's window. Scrawl reads buttons and keys from the input devices (membership of the `input` group: `sudo usermod -aG input $USER`, then log in again; Settings › Permissions shows it as **Input devices**, the Accessibility row on macOS). While ⇧ is held, the ink overlay takes the pointer (`linux::overlay::take_pointer`), so XWayland reports where it is and the ink follows it; a click while ⇧ is held lands on the overlay, not the app.

A click's position is right over X11 apps and with an absolute pointing device. With a mouse or touchpad over a Wayland app, XWayland still has the pointer where it last saw it, so the click's position is stale: the keyframe is cut at the right moment, but its ripple can be in the wrong place. A GNOME Shell extension that reports the pointer would close this gap (see the PR's open questions).

Without the `input` group, a capture on Wayland has no clicks, keys or ⇧ ink outside Scrawl's own windows, and records only the screen and voice.

## Scrawl's own windows are in the recording

Neither X11 nor GNOME's or KDE's screencasts let an app leave its windows out, so the pill and the ink are in `recording.mp4` and the keyframes. The ink is drawn again at the same place on the keyframes, so it doesn't double. The candidate frames used to pick keyframes paint over the pill (`linux::media::own_window_masks`), so its ticking clock never counts as activity.

## Movies, transcription, the rest

- **Movies:** GStreamer (`linux::media`): the source above, `videorate` to 30 fps, the first encoder installed of `x264enc`, `openh264enc` and VA-API's, `mp4mux`. Candidate frames come off a `tee` through an `appsink`; it holds end-of-stream back until its last frame is taken, so `Recorder::finish` drains it. Compositors send frames only when the screen changes, so the last frame is reused until a new one comes. Without an encoder Scrawl records no movie and keeps sharper candidates instead.
- **Transcription:** Parakeet through `transcribe.cpp` (`speech.rs`, shared with Windows). Settings › Recording downloads the model (731 MB, checked against its SHA-256, into `~/.local/share/scrawl/models`); a model Hex installed is used too. A capture made without one says so in `summary.md` and is marked `.transcript-pending`; when the download finishes, `speech_model::transcribe_pending` adds the transcripts.
- **Tray:** a StatusNotifierItem (`ksni`); left click starts a recording, the menu is the macOS menu bar menu. GNOME shows it with the AppIndicator extension (on by default on Ubuntu). Without a tray, Scrawl says so in a notification, and launching it again opens Settings (`linux::ipc`, a socket at `$XDG_RUNTIME_DIR/scrawl.sock`).
- **Clipboard:** text and PNG through the X clipboard (`linux::clipboard`), which XWayland shares with Wayland apps.
- **Launch at login:** `~/.config/autostart/scrawl.desktop`. **Trash:** the freedesktop trash. **Captures:** `~/.local/share/scrawl/captures`; **log:** `~/.local/share/scrawl/Logs/scrawl.log`.
- **Command-line tool:** Settings › Agents links `~/.local/bin/scrawl` to the AppImage (`$APPIMAGE`).

## Not on Linux

The cabled iPhone, video export, `agentContext` lookups (`ax_other.rs`), Sparkle updates (**Check for Updates…** opens the releases page) and the dev-site tab URL (`/sdk.js` registrations still route).

## Packaging

`scripts/linux/appimage.sh [binary]` builds `target/appimage/Scrawl-<arch>.AppImage` with linuxdeploy and its GStreamer plugin, so GStreamer, `pipewiresrc`, `ximagesrc` and the H.264 encoders come with it. CI builds one on every pull request (the `scrawl-linux-x86_64` artifact). Ubuntu 22.04+ needs `libfuse2` to run AppImages (`--appimage-extract-and-run` works without it). Flatpak is not done yet.

## Building and testing

Build dependencies (Ubuntu/Debian; CI installs the same):

```sh
sudo apt install build-essential pkg-config cmake ninja-build clang \
  libxkbcommon-dev libxkbcommon-x11-dev libvulkan-dev libx11-dev libx11-xcb-dev \
  libxcb1-dev libxcb-shm0-dev libxcb-xfixes0-dev libxcb-shape0-dev libxcb-randr0-dev \
  libxcb-composite0-dev libxcb-xinput-dev libasound2-dev libfontconfig-dev libfreetype-dev \
  libssl-dev libdbus-1-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-pipewire
cargo build && ./target/debug/scrawl app
```

GPUI needs Vulkan (`mesa-vulkan-drivers`; llvmpipe works in a VM). Tests: `cargo test`. A desktop test runs in a Tart Ubuntu VM on the Mac mini (`macvm`, see `~/code/home/tools/macvm`): GNOME on Wayland by default, X11 with `WaylandEnable=false` in `/etc/gdm3/custom.conf`. Never test on Kit's desktop session.
