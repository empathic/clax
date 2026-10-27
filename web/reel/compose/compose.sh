#!/usr/bin/env bash
# Interim reel composer (ffmpeg + headless Chrome). A stepping stone until the
# fframes composer (docs/superpowers/specs/2026-10-27-sizzle-reel-design.md
# section 9) lands; run by hand, outputs under target/reel/ (git-ignored).
#
#   web/reel/compose/compose.sh -o OUT [--order ID,ID,...] [--titles FILE]
#                               [--cards] [--title-seconds S] [--no-camera]
#                               [--teaser ID@AT,...] [--intro TEXT] [--inserts ID[@AT],...]
#                               FRAMES_DIR [FRAMES_DIR...]
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
# The reel: --teaser's shots (TEASER_SECONDS each at 1x, whole frame, hard
# cuts; AT is a beat, BEAT+S / BEAT-S in seconds, or a frame number), then the
# --intro card (INTRO_SECONDS), then the vignettes in order, then the end card
# (reel.toml's [end] headline and footer, END_SECONDS). Vignettes named by
# --inserts are quick "and also" shots: their last two beats only (camera.py
# plan --insert), or from AT when named as ID@AT; the camera zooms less on them.
# Pace and camera: camera.py (its constants document both). From beats.json
# (beats ask, working, reveal, touch; frames are 30 fps): start..ask and
# ask..working at 1.25x, working..reveal in 1.2 s, reveal at 1x for 2.5 s,
# touch at 1x for up to 2.5 s; other vignettes play their beats' span at
# 1.0-1.2x (ClaxBot's brief, building, done count as ask, working, reveal).
# A virtual camera crops and eases toward where the pixels change
# (whole page at each reveal), with a soft spotlight while near 1x; the
# lower-third goes on after it. --no-camera shows the whole frame throughout.
# The per-frame crop path is kept in OUT/camera/<n>-<id>.tsv. Cards are 0.6 s
# with a quick slide/scale-in; every join is a hard cut.
# The GIF is each vignette's reveal window (reveal-0.3 s .. +2.2 s; for the
# fallback, the 2.5 s ending 1 s after its last beat), 720 px, 15 fps, one
# global palette, hard cuts.
# Outputs in OUT: cards/*.png, clips/<n>-<id>.mp4, camera/*.tsv, clax-reel.mp4,
# clax-reel.gif, cut.json (duration; the prelude's clips (teaser, intro) with
# start/end; per vignette id, title, context, start/end in the mp4, its clip
# and clip_duration, and its beats as t_cut/t_clip seconds, null for a beat
# outside the vignette's cut).
# Environment: FFMPEG, CHROME, GIF_WIDTH (720), GIF_FPS (15), REEL_PYTHON (a
# Python with numpy and Pillow; by default target/reel-venv in the worktree,
# made on first use).
set -euo pipefail

FFMPEG="${FFMPEG:-$HOME/.local/homebrew/bin/ffmpeg}"
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
GIF_WIDTH="${GIF_WIDTH:-720}"
GIF_FPS="${GIF_FPS:-15}"
CARD_SECONDS=0.6
END_SECONDS=3.5
TEASER_SECONDS=0.5
INTRO_SECONDS=1.8
FPS=30
FFPROBE="${FFPROBE:-$(dirname "$FFMPEG")/ffprobe}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
toml="$here/../reel.toml"
out=""; order=""; titles_file=""; overlay=1; tsec=2.5; camera=1
teaser=""; intro=""; inserts=""
dirs=()
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2 ;;
    --order) order="$2"; shift 2 ;;
    --titles) titles_file="$2"; shift 2 ;;
    --overlay-titles) overlay=1; shift ;;
    --cards) overlay=0; shift ;;
    --title-seconds) tsec="$2"; shift 2 ;;
    --no-camera) camera=0; shift ;;
    --teaser) teaser="$2"; shift 2 ;;
    --intro) intro="$2"; shift 2 ;;
    --inserts) inserts="$2"; shift 2 ;;
    -h|--help) sed -n 2,46p "$0"; exit 0 ;;
    *) dirs+=("$1"); shift ;;
  esac
