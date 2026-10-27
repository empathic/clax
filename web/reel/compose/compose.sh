#!/usr/bin/env bash
# Interim reel composer (ffmpeg + headless Chrome). A stepping stone until the
# fframes composer (docs/superpowers/specs/2026-10-27-sizzle-reel-design.md
# section 9) lands; run by hand, outputs under target/reel/ (git-ignored).
#
#   web/reel/compose/compose.sh -o OUT [--order ID,ID,...] [--titles FILE]
#                               [--cards] [--title-seconds S] FRAMES_DIR [FRAMES_DIR...]
#
# Each FRAMES_DIR holds frames/NNNNNN.png and beats.json (30 fps, 1600x1000);
# its basename minus a "_frames" suffix is the vignette ID. Titles come from
# web/reel/reel.toml, overridden per ID by --titles FILE (lines "id=title").
# A titles line may be "id=Title|context line": the lower-third's second line.
# --order plays only the named vignettes, in that order (default: argument
# order). Titles are lower-thirds over the start of each vignette, shown
# --title-seconds (2.5; fade in 0.25 s, out 0.35 s); --cards instead puts a
# 0.6 s full card (title only) before each vignette.
#
# Pace, from beats.json (beats ask, working, reveal, touch; frames are 30 fps):
#   start..ask 1.5x; ask+0.6 s..reveal 5x (the wait); reveal..reveal+1.5 s 1x
#   (the hold on the result); touch..touch+1 s 1.25x. Without a touch beat the
#   cut ends at reveal+2.5 s. Vignettes lacking ask or reveal play at 1.6x with
#   0.5 s trimmed from each end. Cards are 0.6 s with a quick slide/scale-in;
#   every join is a hard cut; the end card is 2 s.
# The GIF is each vignette's reveal window (reveal-0.3 s .. +2.2 s; for the
# fallback, the 2.5 s ending 1 s after its last beat), 720 px, 15 fps, one
# global palette, hard cuts.
# Outputs in OUT: cards/*.png, clips/<n>-<id>.mp4, clax-reel.mp4, clax-reel.gif,
# cut.json (duration; per vignette id, title, context, start/end in the mp4,
# its clip and clip_duration, and its beats as t_cut/t_clip seconds).
# Environment: FFMPEG, CHROME, GIF_WIDTH (720), GIF_FPS (15).
set -euo pipefail

FFMPEG="${FFMPEG:-$HOME/.local/homebrew/bin/ffmpeg}"
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
GIF_WIDTH="${GIF_WIDTH:-720}"
GIF_FPS="${GIF_FPS:-15}"
CARD_SECONDS=0.6
END_SECONDS=2
FPS=30
FFPROBE="${FFPROBE:-$(dirname "$FFMPEG")/ffprobe}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
toml="$here/../reel.toml"
out=""; order=""; titles_file=""; overlay=1; tsec=2.5
dirs=()
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2 ;;
    --order) order="$2"; shift 2 ;;
    --titles) titles_file="$2"; shift 2 ;;
    --overlay-titles) overlay=1; shift ;;
    --cards) overlay=0; shift ;;
    --title-seconds) tsec="$2"; shift 2 ;;
    -h|--help) sed -n 2,29p "$0"; exit 0 ;;
    *) dirs+=("$1"); shift ;;
  esac
