"""Pacing and virtual camera for the interim reel composer (compose.sh).

  camera.py plan BEATS_JSON NFRAMES [--insert | --insert-from=AT]
                                               -> "seg A N SPEED M" lines, then "gif A N"
  camera.py at BEATS_JSON SPEC                 -> the source frame SPEC names (BEAT[+-S] or N)
  camera.py path FRAMES_DIR PLAN [--no-camera] [--insert]
                                               -> the per-output-frame crop path (TSV)
  camera.py render FRAMES_DIR PATH             -> rgb24 frames on stdout, for ffmpeg

plan turns a vignette's beats into speed-ramped segments: source frames A..A+N
play as M output frames (M = round(N / SPEED)). path decides, for every output
frame, which source frame it shows and which region of it (the crop, kept at
the frame's aspect and inside it, scaled back to full size), plus a spotlight
box and strength. render draws exactly what the path says. The path depends
only on the frames, the beats and the constants below, so the same inputs give
the same path byte for byte.

The camera's focus comes from where pixels change: each source frame is
compared with the one DIFF_SECONDS earlier, at 1/ANALYSIS_SCALE size; the
changed cells (below the top bar) give a robust box (the BOX_PCT..100-BOX_PCT
percentiles of their coordinates) when there are enough of them. The boxes are
pooled over FOCUS_WINDOW_SECONDS of output, padded so the box fills at most
BOX_FILL of the crop, and the crop's path is eased by a raised-cosine (Hann)
average over EASE_SECONDS: every move starts and ends at rest, and a target
held for longer than EASE_SECONDS is reached exactly.
"""
import json
import math
import sys

FPS = 30

# --- Pacing: ask-and-deliver vignettes (beats ask, working, reveal, touch) ---
LEAD_SPEED = 1.25        # start .. ask
ASK_SPEED = 1.25         # ask .. working: the drawing and the typed request, readable
WORK_SECONDS = 1.2       # working .. reveal plays in this long, whatever its length
REVEAL_SECONDS = 2.5     # reveal held at 1x for this long
TOUCH_SECONDS = 2.5      # touch .. end at 1x, at most this long (the payoff)
# --- Pacing: other vignettes (their own beat names) ---
OTHER_SPEED_MIN = 1.0    # their span plays at 1.0..1.2x, as slow as the cap allows
OTHER_SPEED_MAX = 1.2
OTHER_CAP_SECONDS = 7.0  # at most this long; beyond it, the longest gap between
OTHER_GAP_SECONDS = 0.5  # beats is partly fast-forwarded, in this long
OTHER_PRE_SECONDS = 0.3  # the span: from this before the first beat
OTHER_POST_SECONDS = 1.5  # to this after the last
OTHER_GAP_KEEP = 0.8     # a fast-forward keeps this much after and before its beats
# --- Pacing: inserts (plan --insert), the quick "and also" vignettes: their
# last two beats, from INSERT_PRE_SECONDS before the first of them to
# INSERT_POST_SECONDS after the last, at INSERT_SPEED, at most INSERT_CAP_SECONDS.
INSERT_PRE_SECONDS = 1.2
INSERT_POST_SECONDS = 1.5
INSERT_SPEED = 1.2
INSERT_CAP_SECONDS = 4.5
INSERT_ZOOM_MAX = 1.35   # a short shot keeps its page in view
# --- Camera ---
DIFF_SECONDS = 0.3       # compare each frame with the one this much earlier
ANALYSIS_SCALE = 4       # analyse at 1/4 size
DIFF_LEVEL = 20          # a cell changed when its grey level moved by more than this
TOP_IGNORE_PX = 76       # the top bar and its progress line (72 px) never draw the eye
MIN_CHANGE_PX = 320      # less changed area than this (a caret, a ticking clock) is no focus
BOX_PCT = 10             # robust box: the 10th..90th percentile of changed coordinates
FOCUS_WINDOW_SECONDS = 0.6  # boxes pooled (union, then mean) over a centred window this long
BOX_FILL = 0.7           # the focus box fills at most this share of the crop, each way
ZOOM_MAX = 1.8           # the crop is never smaller than 1/ZOOM_MAX of the frame
TOUCH_ZOOM_MAX = 1.4     # after a reveal, keep the transformed page in view around the touch
EASE_SECONDS = 1.0       # every camera move eases over this long
HOLD_SECONDS = 0.6       # after the last change, keep framing it this long ...
DRIFT_SECONDS = 2.0      # ... then drift back to the whole frame over this long
REVEAL_WIDE_SECONDS = 1.2  # the reveal shows the whole page for at least this long, once
                         # the pull-back (EASE_SECONDS from the reveal's first frame) lands
