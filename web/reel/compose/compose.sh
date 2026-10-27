#!/usr/bin/env bash
# Interim reel composer (ffmpeg + headless Chrome). A stepping stone until the
# fframes composer (docs/superpowers/specs/2026-10-27-sizzle-reel-design.md
# section 9) lands; run by hand, outputs under target/reel/ (git-ignored).
#
#   web/reel/compose/compose.sh -o target/reel/out FRAMES_DIR [FRAMES_DIR...]
#
# Each FRAMES_DIR holds frames/NNNNNN.png and beats.json (30 fps, 1600x1000),
# given in reel.toml order; its basename minus a "_frames" suffix is the
# vignette ID whose title comes from web/reel/reel.toml.
# Outputs in OUT: cards/*.png, clips/<n>-<id>.mp4, clax-reel.mp4, clax-reel.gif.
# Environment: FFMPEG, CHROME, GIF_SECONDS (2.5), GIF_LEAD (1.5 s before the
# last beat), GIF_WIDTH (720), GIF_FPS (15).
set -euo pipefail

FFMPEG="${FFMPEG:-$HOME/.local/homebrew/bin/ffmpeg}"
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
GIF_SECONDS="${GIF_SECONDS:-2.5}"
GIF_LEAD="${GIF_LEAD:-1.5}"
GIF_WIDTH="${GIF_WIDTH:-720}"
GIF_FPS="${GIF_FPS:-15}"
CARD_SECONDS=1.5
XFADE=0.3
FPS=30

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
toml="$here/../reel.toml"
out=""
dirs=()
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2 ;;
    -h|--help) sed -n 2,16p "$0"; exit 0 ;;
    *) dirs+=("$1"); shift ;;
  esac
done
[ -n "$out" ] && [ ${#dirs[@]} -gt 0 ] || { echo "usage: compose.sh -o OUT_DIR FRAMES_DIR..." >&2; exit 2; }
mkdir -p "$out/cards" "$out/clips"
out="$(cd "$out" && pwd)"

log() { echo "compose: $*" >&2; }
ff() { "$FFMPEG" -hide_banner -loglevel error -y "$@"; }

# toml_get title ID | headline | footer: values from reel.toml (a flat subset).
toml_get() {
  if [ "$1" = title ]; then
    awk -v id="$2" '
      /^\[\[vignette\]\]/ { cur = "" }
      /^id *=/ { split($0, a, "\""); cur = a[2] }
      /^title *=/ && cur == id { split($0, a, "\""); print a[2]; found = 1 }
      END { if (!found) print id }' "$toml"
  else
    awk -v key="$1" '/^\[/ { sec = $0 } sec == "[end]" && $1 == key { split($0, a, "\""); print a[2] }' "$toml"
  fi
}

urlenc() { python3 -I -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1]))' "$1"; }

card() { # card OUT.png QUERY
  "$CHROME" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 \
    --window-size=1600,1000 --virtual-time-budget=2000 \
    --screenshot="$1" "file://$here/card.html?$2" >/dev/null 2>&1
  [ -s "$1" ] || { echo "compose: Chrome made no card for $2" >&2; exit 1; }
}

ids=(); counts=(); lasts=()
n=0
for d in "${dirs[@]}"; do
  d="$(cd "$d" && pwd)"
  [ -d "$d/frames" ] && [ -f "$d/beats.json" ] || { echo "compose: $d lacks frames/ or beats.json" >&2; exit 1; }
  b="$(basename "$d")"; id="${b%_frames}"
  n=$((n + 1))
  tag="$(printf '%02d-%s' "$n" "$id")"
  ids+=("$tag"); dirs[$((n - 1))]="$d"
  counts+=("$(ls "$d/frames" | grep -c '\.png$')")
  lasts+=("$(python3 -I -c 'import json,sys;b=json.load(open(sys.argv[1]));print(max([x["frame"] for x in b["beats"]] or [b["frames"]//2]))' "$d/beats.json")")
  card "$out/cards/$tag.png" "title=$(urlenc "$(toml_get title "$id")")"
done
card "$out/cards/end.png" "end=1&title=$(urlenc "$(toml_get headline)")&footer=$(urlenc "$(toml_get footer)")"
log "cards done"

