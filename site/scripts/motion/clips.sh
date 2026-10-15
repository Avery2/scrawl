#!/bin/sh
# Assemble synced before/after clips from two capture.ts frame folders taken over the same range.
#
#   scripts/motion/clips.sh <before-dir> <after-dir> <out-prefix> [scale-width]
#
# Writes <out>.mp4 (side by side, 60 fps, h264 High yuv420p +faststart), <out>-strip.png (12 frames
# of each side, before on top) and checks the strip for blank tiles.
set -eu
before=$1
after=$2
out=$3
width=${4:-960}
font=/System/Library/Fonts/Supplemental/Arial.ttf
label() { printf "drawtext=fontfile=%s:text='%s':x=16:y=14:fontsize=22:fontcolor=white:box=1:boxcolor=black@0.55:boxborderw=8" "$font" "$1"; }

ffmpeg -v error -y -framerate 60 -i "$before/frames/f%05d.png" -framerate 60 -i "$after/frames/f%05d.png" \
  -filter_complex "[0]scale=$width:-2,$(label 'Before')[a];[1]scale=$width:-2,$(label 'After')[b];[a][b]hstack=2,pad=ceil(iw/2)*2:ceil(ih/2)*2" \
  -r 60 -c:v libx264 -profile:v high -pix_fmt yuv420p -crf 18 -movflags +faststart -an "$out.mp4"

n=$(ls "$after/frames" | wc -l | tr -d ' ')
step=$(( n / 12 ))
[ "$step" -lt 1 ] && step=1
tile() {
  ffmpeg -v error -y -framerate 60 -i "$1/frames/f%05d.png" -vf "select='not(mod(n\,$step))',scale=$(( width / 2 )):-2,tile=6x2" -frames:v 1 "$2"
}
tile "$before" "$out-strip-before.png"
tile "$after" "$out-strip-after.png"
ffmpeg -v error -y -i "$out-strip-before.png" -i "$out-strip-after.png" -filter_complex vstack=2 "$out-strip.png"
rm "$out-strip-before.png" "$out-strip-after.png"

uv run --quiet --with pillow python3 - "$out-strip.png" <<'EOF'
import sys
from PIL import Image, ImageStat
im = Image.open(sys.argv[1]).convert("L"); w, h = im.size
tiles = [im.crop((c*w//6, r*h//4, (c+1)*w//6, (r+1)*h//4)) for r in range(4) for c in range(6)]
blank = [i for i, t in enumerate(tiles) if ImageStat.Stat(t).stddev[0] < 3]
print(f"{sys.argv[1]}: blank tiles:", blank or "none")
EOF
echo "wrote $out.mp4"