WORK_ZOOM_MAX = 1.3      # while the agent works, so the reveal's pull-back starts near the page
SPOT_DIM = 0.25          # spotlight: outside the focus box is this much darker
SPOT_ZOOM = 1.15         # only while zoomed under this (fading out from SPOT_ZOOM - 0.1)
SPOT_PAD = 0.12          # the spotlit box: the focus box grown by this share of the frame width,
SPOT_MIN = 0.35          # and at least this share of the frame each way; the spotlight
SPOT_MAX_AREA = 0.6      # fades out as that box grows past SPOT_MAX_AREA - 0.2 of the frame
SPOT_FEATHER = 0.08      # its soft edge, as a share of the frame width


# Beats that play the ask-and-deliver roles under other names (ClaxBot's brief).
BEAT_ROLES = {"brief": "ask", "building": "working", "done": "reveal"}


def roles(beats):
    """{role: first frame} for a vignette's beats, other names mapped by BEAT_ROLES."""
    at = {}
    for x in beats["beats"]:
        at.setdefault(BEAT_ROLES.get(x["name"], x["name"]), x["frame"])
    return at


def beat_frame(beats, spec):
    """The source frame of "BEAT", "BEAT+S", "BEAT-S" (seconds) or a frame number."""
    import re
    m = re.fullmatch(r"([A-Za-z_]+)?([+-][0-9.]+)?|([0-9]+)", spec)
    if not m:
        raise SystemExit("camera.py: bad frame spec " + spec)
    if m.group(3):
        return int(m.group(3))
    names = {x["name"]: x["frame"] for x in reversed(beats["beats"])}
    if m.group(1) not in names:
        raise SystemExit("camera.py: no beat %s in %s" % (m.group(1), sorted(names)))
    return max(0, names[m.group(1)] + secs(float(m.group(2) or 0)))


def secs(x):
    return round(x * FPS)


def span_plan(fr, a, e, vmin, vmax, cap_seconds):
    """Source a..e at vmin..vmax, as slow as cap_seconds allows; past the cap the
    longest gap between beats (fr) is partly fast-forwarded in OTHER_GAP_SECONDS."""
    cap = cap_seconds * FPS
    v = min(max((e - a) / cap, vmin), vmax)
    excess = (e - a) / v - cap  # output frames over the cap
    gaps = [(fr[i + 1] - fr[i], fr[i], fr[i + 1]) for i in range(len(fr) - 1)
            if a <= fr[i] and fr[i + 1] <= e]
    if excess > 0 and gaps:
        _, b0, b1 = max(gaps)
        keep = secs(OTHER_GAP_KEEP)
        room = (b1 - keep) - (b0 + keep)
        # Fast-forwarding f source frames into OTHER_GAP_SECONDS saves f/v - gap frames.
        f = min(room, math.ceil((excess + OTHER_GAP_SECONDS * FPS) * v))
        if f > OTHER_GAP_SECONDS * FPS * v:
            g0 = (b0 + keep) + (room - f) // 2
            return [(a, g0, v), (g0, g0 + f, f / (OTHER_GAP_SECONDS * FPS)), (g0 + f, e, v)]
    return [(a, e, v)]


