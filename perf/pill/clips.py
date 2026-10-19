# /// script
# dependencies = ["numpy", "pillow"]
# ///
"""Before/after clips from two `perf/pill/record.sh` recordings of the same lab scene.

    uv run perf/pill/clips.py BEFORE.mov AFTER.mov OUT_PREFIX [--slow 4] [--strip-at SECONDS]

Both recordings are aligned on the capsule's first appearance over the plain backdrop, made
constant 60 fps, labelled and stacked (before on top): OUT_PREFIX.mp4, plus OUT_PREFIX-slow.mp4
(--slow N: N times slower, frames held) and OUT_PREFIX-strip.png (12 frames every 2 display
frames from --strip-at seconds after the capsule appears, before row over after row). Writes
phone-safe H.264 (yuv420p, faststart).
"""

import argparse
import subprocess
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

W, H = 1280, 152  # the backdrop region (640 x 76 pt) at 2x


def frames(path: Path, w=160, h=19):
    raw = subprocess.run(
        ["ffmpeg", "-v", "error", "-i", str(path), "-vf", f"fps=60,scale={w}:{h},format=gray", "-f", "rawvideo", "-"],
        check=True,
        capture_output=True,
    ).stdout
    return np.frombuffer(raw, np.uint8).reshape(-1, h, w).astype(np.int16)


def appear(path: Path) -> float:
    """Seconds until the capsule first shows: a frame that differs from the first."""
    f = frames(path)
    ref = f[min(2, len(f) - 1), 3:15, 50:110]
    diff = np.abs(f[:, 3:15, 50:110] - ref).mean(axis=(1, 2))
    hits = np.nonzero(diff > 2.5)[0]
    if not len(hits):
        raise SystemExit(f"{path}: the capsule never appears")
    return hits[0] / 60.0


def label(text: str, path: Path):
    im = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    font = ImageFont.truetype("/System/Library/Fonts/SFNS.ttf", 26)
    d.rounded_rectangle((16, 14, 16 + 20 + d.textlength(text, font=font), 54), 10, fill=(20, 20, 22, 220))
    d.text((26, 18), text, font=font, fill=(255, 255, 255, 255))
    im.save(path)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("before", type=Path)
    ap.add_argument("after", type=Path)
    ap.add_argument("out")
    ap.add_argument("--slow", type=float, default=0)
    ap.add_argument("--lead", type=float, default=0.3, help="seconds kept before the capsule appears")
    ap.add_argument("--length", type=float, default=0, help="seconds kept (default: the shorter recording)")
    ap.add_argument("--strip-at", type=float, default=None)
    ap.add_argument("--caption", default="")
    a = ap.parse_args()

    starts = [max(0.0, appear(p) - a.lead) for p in (a.before, a.after)]
    durations = [
        float(subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(p)], capture_output=True, text=True).stdout)
        - s
        for p, s in zip((a.before, a.after), starts)
    ]
    length = a.length or min(durations)
    tmp = Path(tempfile.mkdtemp())
    label(f"Before{a.caption}", tmp / "before.png")
    label(f"After{a.caption}", tmp / "after.png")
    enc = ["-c:v", "libx264", "-profile:v", "high", "-pix_fmt", "yuv420p", "-movflags", "+faststart", "-an", "-crf", "16"]
    side = lambda i, s: f"[{i}:v]trim=start={s:.4f}:duration={length:.4f},setpts=PTS-STARTPTS,fps=60,scale={W}:{H}[v{i}];"
    graph = (
        side(0, starts[0])
        + side(1, starts[1])
        + "[v0][2:v]overlay=0:0[l0];[v1][3:v]overlay=0:0[l1];"
        + f"[l0]pad={W}:{H + 8}:0:0:color=0x111111[p0];[p0][l1]vstack[out]"
    )
    out = Path(a.out + ".mp4")
    subprocess.run(
        ["ffmpeg", "-v", "error", "-y", "-i", str(a.before), "-i", str(a.after), "-i", str(tmp / "before.png"), "-i", str(tmp / "after.png"),
         "-filter_complex", graph, "-map", "[out]", "-r", "60", *enc, str(out)],
        check=True,
    )
    print(out)
    if a.slow:
        slow = Path(a.out + "-slow.mp4")
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", str(out), "-vf", f"setpts={a.slow}*PTS,fps=60", *enc, str(slow)], check=True)
        print(slow)
    if a.strip_at is not None:
        t0 = a.lead + a.strip_at
        rows = []
        for i, (p, s) in enumerate(zip((a.before, a.after), starts)):
            png = tmp / f"row{i}.png"
            subprocess.run(
                ["ffmpeg", "-v", "error", "-y", "-ss", f"{s + t0:.4f}", "-i", str(p), "-vf",
                 f"fps=60,select='not(mod(n\\,2))',scale=640:-1,tile=4x3:padding=4:color=0x111111", "-frames:v", "1", str(png)],
                check=True,
            )
            rows.append(Image.open(png).convert("RGB"))
        strip = Image.new("RGB", (rows[0].width, sum(r.height for r in rows) + 16), (17, 17, 17))
        y = 0
        for r in rows:
            strip.paste(r, (0, y))
            y += r.height + 16
        path = Path(a.out + "-strip.png")
        strip.save(path)
        stats = [np.asarray(r.convert("L")).std() for r in rows]
        print(path, "row contrast", [round(float(v), 1) for v in stats])


if __name__ == "__main__":
    main()
