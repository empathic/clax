#!/usr/bin/env bash
# The landing page's reel (//web/reel:reel): compose.sh with the landing
# cut, the teaser, the intro line, the order, the "and also" inserts, the
# poster frame and the titles with their context lines
# (landing-titles.txt).
#
#   web/reel/compose/landing.sh OUT_DIR FRAMES_DIR...
#
# Each FRAMES_DIR is a vignette's deterministic recording (<id>_frames).
# Writes OUT_DIR/reel.mp4 and OUT_DIR/reel-poster.jpg; compose.sh's other
# outputs stay in OUT_DIR/work. FFMPEG, FFPROBE, CHROME and REEL_PYTHON
# pass through to compose.sh; REEL_BROWSERS, a PLAYWRIGHT_BROWSERS_PATH
# directory (@chromium//:browsers), sets CHROME to its Chromium.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
[ $# -ge 2 ] || { echo "usage: landing.sh OUT_DIR FRAMES_DIR..." >&2; exit 2; }
out="$1"; shift
if [ -n "${REEL_BROWSERS:-}" ]; then
  CHROME="$(find "$REEL_BROWSERS"/chromium-[0-9]* -type f \
    \( -name 'Google Chrome for Testing' -o -name chrome \) -perm -u+x | sed -n 1p)"
  [ -n "$CHROME" ] || { echo "landing.sh: no Chromium in $REEL_BROWSERS" >&2; exit 1; }
  export CHROME
fi

"$here/compose.sh" -o "$out/work" \
  --poster globe@touch+1.1 \
  --teaser globe@touch+1.1,dash@reveal+0.2,agent@reveal+0.1,game@touch-0.4 \
  --intro "Ask. It builds. You shape it together." \
  --order coldopen,agent,dash,globe,game,realtime,history,claxbot,share \
  --inserts realtime,history,claxbot,share@share+2.7 \
  --titles "$here/landing-titles.txt" \
  "$@"
cp "$out/work/clax-reel.mp4" "$out/reel.mp4"
cp "$out/work/reel-poster.jpg" "$out/reel-poster.jpg"