# chain OUT.mp4 SPEC...: SPEC is "card:PNG:SECS" or "seq:DIR:COUNT"; pieces are
# joined with XFADE-second crossfades and encoded silent H.264.
chain() {
  local dest="$1"; shift
  local inputs=() filt="" i=0 total=0 prev="" label dur
  for spec in "$@"; do
    IFS=: read -r kind path val <<<"$spec"
    if [ "$kind" = card ]; then
      inputs+=(-loop 1 -framerate $FPS -t "$val" -i "$path"); dur="$val"
    else
      inputs+=(-framerate $FPS -i "$path/frames/%06d.png"); dur="$(python3 -I -c "print($val/$FPS)")"
    fi
    filt+="[$i:v]format=yuv420p,setsar=1,fps=$FPS,settb=1/$FPS[v$i];"
    if [ $i -eq 0 ]; then
      prev="v0"; total="$dur"
    else
      local off; off="$(python3 -I -c "print(round($total-$XFADE,4))")"
      label="x$i"
      filt+="[$prev][v$i]xfade=transition=fade:duration=$XFADE:offset=$off[$label];"
      prev="$label"; total="$(python3 -I -c "print(round($total+$dur-$XFADE,4))")"
    fi
    i=$((i + 1))
  done
  if [ $i -eq 1 ]; then filt+="[v0]null[outv]"; else filt="${filt%;}"; filt="${filt%\[$prev\]}[outv]"; fi
  ff "${inputs[@]}" -filter_complex "$filt" -map "[outv]" -r $FPS \
    -c:v libx264 -pix_fmt yuv420p -crf 20 -preset medium -movflags +faststart -an "$dest"
}

for ((k = 0; k < n; k++)); do
  chain "$out/clips/${ids[k]}.mp4" "card:$out/cards/${ids[k]}.png:$CARD_SECONDS" "seq:${dirs[k]}:${counts[k]}"
done
log "clips done"

specs=()
for ((k = 0; k < n; k++)); do
  specs+=("card:$out/cards/${ids[k]}.png:$CARD_SECONDS" "seq:${dirs[k]}:${counts[k]}")
done
specs+=("card:$out/cards/end.png:4")
chain "$out/clax-reel.mp4" "${specs[@]}"
log "mp4 done"

# GIF: a window of GIF_SECONDS ending GIF_SECONDS-GIF_LEAD after each
# vignette's last beat, hard cuts, one global palette; no dithering vs
# sierra2_4a, the smaller wins.
inputs=(); filt=""; labels=""
for ((k = 0; k < n; k++)); do
  read -r start len < <(python3 -I -c "
fps, lead, secs, last, cnt = $FPS, $GIF_LEAD, $GIF_SECONDS, ${lasts[k]}, ${counts[k]}
want = round(secs * fps)
s = max(0, min(round(last - lead * fps), cnt - want))
print(s, min(want, cnt - s))")
  inputs+=(-start_number "$start" -framerate $FPS -i "${dirs[k]}/frames/%06d.png")
  filt+="[$k:v]trim=end_frame=$len,setpts=PTS-STARTPTS,fps=$GIF_FPS,scale=$GIF_WIDTH:-1:flags=lanczos,setsar=1[g$k];"
  labels+="[g$k]"
done
filt+="${labels}concat=n=$n:v=1:a=0[all]"
pal="$out/palette.png"
ff "${inputs[@]}" -filter_complex "$filt;[all]palettegen=stats_mode=full:max_colors=256[p]" -map "[p]" -frames:v 1 "$pal"
best=""; bestsize=0
for dither in none sierra2_4a; do
  g="$out/clax-reel.$dither.gif"
  ff "${inputs[@]}" -i "$pal" -filter_complex "$filt;[all][$n:v]paletteuse=dither=$dither:diff_mode=rectangle[o]" -map "[o]" -loop 0 "$g"
  sz="$(stat -f %z "$g" 2>/dev/null || stat -c %s "$g")"
  log "gif dither=$dither: $sz bytes"
  if [ -z "$best" ] || [ "$sz" -lt "$bestsize" ]; then best="$g"; bestsize="$sz"; fi
done
mv "$best" "$out/clax-reel.gif"
rm -f "$out"/clax-reel.*.gif "$pal"

echo "mp4: $(du -h "$out/clax-reel.mp4" | cut -f1)  $out/clax-reel.mp4"
echo "gif: $(du -h "$out/clax-reel.gif" | cut -f1)  $out/clax-reel.gif"
echo "clips: $out/clips/"