done
[ -n "$out" ] && [ ${#dirs[@]} -gt 0 ] || { echo "usage: compose.sh -o OUT_DIR [--order IDS] [--titles FILE] [--cards] [--title-seconds S] [--no-camera] [--teaser ID@AT,...] [--intro TEXT] [--inserts ID[@AT],...] FRAMES_DIR..." >&2; exit 2; }
mkdir -p "$out/cards" "$out/clips" "$out/camera"
rm -f "$out/cards/cut.tsv"
out="$(cd "$out" && pwd)"

log() { echo "compose: $*" >&2; }

# The camera's Python: REEL_PYTHON, or a venv in the worktree's target/ with
# pinned numpy and Pillow, made on first use.
PY="${REEL_PYTHON:-}"
if [ -z "$PY" ]; then
  venv="$(git -C "$here" rev-parse --show-toplevel)/target/reel-venv"
  if ! "$venv/bin/python" -I -c 'import numpy, PIL' 2>/dev/null; then
    log "making $venv (numpy, Pillow)"
    python3 -I -m venv "$venv"
    "$venv/bin/python" -I -m pip install -q numpy==2.0.2 pillow==11.3.0
  fi
  PY="$venv/bin/python"
fi
cam() { "$PY" -I "$here/camera.py" "$@"; }
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
all_dirs=("${dirs[@]}")
dirs=("${sel[@]}")

# plan DIR: prints "seg START LEN SPEED OUT_FRAMES" lines, then "gif START LEN".
plan() { cam plan "$1/beats.json" "$(ls "$1/frames" | grep -c '\.png$')" ${2:+"$2"}; }
# insert_flag ID: "--insert" or "--insert-from=AT" when --inserts names ID (as ID or ID@AT).
insert_flag() {
  local e
  IFS=, read -r -a ins <<<"$inserts"
  for e in ${ins[@]+"${ins[@]}"}; do
    [ "$e" = "$1" ] && { echo --insert; return; }
    [ "${e%%@*}" = "$1" ] && { echo "--insert-from=${e#*@}"; return; }
  done
  return 0
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

clips=(); prelude=()
# The teaser: whole-frame shots, hard cuts, no titles.
if [ -n "$teaser" ]; then
  IFS=, read -r -a shots <<<"$teaser"; tl="$out/cards/teaser.txt"; : > "$tl"
  for ((k = 0; k < ${#shots[@]}; k++)); do
    tid="${shots[k]%%@*}"; tat="${shots[k]#*@}"; td=""
    for d in "${all_dirs[@]}"; do [ "$(basename "${d%/}")" = "${tid}_frames" ] && td="$(cd "$d" && pwd)"; done
    [ -n "$td" ] || { echo "compose: --teaser names $tid but no ${tid}_frames directory was given" >&2; exit 1; }
    f="$(cam at "$td/beats.json" "$tat")"; m="$(python3 -I -c "print(round($TEASER_SECONDS * $FPS))")"
    printf 'seg %s %s 1.0 %s\n' "$f" "$m" "$m" > "$out/cards/teaser-$k.plan"
    cam path "$td" "$out/cards/teaser-$k.plan" --no-camera > "$out/camera/teaser-$k.tsv"
    read -r w h < <(head -1 "$out/camera/teaser-$k.tsv" | sed -E 's/.*; ([0-9]+)x([0-9]+).*/\1 \2/')
    cam render "$td" "$out/camera/teaser-$k.tsv" |
      ff -f rawvideo -pix_fmt rgb24 -s "${w}x$h" -framerate $FPS -i - -vf "setsar=1,format=yuv420p" \
         -r $FPS -c:v libx264 -pix_fmt yuv420p -crf 20 -preset medium -an "$out/cards/teaser-$k.mp4"
    printf "file '%s'\n" "$out/cards/teaser-$k.mp4" >> "$tl"
    rm -f "$out/cards/teaser-$k.plan"
  done
  ff -f concat -safe 0 -i "$tl" -c copy "$out/clips/00-teaser.mp4"
  rm -f "$tl" "$out"/cards/teaser-*.mp4
  prelude+=("$out/clips/00-teaser.mp4")
  log "clip teaser"
fi
if [ -n "$intro" ]; then
  card_clip "$out/clips/00-intro.mp4" "title=$(urlenc "$intro")" "$INTRO_SECONDS"
  mv "$out/clips/00-intro.png" "$out/cards/intro.png"
  prelude+=("$out/clips/00-intro.mp4")
fi
for ((k = 0; k < n; k++)); do
  d="${dirs[k]}"; tag="${tags[k]}"; title="$(toml_get title "${ids[k]}")"; sub=""
  case "$title" in *"|"*) sub="${title#*|}"; title="${title%%|*}" ;; esac
  ins="$(insert_flag "${ids[k]}")"
  plan "$d" "$ins" > "$out/cards/$tag.plan"
  cam path "$d" "$out/cards/$tag.plan" $([ "$camera" = 1 ] || echo --no-camera) ${ins:+--insert} > "$out/camera/$tag.tsv"
  read -r w h < <(head -1 "$out/camera/$tag.tsv" | sed -E 's/.*; ([0-9]+)x([0-9]+).*/\1 \2/')
  printf '%s\t%s\t%s\t%s\n' "$tag" "${ids[k]}" "$title" "$sub" >> "$out/cards/cut.tsv"
  raw=(-f rawvideo -pix_fmt rgb24 -s "${w}x$h" -framerate $FPS -i -)
  enc=(-r $FPS -c:v libx264 -pix_fmt yuv420p -crf 20 -preset medium -an)
  if [ "$overlay" = 1 ]; then
    shot "$out/cards/$tag.png" "lower=1&title=$(urlenc "$title")&sub=$(urlenc "$sub")" --default-background-color=00000000
    filt="[0:v]setsar=1[body];[1:v]format=rgba,fade=t=in:st=0:d=0.25:alpha=1,fade=t=out:st=$(python3 -I -c "print($tsec-0.35)"):d=0.35:alpha=1[ov];[body][ov]overlay=0:0:enable='lt(t,$tsec)',format=yuv420p[o]"
    cam render "$d" "$out/camera/$tag.tsv" |
      ff "${raw[@]}" -loop 1 -framerate $FPS -t "$tsec" -i "$out/cards/$tag.png" -filter_complex "$filt" -map "[o]" "${enc[@]}" "$out/clips/$tag.mp4"
  else
    card_clip "$out/cards/$tag.card.mp4" "title=$(urlenc "$title")" "$CARD_SECONDS"
    cam render "$d" "$out/camera/$tag.tsv" |
      ff "${raw[@]}" -vf "setsar=1,format=yuv420p" "${enc[@]}" "$out/cards/$tag.body.mp4"
    printf "file '%s'\nfile '%s'\n" "$out/cards/$tag.card.mp4" "$out/cards/$tag.body.mp4" > "$out/cards/$tag.txt"
    ff -f concat -safe 0 -i "$out/cards/$tag.txt" -c copy -movflags +faststart "$out/clips/$tag.mp4"
    rm -f "$out/cards/$tag.card.mp4" "$out/cards/$tag.body.mp4" "$out/cards/$tag.txt"
  fi
  log "clip $tag"
  clips+=("$out/clips/$tag.mp4")
done
card_clip "$out/clips/end.mp4" "end=1&title=$(urlenc "$(toml_get headline)")&footer=$(urlenc "$(toml_get footer)")" "$END_SECONDS"
mv "$out/clips/end.png" "$out/cards/end.png" 2>/dev/null || true
clips+=("$out/clips/end.mp4")
log "clips done"

list="$out/clips.txt"; : > "$list"
for c in ${prelude[@]+"${prelude[@]}"} "${clips[@]}"; do printf "file '%s'\n" "$c" >> "$list"; done
ff -f concat -safe 0 -i "$list" -c copy -movflags +faststart "$out/clax-reel.mp4"
rm -f "$list"
log "mp4 done"

# cut.json: map each vignette's beats through its speed ramp.
python3 -I - "$out" "$FFPROBE" "$overlay" "$CARD_SECONDS" "${#prelude[@]}" ${prelude[@]+"${prelude[@]}"} "${dirs[@]}" <<'PY'
import json, subprocess, sys
out, probe, overlay, card = sys.argv[1], sys.argv[2], sys.argv[3] == "1", float(sys.argv[4])
npre = int(sys.argv[5]); pre = sys.argv[6:6 + npre]; dirs = sys.argv[6 + npre:]
dur = lambda p: float(subprocess.check_output([probe, "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", p]).decode())
fps = 30
res = {"duration": round(dur(out + "/clax-reel.mp4"), 3), "vignettes": []}
t = 0.0
res["prelude"] = []
for p in pre:  # the teaser and the intro card, before the vignettes
    cd = dur(p)
    res["prelude"].append({"clip": "clips/" + p.rsplit("/", 1)[1], "start": round(t, 3), "end": round(t + cd, 3)})
    t += cd
for line, d in zip(open(out + "/cards/cut.tsv").read().splitlines(), dirs):
    tag, id_, title, sub = line.split("\t")
    segs = [l.split() for l in open(out + "/cards/%s.plan" % tag) if l.startswith("seg")]
    segs = [(int(a), int(n), int(m)) for _, a, n, _v, m in segs]
    beats = json.load(open(d + "/beats.json"))["beats"]
    def body(f):  # source frame -> seconds into the played body; None when not played
        if f < segs[0][0] or f >= segs[-1][0] + segs[-1][1]: return None
        acc = 0.0
        for a, n, m in segs:  # source a..a+n plays as m frames (camera.py source_frames)
            if f < a: return acc
            if f < a + n: return acc + (-(-(f - a) * m // n)) / fps  # first out frame showing f
            acc += m / fps
        return acc
    lead = 0.0 if overlay else card
    def beat(b):
        x = body(b["frame"])
        if x is None:  # outside this vignette's cut
            return {"name": b["name"], "t_cut": None, "t_clip": None}
        return {"name": b["name"], "t_cut": round(t + lead + x, 3), "t_clip": round(lead + x, 3)}
    clip = "clips/%s.mp4" % tag
    cd = dur(out + "/" + clip)
    res["vignettes"].append({"id": id_, "title": title, "context": sub, "start": round(t, 3), "end": round(t + cd, 3),
        "clip": clip, "clip_duration": round(cd, 3),
        "beats": [beat(b) for b in beats]})
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