done
[ -n "$out" ] && [ ${#dirs[@]} -gt 0 ] || { echo "usage: compose.sh -o OUT_DIR [--order IDS] [--titles FILE] [--cards] [--title-seconds S] FRAMES_DIR..." >&2; exit 2; }
mkdir -p "$out/cards" "$out/clips"
rm -f "$out/cards/cut.tsv"
out="$(cd "$out" && pwd)"

log() { echo "compose: $*" >&2; }
ff() { "$FFMPEG" -hide_banner -loglevel error -y "$@"; }

# toml_get title ID | headline | footer: values from reel.toml (a flat subset).
toml_get() {
  if [ "$1" = title ]; then
    if [ -n "$titles_file" ] && grep -q "^$2=" "$titles_file"; then
      grep -m1 "^$2=" "$titles_file" | cut -d= -f2-; return
    fi
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

shot() { # shot OUT.png QUERY [extra chrome flags]
  local dest="$1" query="$2"; shift 2
  "$CHROME" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 \
    --window-size=1600,1000 --virtual-time-budget=2000 "$@" \
    --screenshot="$dest" "file://$here/card.html?$query" >/dev/null 2>&1
  [ -s "$dest" ] || { echo "compose: Chrome made no image for $query" >&2; exit 1; }
}

# Resolve the vignettes (and their order) from the FRAMES_DIR arguments.
sel=()
if [ -n "$order" ]; then
  IFS=, read -r -a want <<<"$order"
  for id in "${want[@]}"; do
    hit=""
    for d in "${dirs[@]}"; do [ "$(basename "${d%/}")" = "${id}_frames" ] && hit="$d"; done
    [ -n "$hit" ] || { echo "compose: --order names $id but no ${id}_frames directory was given" >&2; exit 1; }
    sel+=("$hit")
  done
else
  sel=("${dirs[@]}")
fi
dirs=("${sel[@]}")

# plan DIR: prints "kind START LEN SPEED" lines (kind seg), then "gif START LEN".
plan() {
  python3 -I - "$1/beats.json" "$(ls "$1/frames" | grep -c '\.png$')" <<'PY'
import json, sys
b = json.load(open(sys.argv[1])); n = int(sys.argv[2]); fps = 30
at = {}
for x in b["beats"]:
    at.setdefault(x["name"], x["frame"])
s = lambda sec: round(sec * fps)
segs = []
if "ask" in at and "reveal" in at and at["reveal"] > at["ask"]:
    ask, rev = at["ask"], at["reveal"]
    segs.append((0, ask, 1.5))
    if ask + s(0.6) < rev:
        segs.append((ask + s(0.6), rev, 5.0))
    hold_end = rev + s(1.5)
    segs.append((rev, hold_end, 1.0))
    if "touch" in at and at["touch"] >= rev:
        t0 = max(at["touch"], hold_end)
        segs.append((t0, at["touch"] + s(1.0) if at["touch"] + s(1.0) > t0 else t0 + s(1.0), 1.25))
    else:
        segs.append((hold_end, rev + s(2.5), 1.0))
    g0 = max(0, rev - s(0.3)); glen = s(2.5)
else:
    segs.append((s(0.5), n - s(0.5), 1.6))
    last = max([x["frame"] for x in b["beats"]] or [n // 2])
    g0 = max(0, min(last + s(1.0) - s(2.5), n - s(2.5))); glen = s(2.5)
for a, e, v in segs:
    e = min(e, n)
    if e > a:
        print("seg", a, e - a, v)
g0 = min(g0, max(0, n - 1)); glen = min(glen, n - g0)
print("gif", g0, glen)
PY
}

# card_clip OUT.mp4 QUERY SECONDS [flags]: 3-phase slide/scale-in then a hold.
card_clip() {
  local dest="$1" query="$2" secs="$3" base="${1%.mp4}" ph
  local pa=0.3 pb=0.7 pc=1
  for ph in a b c; do eval "p=\$p$ph"; shot "$base.$ph.png" "$query&p=$p"; done
  local hold; hold="$(python3 -I -c "print(round($secs - 4/$FPS, 4))")"
  ff -loop 1 -framerate $FPS -t 0.0667 -i "$base.a.png" -loop 1 -framerate $FPS -t 0.0667 -i "$base.b.png" \
     -loop 1 -framerate $FPS -t "$hold" -i "$base.c.png" \
     -filter_complex "[0:v][1:v][2:v]concat=n=3:v=1:a=0,format=yuv420p,setsar=1,fps=$FPS[o]" -map "[o]" -r $FPS \
     -c:v libx264 -pix_fmt yuv420p -crf 20 -preset medium -an "$dest"
  mv "$base.c.png" "$base.png"; rm -f "$base.a.png" "$base.b.png"
}

ids=(); counts=(); tags=()
n=0
for d in "${dirs[@]}"; do
  d="$(cd "$d" && pwd)"
  [ -d "$d/frames" ] && [ -f "$d/beats.json" ] || { echo "compose: $d lacks frames/ or beats.json" >&2; exit 1; }
  b="$(basename "$d")"; id="${b%_frames}"
  dirs[$n]="$d"; n=$((n + 1))
  tags+=("$(printf '%02d-%s' "$n" "$id")"); ids+=("$id")
done

clips=()
for ((k = 0; k < n; k++)); do
  d="${dirs[k]}"; tag="${tags[k]}"; title="$(toml_get title "${ids[k]}")"; sub=""
  case "$title" in *"|"*) sub="${title#*|}"; title="${title%%|*}" ;; esac
  inputs=(); filt=""; labels=""; i=0
  while read -r kind a len v; do
    [ "$kind" = seg ] || continue
    inputs+=(-start_number "$a" -framerate $FPS -i "$d/frames/%06d.png")
    filt+="[$i:v]trim=end_frame=$len,setpts=(PTS-STARTPTS)/$v,fps=$FPS,format=yuv420p,setsar=1[s$i];"
    labels+="[s$i]"; i=$((i + 1))
  done < <(plan "$d" | tee "$out/cards/$tag.plan")
  printf '%s\t%s\t%s\t%s\n' "$tag" "${ids[k]}" "$title" "$sub" >> "$out/cards/cut.tsv"
  if [ "$overlay" = 1 ]; then
    shot "$out/cards/$tag.png" "lower=1&title=$(urlenc "$title")&sub=$(urlenc "$sub")" --default-background-color=00000000
    inputs+=(-loop 1 -framerate $FPS -t $tsec -i "$out/cards/$tag.png")
    filt+="${labels}concat=n=$i:v=1:a=0[body];[$i:v]format=rgba,fade=t=in:st=0:d=0.25:alpha=1,fade=t=out:st=$(python3 -I -c "print($tsec-0.35)"):d=0.35:alpha=1[ov];[body][ov]overlay=0:0:enable='lt(t,$tsec)',format=yuv420p[o]"
    pieces="$out/clips/$tag.mp4"
    ff "${inputs[@]}" -filter_complex "$filt" -map "[o]" -r $FPS -c:v libx264 -pix_fmt yuv420p -crf 20 -preset medium -an "$pieces"
  else
    card_clip "$out/cards/$tag.card.mp4" "title=$(urlenc "$title")" "$CARD_SECONDS"
    filt+="${labels}concat=n=$i:v=1:a=0[o]"
    ff "${inputs[@]}" -filter_complex "$filt" -map "[o]" -r $FPS -c:v libx264 -pix_fmt yuv420p -crf 20 -preset medium -an "$out/cards/$tag.body.mp4"
    printf "file '%s'\nfile '%s'\n" "$out/cards/$tag.card.mp4" "$out/cards/$tag.body.mp4" > "$out/cards/$tag.txt"
    ff -f concat -safe 0 -i "$out/cards/$tag.txt" -c copy -movflags +faststart "$out/clips/$tag.mp4"
    rm -f "$out/cards/$tag.card.mp4" "$out/cards/$tag.body.mp4" "$out/cards/$tag.txt"
  fi
  clips+=("$out/clips/$tag.mp4")
done
card_clip "$out/clips/end.mp4" "end=1&title=$(urlenc "$(toml_get headline)")&footer=$(urlenc "$(toml_get footer)")" "$END_SECONDS"
mv "$out/clips/end.png" "$out/cards/end.png" 2>/dev/null || true
clips+=("$out/clips/end.mp4")
log "clips done"

list="$out/clips.txt"; : > "$list"
for c in "${clips[@]}"; do printf "file '%s'\n" "$c" >> "$list"; done
ff -f concat -safe 0 -i "$list" -c copy -movflags +faststart "$out/clax-reel.mp4"
rm -f "$list"
log "mp4 done"

# cut.json: map each vignette's beats through its speed ramp.
python3 -I - "$out" "$FFPROBE" "$overlay" "$CARD_SECONDS" "${dirs[@]}" <<'PY'
import json, subprocess, sys
out, probe, overlay, card = sys.argv[1], sys.argv[2], sys.argv[3] == "1", float(sys.argv[4])
dirs = sys.argv[5:]
dur = lambda p: float(subprocess.check_output([probe, "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", p]).decode())
fps = 30
res = {"duration": round(dur(out + "/clax-reel.mp4"), 3), "vignettes": []}
t = 0.0
for line, d in zip(open(out + "/cards/cut.tsv").read().splitlines(), dirs):
    tag, id_, title, sub = line.split("\t")
    segs = [l.split() for l in open(out + "/cards/%s.plan" % tag) if l.startswith("seg")]
    segs = [(int(a), int(n), float(v)) for _, a, n, v in segs]
    beats = json.load(open(d + "/beats.json"))["beats"]
    def body(f):  # source frame -> seconds into the played body
        acc = 0.0
        for a, n, v in segs:
            if f < a: return acc
            if f < a + n: return acc + (f - a) / fps / v
            acc += n / fps / v
        return acc
    lead = 0.0 if overlay else card
    clip = "clips/%s.mp4" % tag
    cd = dur(out + "/" + clip)
    res["vignettes"].append({"id": id_, "title": title, "context": sub, "start": round(t, 3), "end": round(t + cd, 3),
        "clip": clip, "clip_duration": round(cd, 3),
        "beats": [{"name": b["name"], "t_cut": round(t + lead + body(b["frame"]), 3), "t_clip": round(lead + body(b["frame"]), 3)} for b in beats]})
    t += cd
json.dump(res, open(out + "/cut.json", "w"), indent=2)
PY
rm -f "$out"/cards/*.plan "$out/cards/cut.tsv"

# GIF: each vignette's reveal window, hard cuts, one global palette; no
# dithering vs sierra2_4a, the smaller wins.
inputs=(); filt=""; labels=""
for ((k = 0; k < n; k++)); do
  read -r _ start len < <(plan "${dirs[k]}" | grep '^gif')
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
