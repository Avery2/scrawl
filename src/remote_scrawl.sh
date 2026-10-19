#!/bin/sh
# scrawl: installed by Scrawl on your Mac
# (Settings > Remote Machines > Allow zoom).
#
# Captures copied here from the Mac are in ~/.scrawl/captures.
# The video stays on the Mac, so this asks Scrawl there for a
# close look at one moment of a capture:
#
#   scrawl zoom <capture or latest> --at <M:SS or "words said">
#               [--span 2] [--frames 12] [--full]
#
# The request goes over ~/.scrawl/scrawl.sock, which your SSH
# connection from the Mac forwards to Scrawl (the RemoteForward
# line Scrawl added to the Mac's ~/.ssh/config), so it works only
# while that connection is open. It sends the key in
# ~/.scrawl/token. The image Scrawl sends back is saved in the
# capture's zoom/ folder, and the script prints its path and what
# was said.
#
# To remove it, turn off Allow zoom in Scrawl's settings, or
# delete ~/.scrawl/bin/scrawl and ~/.scrawl/token.
set -eu

home=$HOME/.scrawl
sock=$home/scrawl.sock

usage() {
  echo 'usage: scrawl zoom <capture|latest> --at <M:SS or "words said">' \
    '[--span SECONDS] [--frames N] [--full]' >&2
  exit 2
}

[ "${1:-}" = zoom ] || usage
shift
capture=latest at= span=2 frames=12 full=false
while [ $# -gt 0 ]; do
  case $1 in
    --at) [ $# -ge 2 ] || usage; at=$2; shift 2 ;;
    --at=*) at=${1#--at=}; shift ;;
    --span) [ $# -ge 2 ] || usage; span=$2; shift 2 ;;
    --span=*) span=${1#--span=}; shift ;;
    --frames) [ $# -ge 2 ] || usage; frames=$2; shift 2 ;;
    --frames=*) frames=${1#--frames=}; shift ;;
    --full) full=true; shift ;;
    -*) usage ;;
    *) capture=$1; shift ;;
  esac
done
[ -n "$at" ] || usage

# A name or any path into the capture works.
case $capture in */summary.md | */scrawl.json | */contact-sheet.png) capture=$(dirname "$capture") ;; esac
name=$(basename "$capture")
if [ "$name" = latest ]; then
  name=$(ls -t "$home/captures" 2>/dev/null | grep '\.scrawl$' | head -n 1) || true
  [ -n "$name" ] || { echo "scrawl: there are no captures in $home/captures yet." >&2; exit 1; }
fi
case $name in *.scrawl) ;; *) name=$name.scrawl ;; esac

if [ ! -S "$sock" ]; then
  echo "scrawl: not connected to your Mac. Zoom works while you're" \
    "connected over SSH from the Mac that runs Scrawl ($sock is missing)." >&2
  exit 1
fi
if [ ! -r "$home/token" ]; then
  echo "scrawl: $home/token is missing. Turn Allow zoom off and on again in Scrawl's settings." >&2
  exit 1
fi

out=$home/captures/$name/zoom
mkdir -p "$out"
body=$(mktemp "$out/.zoom.XXXXXX")
head=$(mktemp "$out/.head.XXXXXX")
trap 'rm -f "$body" "$head"' EXIT

set +e
code=$(curl -s --unix-socket "$sock" --max-time 180 -o "$body" -D "$head" -w '%{http_code}' \
  -H @"$home/token" \
  --data-urlencode "capture=$name" --data-urlencode "at=$at" \
  --data-urlencode "span=$span" --data-urlencode "frames=$frames" --data-urlencode "full=$full" \
  http://scrawl/zoom)
status=$?
set -e
case $status in
  0) ;;
  7)
    # Nothing listens: an earlier connection left the socket
    # behind, so this connection couldn't put its own there.
    rm -f "$sock"
    echo "scrawl: $sock was left over from an earlier SSH connection, so I removed it. Reconnect over SSH and try again." >&2
    exit 1 ;;
  52 | 56)
    echo "scrawl: the SSH connection is up, but Scrawl didn't answer. Is Scrawl running on the Mac?" >&2
    exit 1 ;;
  *)
    echo "scrawl: the request to your Mac failed (curl exit $status)." >&2
    exit 1 ;;
esac

if [ "$code" != 200 ]; then
  echo "scrawl: $(cat "$body")" >&2
  exit 1
fi

field() { grep -i "^$1:" "$head" | tail -n 1 | sed 's/^[^:]*: *//' | tr -d '\r'; }
file=$(field X-Scrawl-File)
case $file in '' | */* | .*) echo "scrawl: unexpected reply from the Mac." >&2; exit 1 ;; esac
mv "$body" "$out/$file"
echo "$out/$file"
cells=$(field X-Scrawl-Cells)
[ -z "$cells" ] || echo "$cells"
said=$(field X-Scrawl-Said)
[ -z "$said" ] || echo "Said: “$said”"