def plan(beats, n, insert=False, insert_from=None):
    """Speed-ramped segments [(a, len, speed, out_frames)] and the GIF window (g0, glen)."""
    at = roles(beats)
    segs = []
    if not insert and "ask" in at and "reveal" in at and at["reveal"] > at["ask"]:
        ask, rev = at["ask"], at["reveal"]
        work = min(max(at.get("working", rev), ask), rev)
        segs.append((0, ask, LEAD_SPEED))
        segs.append((ask, work, ASK_SPEED))
        if rev > work:
            segs.append((work, rev, (rev - work) / (WORK_SECONDS * FPS)))
        end = rev + secs(REVEAL_SECONDS)
        if "touch" in at and at["touch"] >= rev:
            end = max(end, at["touch"] + secs(TOUCH_SECONDS))
        segs.append((rev, min(end, n), 1.0))
        g0, glen = max(0, rev - secs(0.3)), secs(2.5)
    else:
        fr = sorted(x["frame"] for x in beats["beats"]) or [n // 2]
        if insert:
            a = max(0, fr[max(0, len(fr) - 2)] - secs(INSERT_PRE_SECONDS))
            if insert_from is not None:  # the shot's best moment starts earlier or later
                a = min(max(0, insert_from), fr[-1])
            e = min(n, fr[-1] + secs(INSERT_POST_SECONDS))
            segs = span_plan(fr, a, e, INSERT_SPEED, INSERT_SPEED, INSERT_CAP_SECONDS)
        else:
            a = max(0, fr[0] - secs(OTHER_PRE_SECONDS))
            e = min(n, fr[-1] + secs(OTHER_POST_SECONDS))
            segs = span_plan(fr, a, e, OTHER_SPEED_MIN, OTHER_SPEED_MAX, OTHER_CAP_SECONDS)
        last = fr[-1]
        g0, glen = max(0, min(last + secs(1.0) - secs(2.5), n - secs(2.5))), secs(2.5)
    out = []
    for a, e, v in segs:
        e = min(e, n)
        if e > a:
            out.append((a, e - a, v, max(1, round((e - a) / v))))
    g0 = min(g0, max(0, n - 1))
    return out, (g0, min(glen, n - g0))


def read_plan(path):
    segs = []
    for line in open(path):
        p = line.split()
        if p and p[0] == "seg":
            segs.append((int(p[1]), int(p[2]), float(p[3]), int(p[4])))
    return segs


def source_frames(segs):
    """The source frame each output frame shows, and its segment index."""
    src, seg_of = [], []
    for k, (a, n, _v, m) in enumerate(segs):
        for j in range(m):
            src.append(a + (j * n) // m)
            seg_of.append(k)
    return src, seg_of


def frame_path(frames_dir, i):
    return "%s/frames/%06d.png" % (frames_dir, i)


def change_boxes(frames_dir, needed, size):
    """{source frame: (x0, y0, x1, y1) in source pixels, or None} for the frames in needed."""
    import numpy as np
    from PIL import Image

    lag = secs(DIFF_SECONDS)
    cache = {}

    def grey(i):
        if i not in cache:
            im = Image.open(frame_path(frames_dir, i)).convert("L").reduce(ANALYSIS_SCALE)
            cache[i] = np.asarray(im, dtype=np.int16)
        return cache[i]

    top = math.ceil(TOP_IGNORE_PX / ANALYSIS_SCALE)
    min_cells = MIN_CHANGE_PX / (ANALYSIS_SCALE * ANALYSIS_SCALE)
    boxes = {}
    for i in sorted(needed):
        if i < lag:
            boxes[i] = None
            continue
        d = np.abs(grey(i) - grey(i - lag)) > DIFF_LEVEL
        d[:top] = False
        ys, xs = np.nonzero(d)
        if len(xs) < min_cells:
            boxes[i] = None
            continue
        s = ANALYSIS_SCALE
        x0, x1 = np.percentile(xs, [BOX_PCT, 100 - BOX_PCT])
        y0, y1 = np.percentile(ys, [BOX_PCT, 100 - BOX_PCT])
        boxes[i] = (float(x0) * s, float(y0) * s, float(x1 + 1) * s, float(y1 + 1) * s)
        for k in [k for k in cache if k < i - lag]:
            del cache[k]
    return boxes


def hann(n):
    if n < 2:
        return [1.0]
    w = [0.5 - 0.5 * math.cos(2 * math.pi * (k + 1) / (n + 1)) for k in range(n)]
    t = sum(w)
    return [x / t for x in w]


def ease(series, width):
    """Raised-cosine moving average, edges held."""
    w = hann(width)
    h = len(w) // 2
    n = len(series)
    return [sum(w[k] * series[min(n - 1, max(0, j + k - h))] for k in range(len(w))) for j in range(n)]


def crop_for(box, W, H, zmax=ZOOM_MAX):
    """The crop (x0, y0, cw) framing box at the frame's aspect, padded, inside the frame."""
    aspect = W / H
    bx0, by0, bx1, by1 = box
    cw = max((bx1 - bx0) / BOX_FILL, (by1 - by0) / BOX_FILL * aspect, W / zmax)
    cw = min(cw, W)
    ch = cw / aspect
    cx, cy = (bx0 + bx1) / 2, (by0 + by1) / 2
    x0 = min(max(cx - cw / 2, 0.0), W - cw)
    y0 = min(max(cy - ch / 2, 0.0), H - ch)
    return x0, y0, cw


def spot_box(box, W, H):
    """The spotlit region for a focus box: padded, at least SPOT_MIN of the frame, inside it."""
    pad = SPOT_PAD * W
    out = []
    for lo, hi, size in ((box[0], box[2], W), (box[1], box[3], H)):
        lo, hi = lo - pad, hi + pad
        grow = max(0.0, SPOT_MIN * size - (hi - lo)) / 2
        lo, hi = lo - grow, hi + grow
        out.append((max(0.0, lo), min(float(size), hi)))
    return out[0][0], out[1][0], out[0][1], out[1][1]


def path(frames_dir, segs, beats, size, camera=True, insert=False):
    W, H = size
    src, seg_of = source_frames(segs)
    m = len(src)
    full = (0.0, 0.0, float(W))
    if not camera:
        return [(src[j],) + full + (0.0, 0.0, 0.0, float(W), float(H)) for j in range(m)]

    at = roles(beats)
    # Out-frame index where each segment starts, to place beats in output time.
    starts, acc = [], 0
    for _a, _n, _v, mm in segs:
        starts.append(acc)
        acc += mm

    def out_of(f):
        acc = 0
        for a, n, _v, mm in segs:
            if f < a:
                return acc
            if f < a + n:
                return acc + -(-(f - a) * mm // n)  # the first output frame showing f
            acc += mm
        return acc

    # Phases in output frames: "follow" the change, or hold the "wide" frame.
    wide = [False] * m
    zmax = [INSERT_ZOOM_MAX if insert else ZOOM_MAX] * m
    half = secs(EASE_SECONDS) // 2
    if "ask" in at and "reveal" in at and at["reveal"] > at["ask"]:
        rev = out_of(at["reveal"])
        work = out_of(min(max(at.get("working", at["reveal"]), at["ask"]), at["reveal"]))
        # The whole frame is the target from half an ease after the reveal, so the
        # pull-back runs from its first frame and lands EASE_SECONDS later.
        wide_from = rev + half
        wide_end = rev + 3 * half + secs(REVEAL_WIDE_SECONDS)
        touch = out_of(at["touch"]) if "touch" in at and at["touch"] >= at["reveal"] else m
        follow_from = max(touch, wide_end)
        for j in range(m):
            wide[j] = wide_from <= j < follow_from
            if work <= j < rev:
                zmax[j] = min(zmax[j], WORK_ZOOM_MAX)
            elif j >= rev:
                zmax[j] = min(zmax[j], TOUCH_ZOOM_MAX)

    boxes = change_boxes(frames_dir, set(src), size)
    raw = [boxes[s] for s in src]
    # Pool: union over a centred window, so typing or a moving pointer frames its whole stretch.
    win = secs(FOCUS_WINDOW_SECONDS)
    lo, hi = win // 2, win - win // 2
    pooled = []
    for j in range(m):
        got = [b for b in raw[max(0, j - lo):j + hi] if b is not None]
        if not got or wide[j]:
            pooled.append(None)
        else:
            pooled.append((min(b[0] for b in got), min(b[1] for b in got),
                           max(b[2] for b in got), max(b[3] for b in got)))
    # Mean of the pooled boxes over the same window, for a steadier frame.
    focus = []
    for j in range(m):
        got = [b for b in pooled[max(0, j - lo):j + hi] if b is not None]
        if pooled[j] is None:
            focus.append(None)
        else:
            focus.append(tuple(sum(b[k] for b in got) / len(got) for k in range(4)))

    # Targets: frame the focus; hold it HOLD_SECONDS after it goes, then drift to full.
    tx, ty, tw, ts = [], [], [], []
    sb = []  # spotlight box targets
    last, since = None, 0
    hold, drift = secs(HOLD_SECONDS), max(1, secs(DRIFT_SECONDS))
    for j in range(m):
        if wide[j]:
            last = None
        if focus[j] is not None:
            last, since = focus[j], 0
            c, s, box = crop_for(focus[j], W, H, zmax[j]), 1.0, focus[j]
        elif last is not None and since < hold + drift:
            since += 1
            k = min(1.0, max(0.0, (since - hold) / drift))
            c0 = crop_for(last, W, H, zmax[j])
            c = tuple(c0[i] + (full[i] - c0[i]) * k for i in range(3))
            s, box = 1.0 - k, last
        else:
            last = None
            c, s, box = full, 0.0, (0.0, 0.0, float(W), float(H))
        tx.append(c[0]); ty.append(c[1]); tw.append(c[2]); ts.append(s)
        sb.append(spot_box(box, W, H) if s > 0 else (0.0, 0.0, float(W), float(H)))

    n_ease = secs(EASE_SECONDS)
    x0s, y0s, cws, ss = ease(tx, n_ease), ease(ty, n_ease), ease(tw, n_ease), ease(ts, n_ease)
    sbs = [ease([b[k] for b in sb], n_ease) for k in range(4)]
    out = []
    for j in range(m):
        cw = min(float(W), cws[j])
        ch = cw * H / W
        x0 = min(max(x0s[j], 0.0), W - cw)
        y0 = min(max(y0s[j], 0.0), H - ch)
        zoom = W / cw
        fade = min(1.0, max(0.0, (SPOT_ZOOM - zoom) / 0.1))
        # A spotlit box covering most of the frame would only darken a strip at its edge.
        area = (sbs[2][j] - sbs[0][j]) * (sbs[3][j] - sbs[1][j]) / (W * H)
        fade *= min(1.0, max(0.0, (SPOT_MAX_AREA - area) / 0.2))
        out.append((src[j], x0, y0, cw, SPOT_DIM * ss[j] * fade) + tuple(sbs[k][j] for k in range(4)))
    return out


def write_path(rows, size, f):
    W, H = size
    f.write("# out src x0 y0 cw ch zoom spot sx0 sy0 sx1 sy1 (source pixels; %dx%d)\n" % (W, H))
    for j, (s, x0, y0, cw, spot, a, b, c, d) in enumerate(rows):
        f.write("%d\t%d\t%.3f\t%.3f\t%.3f\t%.3f\t%.4f\t%.4f\t%.1f\t%.1f\t%.1f\t%.1f\n" % (
            j, s, x0, y0, cw, cw * size[1] / W, W / cw, spot, a, b, c, d))


def render(frames_dir, path_file):
    import numpy as np
    from PIL import Image

    rows = [l.split("\t") for l in open(path_file) if not l.startswith("#")]
    outp = sys.stdout.buffer
    for r in rows:
        src = int(r[1])
        x0, y0, cw, ch, spot = (float(v) for v in (r[2], r[3], r[4], r[5], r[7]))
        sx0, sy0, sx1, sy1 = (float(v) for v in r[8:12])
        im = Image.open(frame_path(frames_dir, src)).convert("RGB")
        W, H = im.size
        if not (cw >= W - 1e-3 and abs(x0) < 1e-3 and abs(y0) < 1e-3):
            im = im.transform((W, H), Image.AFFINE, (cw / W, 0, x0, 0, ch / H, y0),
                              resample=Image.BICUBIC)
        if spot > 1e-3:
            k = W / cw
            fe = SPOT_FEATHER * W

            def ramp(n, a, b):
                p = np.arange(n, dtype=np.float32) + 0.5
                t = np.clip(np.minimum(p - (a - fe / 2), (b + fe / 2) - p) / fe, 0, 1)
                return t * t * (3 - 2 * t)

            mx = ramp(W, (sx0 - x0) * k, (sx1 - x0) * k)
            my = ramp(H, (sy0 - y0) * k, (sy1 - y0) * k)
            f = 1.0 - spot * (1.0 - my[:, None] * mx[None, :])
            a = np.asarray(im, dtype=np.float32) * f[:, :, None]
            im = Image.fromarray(np.clip(a + 0.5, 0, 255).astype(np.uint8))
        outp.write(im.tobytes())


def main(argv):
    cmd = argv[1]
    if cmd == "plan":
        beats = json.load(open(argv[2]))
        frm = [a.split("=", 1)[1] for a in argv if a.startswith("--insert-from=")]
        segs, (g0, glen) = plan(beats, int(argv[3]), insert="--insert" in argv or bool(frm),
                                insert_from=beat_frame(beats, frm[0]) if frm else None)
        for a, n, v, m in segs:
            print("seg", a, n, repr(round(v, 6)), m)
        print("gif", g0, glen)
    elif cmd == "at":
        print(beat_frame(json.load(open(argv[2])), argv[3]))
    elif cmd == "path":
        from PIL import Image
        frames_dir = argv[2]
        size = Image.open(frame_path(frames_dir, 0)).size
        rows = path(frames_dir, read_plan(argv[3]), json.load(open(frames_dir + "/beats.json")),
                    size, camera="--no-camera" not in argv, insert="--insert" in argv)
        write_path(rows, size, sys.stdout)
    elif cmd == "render":
        render(argv[2], argv[3])
    else:
        sys.exit("camera.py: unknown command " + cmd)


if __name__ == "__main__":
    main(sys.argv)
