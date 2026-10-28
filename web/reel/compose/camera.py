"""Pacing, focus treatment and (opt-in) camera motion for the interim reel
composer (compose.sh).

  camera.py plan BEATS_JSON NFRAMES [--insert | --insert-from=AT]
                                               -> "seg A N SPEED M" lines, then "gif A N"
  camera.py at BEATS_JSON SPEC                 -> the source frame SPEC names (BEAT[+-S] or N)
  camera.py path FRAMES_DIR PLAN [--insert] [--camera-motion | --no-camera]
                                               -> the per-output-frame treatment (or crop) TSV
  camera.py render FRAMES_DIR PATH             -> rgb24 frames on stdout, for ffmpeg
  camera.py poster FRAMES_DIR AT OUT.jpg WIDTH  -> one source frame as a JPEG poster
  camera.py focus-report FRAMES_DIR PLAN       -> each focus region: fully on vs its first
                                                  change, fade-out vs its last (seconds)

plan turns a vignette's beats into speed-ramped segments: source frames A..A+N
play as M output frames (M = round(N / SPEED)). path decides, for every output
frame, which source frame it shows and how; render draws exactly what the path
says. A path depends only on the frames, the beats and the constants below, so
the same inputs give the same path byte for byte.

The default path never moves the frame. It draws the eye with a treatment:
outside a focus region the page is greyed (TREAT_DESAT) and dimmed toward
NEUTRAL (TREAT_DIM) behind a feathered edge; the region keeps its colour and
gets a thin accent ring with a low glow. The composer is offline, so the
focus looks ahead (focus_plan): the action at frame t is what changes from t
to t + ACTION_LOOK_SECONDS (_Changes.box; changed clusters no bigger than the
pointer left out), grown to the UI block around it (snap: the connected run
of edges that holds it). A region is chosen as its action is about to start
and moves when the action over the next SWITCH_SECONDS will be mostly outside
it (once it has stood DWELL_SECONDS); each region is fully on LEAD_SECONDS
before its first changed frame, after a FADE_SECONDS fade or cross-fade, and
fades out LINGER_SECONDS after its last one. At the working beat it moves to
the card that answered the ask. It is gone PRE_REVEAL_SECONDS before the
reveal's first changed frame, stays off REVEAL_CLEAN_SECONDS after it, and
takes no action until the touch. focus-report prints that timeline.

--camera-motion is the earlier path: a crop toward the change (path), eased by
a raised-cosine average over EASE_SECONDS, zoom 1.0..ZOOM_MAX, whole page from
each reveal, with a spotlight near 1x.
"""
import json
import math
import os
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
TOP_IGNORE_PX = 84       # the top bar and its progress line (72-80 px) never draw the eye
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
# --- Treatment (the default) ---
POINTER_PX = 40          # changed clusters no larger than this each way (the pointer, a caret,
                         # a ticking digit) never draw the focus
MIN_ACTION_PX = 600      # and the rest must cover at least this much
ACTION_PCT = 5           # action box: the 5th..95th percentile of the changed cells
BAND_MIN_W_PX = 700      # a change this wide and under BAND_MAX_H_PX tall (a header strip,
BAND_MAX_H_PX = 100      # a progress line) never draws the focus
EDGE_LEVEL = 10          # snapping: a grey step of more than this is an edge of the UI
SNAP_LINK = 3            # edges within 3 cells (12 px) belong to one block
SNAP_MAX_AREA = 0.25     # a block larger than this share of the page is no block
FOCUS_PAD_PX = 14        # the focus region's margin around its block,
FALLBACK_PAD_PX = 44     # or around the action itself when no block holds it
ACTION_LOOK_SECONDS = 0.3  # action at frame t: what changes from t to t+0.3 s (output time),
                         # so a region is known before its action starts
ADOPT_SECONDS = 0.3      # a focus needs this much action within the next SWITCH_SECONDS
SWITCH_SECONDS = 0.5     # it moves when, over the next 0.5 s, the action is mostly outside it
SWITCH_OUTSIDE = 0.5     # "outside": less than this share of an action box is inside the region
SWITCH_IOU = 0.75        # a new region this much like the current one is no move
DWELL_SECONDS = 1.0      # a region stands at least this long before it moves
SHRINK_SHARE = 0.3       # action held within this share of a large region's area narrows it
WORK_LOOK_SECONDS = (0.3, 1.5)  # at the working beat, the focus moves to what changed from this
                         # long before it to this long after it (in source time)
SAME_GAP_SECONDS = 1.0   # actions in one region this close keep it on between them
REGION_MIN_PX = (360, 170)  # a region is at least this big (around its centre, inside the page)
REGION_MAX_AREA = 0.55   # a region covering more of the page than this is no focus
LEAD_SECONDS = 0.15      # a region is fully on this long before its first changed frame,
FADE_SECONDS = 0.4       # after fading (or cross-fading) in over this long
LINGER_SECONDS = 0.8     # it starts fading out this long after its last changed frame,
FADE_OUT_SECONDS = 0.4   # over this long
REVEAL_CHANGE = 0.05     # the reveal is the first frame, from 0.3 s before its beat, where this
                         # share of the page changes from the frame before (the beat can be off)
PRE_REVEAL_SECONDS = 0.3  # the treatment is gone this long before that frame ...
REVEAL_CLEAN_SECONDS = 1.2  # ... stays off this long after it, and takes no action until the touch
TREAT_DIM = 0.30         # outside the focus: this much toward NEUTRAL (inserts: INSERT_DIM)
INSERT_DIM = 0.2
TREAT_DESAT = 0.78       # and this much of its colour gone (inserts: 0.6)
INSERT_DESAT = 0.6
NEUTRAL = (96, 96, 100)  # the grey it fades toward
FEATHER_PX = 28          # the soft edge of the focus, outside its rectangle
CORNER_PX = 12           # its rounded corners, as the UI's cards round theirs
RING_RGB = (240, 112, 64)  # the highlight ring: the accent, about 2.5 px, centred on the edge
RING_ALPHA = 0.8
RING_HALF_PX = 1.25
GLOW_ALPHA = 0.16        # and a low outer glow, falling off over GLOW_PX
GLOW_PX = 9.0
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



# --- Treatment: focus regions from the frames (the default; no camera motion) ---

def _grey(frames_dir, i):
    import numpy as np
    from PIL import Image
    im = Image.open(frame_path(frames_dir, i)).convert("L").reduce(ANALYSIS_SCALE)
    return np.asarray(im, dtype=np.int16)


def _components(cells, link):
    """Connected components of the True cells (cells within link of each other connect),
    as lists of (y, x)."""
    import numpy as np
    ys, xs = np.nonzero(cells)
    todo = set(zip(ys.tolist(), xs.tolist()))
    comps = []
    while todo:
        seed = min(todo)  # deterministic order
        todo.discard(seed)
        comp, stack = [seed], [seed]
        while stack:
            y, x = stack.pop()
            for dy in range(-link, link + 1):
                for dx in range(-link, link + 1):
                    q = (y + dy, x + dx)
                    if q in todo:
                        todo.discard(q)
                        comp.append(q)
                        stack.append(q)
        comps.append(comp)
    return comps


class _Changes:
    """Where frames differ, at 1/ANALYSIS_SCALE size below the top bar."""

    def __init__(self, frames_dir):
        self.dir, self.cache, self.boxes = frames_dir, {}, {}
        self.top = math.ceil(TOP_IGNORE_PX / ANALYSIS_SCALE)

    def grey(self, i):
        if i not in self.cache:
            if len(self.cache) >= 96:
                del self.cache[next(iter(self.cache))]
            self.cache[i] = _grey(self.dir, i)
        return self.cache[i]

    def mask(self, a, b):
        import numpy as np
        d = np.abs(self.grey(b) - self.grey(a)) > DIFF_LEVEL
        d[:self.top] = False
        return d

    def share(self, a, b):
        """The share of the page that changed from a to b."""
        return float(self.mask(a, b).mean())

    def moved(self, a, b, region):
        """Changed cells from a to b inside region."""
        s = ANALYSIS_SCALE
        x0, y0, x1, y1 = (int(region[0] // s), int(region[1] // s),
                          int(math.ceil(region[2] / s)), int(math.ceil(region[3] / s)))
        return int(self.mask(a, b)[y0:y1, x0:x1].sum())

    def box(self, a, b):
        """The action from a to b: a box, or None. Changed clusters no larger than
        POINTER_PX each way (the pointer, a caret, ticking digits) and wide header
        strips are left out; the rest must cover MIN_ACTION_PX."""
        import numpy as np
        if a == b:
            return None
        if (a, b) in self.boxes:
            return self.boxes[(a, b)]
        d = self.mask(a, b)
        s = ANALYSIS_SCALE * 2  # clusters on an 8 px grid
        small = POINTER_PX / s
        h, w = d.shape[0] // 2 * 2, d.shape[1] // 2 * 2
        c = d[:h, :w].reshape(h // 2, 2, w // 2, 2).any(axis=(1, 3))
        n = int(c.sum())
        out = None
        if n * s * s >= MIN_ACTION_PX:
            if n > 3000:  # a whole-page change: nothing small to sort out
                ys, xs = np.nonzero(c)
            else:
                keep = []
                for comp in _components(c, 2):
                    cy = [p[0] for p in comp]; cx = [p[1] for p in comp]
                    if max(cy) - min(cy) + 1 > small or max(cx) - min(cx) + 1 > small:
                        keep += comp
                ys = np.array([p[0] for p in keep]); xs = np.array([p[1] for p in keep])
            if len(xs) * s * s >= MIN_ACTION_PX:
                x0, x1 = np.percentile(xs, [ACTION_PCT, 100 - ACTION_PCT])
                y0, y1 = np.percentile(ys, [ACTION_PCT, 100 - ACTION_PCT])
                b_ = (float(x0) * s, float(y0) * s, float(x1 + 1) * s, float(y1 + 1) * s)
                if not (b_[2] - b_[0] > BAND_MIN_W_PX and b_[3] - b_[1] < BAND_MAX_H_PX):
                    out = b_
        self.boxes[(a, b)] = out
        return out


def snap(frames_dir, i, box, size):
    """Grow box to the UI block around it in source frame i: the connected run of
    edges (a card's border, its text) that holds it, edges within SNAP_LINK cells
    joined (then within one cell, if that block is most of the page); else box
    padded by FALLBACK_PAD_PX."""
    import numpy as np
    W, H = size
    g = _grey(frames_dir, i)
    e = np.zeros(g.shape, bool)
    e[:, 1:] |= np.abs(np.diff(g, axis=1)) > EDGE_LEVEL
    e[1:, :] |= np.abs(np.diff(g, axis=0)) > EDGE_LEVEL
    e[:math.ceil(TOP_IGNORE_PX / ANALYSIS_SCALE)] = False
    s = ANALYSIS_SCALE
    bx0, by0, bx1, by1 = (int(box[0] // s), int(box[1] // s), int(math.ceil(box[2] / s)), int(math.ceil(box[3] / s)))
    limit = SNAP_MAX_AREA * e.size
    def pad(b, p=FOCUS_PAD_PX):
        return (max(0.0, b[0] - p), max(0.0, b[1] - p), min(float(W), b[2] + p), min(float(H), b[3] + p))
    seed = np.zeros(e.shape, bool)
    seed[by0:by1, bx0:bx1] = e[by0:by1, bx0:bx1]
    if not seed.any():
        return pad(box, FALLBACK_PAD_PX)
    for link in range(SNAP_LINK, 0, -1):
        cur, ok = seed.copy(), True
        for _ in range(400):
            grown = cur.copy()
            for r in range(1, link + 1):
                grown[r:, :] |= cur[:-r, :]; grown[:-r, :] |= cur[r:, :]
                grown[:, r:] |= cur[:, :-r]; grown[:, :-r] |= cur[:, r:]
            grown &= e
            grown |= cur
            if (grown == cur).all():
                break
            cur = grown
            ys, xs = np.nonzero(cur)
            if (ys.max() - ys.min() + 1) * (xs.max() - xs.min() + 1) > limit:
                ok = False
                break
        if ok:
            ys, xs = np.nonzero(cur)
            b = (min(box[0], xs.min() * s), min(box[1], ys.min() * s),
                 max(box[2], (xs.max() + 1) * s), max(box[3], (ys.max() + 1) * s))
            return pad(tuple(float(v) for v in b))
    return pad(box, FALLBACK_PAD_PX)


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



def _out_of(segs):
    def out_of(f):  # the first output frame showing source frame f
        acc = 0
        for a, n, _v, mm in segs:
            if f < a:
                return acc
            if f < a + n:
                return acc + -(-(f - a) * mm // n)
            acc += mm
        return acc
    return out_of


def _area(b):
    return max(0.0, b[2] - b[0]) * max(0.0, b[3] - b[1])


def _inter(a, b):
    return _area((max(a[0], b[0]), max(a[1], b[1]), min(a[2], b[2]), min(a[3], b[3])))


def _union(bs):
    return (min(b[0] for b in bs), min(b[1] for b in bs), max(b[2] for b in bs), max(b[3] for b in bs))


def _smooth(t):
    t = min(1.0, max(0.0, t))
    return t * t * (3 - 2 * t)


def focus_plan(frames_dir, segs, beats, size):
    """The focus timeline: (intervals, clean) where each interval is a dict with the
    region and, in output frames, in_start, on_full, first (its first changed frame),
    last (its last), off_start and off_end; clean is the reveal's (start, end) or None.

    Offline, so it looks ahead: the action at frame t is what changes from t to
    t + ACTION_LOOK_SECONDS; a region is chosen when the action is about to start,
    moves when the action over the next SWITCH_SECONDS will be mostly outside it,
    and is fully on LEAD_SECONDS before its first changed frame."""
    W, H = size
    src, _ = source_frames(segs)
    m = len(src)
    at = roles(beats)
    out_of = _out_of(segs)
    ch = _Changes(frames_dir)
    nframes = len([f for f in os.listdir(frames_dir + "/frames") if f.endswith(".png")])
    K = secs(ACTION_LOOK_SECONDS)

    clean, quiet_until, work = None, None, None
    if "ask" in at and "reveal" in at and at["reveal"] > at["ask"]:
        vis = at["reveal"]
        for f in range(max(0, at["reveal"] - secs(0.3)), min(nframes - 1, at["reveal"] + secs(1.5))):
            if ch.share(f, f + 1) > REVEAL_CHANGE:
                vis = f + 1  # the first frame of the changed page
                break
        v = out_of(vis)
        touch = out_of(at["touch"]) if "touch" in at and at["touch"] >= at["reveal"] else m
        clean = (max(0, v - secs(PRE_REVEAL_SECONDS)), min(m, v + secs(REVEAL_CLEAN_SECONDS)))
        quiet_until = max(clean[1], touch)  # no new action until the touch
        if "working" in at and at["ask"] < at["working"] < at["reveal"]:
            work = out_of(at["working"])

    def quiet(j):
        return clean is not None and clean[0] <= j < quiet_until

    act = []
    for j in range(m):
        e = min(m - 1, j + K)
        if clean is not None and j < clean[0]:
            e = min(e, clean[0] - 1)  # never look into the reveal
        act.append(None if quiet(j) else ch.box(src[j], src[e]))

    def iou(a, b):
        return _inter(a, b) / max(1.0, _area(a) + _area(b) - _inter(a, b))

    def fit(region):
        region = list(region)
        for lo, hi, size_, least in ((0, 2, W, REGION_MIN_PX[0]), (1, 3, H, REGION_MIN_PX[1])):
            grow = max(0.0, least - (region[hi] - region[lo])) / 2
            a, b = region[lo] - grow, region[hi] + grow
            shift = max(0.0, -a) - max(0.0, b - size_)
            region[lo], region[hi] = max(0.0, a + shift), min(float(size_), b + shift)
        region = tuple(region)
        return None if _area(region) > REGION_MAX_AREA * W * H else region

    # Pass 1: which region each action belongs to. occ: [region, [action frames]].
    occ = []
    R, Rstart, Rlast = None, None, None
    adopt, look, dwell = secs(ADOPT_SECONDS), secs(SWITCH_SECONDS), secs(DWELL_SECONDS)

    def begin(region, j):
        nonlocal R, Rstart, Rlast
        region = fit(region)
        R, Rstart, Rlast = region, j, j
        if region is not None:
            occ.append([region, [j]])

    forced = None
    if work is not None:
        wsrc = [f for f in range(at["working"] - secs(WORK_LOOK_SECONDS[0]),
                                 min(at["working"] + secs(WORK_LOOK_SECONDS[1]),
                                     at["reveal"] - secs(PRE_REVEAL_SECONDS)))
                if 0 < f < nframes - 1]
        got = [(f, ch.box(f, min(nframes - 1, f + K))) for f in wsrc]
        got = [(f, b) for f, b in got if b is not None]
        if got:
            boxes = [b for _f, b in got]
            count = [sum(iou(g, h) > 0.7 for h in boxes) for g in boxes]
            # Held for longer than a single change's look-ahead lasts (adopt + K/3).
            held = [k for k in range(len(boxes)) if count[k] >= adopt + K // 3]
            best = (min(held, key=lambda k: (_area(boxes[k]), -k)) if held
                    else max(range(len(boxes)), key=lambda k: (count[k], k)))
            group = [(f, b) for f, b in got if iou(boxes[best], b) > 0.7]
            # The card that answered the ask, from the first output frame its change shows in.
            first = max(work, min(out_of(f) for f, _b in group))
            forced = (min(first, m - 1), snap(frames_dir, group[0][0] + K, _union([b for _f, b in group]), size))
            if clean is not None and forced[0] > clean[0] - secs(0.3) - secs(FADE_SECONDS) - secs(LEAD_SECONDS):
                forced = None  # it could not stand before the reveal

    for j in range(m):
        if forced and j == forced[0] and not quiet(j):
            begin(forced[1], j)
            continue
        c = act[j]
        if c is None:
            continue
        fut = [b for b in act[j:j + look] if b is not None]
        # Just before the working beat's region, keep the one on screen: no flash between.
        # No new region that could not stand DWELL_SECONDS (fully on) before the working
        # beat's region or the reveal takes over.
        span = dwell + secs(FADE_SECONDS) + secs(LEAD_SECONDS)
        short = secs(0.5) + secs(FADE_SECONDS) + secs(LEAD_SECONDS)  # on at least 0.5 s
        hold = ((forced is not None and forced[0] - span < j < forced[0] and R is not None)
                or (clean is not None and clean[0] - short < j < clean[0]))
        if R is None or j - Rlast > secs(SAME_GAP_SECONDS):
            if len(fut) >= adopt and not hold:
                begin(snap(frames_dir, src[min(m - 1, j + K)], _union(fut), size), j)
            continue
        inside = _inter(c, R) >= SWITCH_OUTSIDE * _area(c)
        outs = [b for b in fut if _inter(b, R) < SWITCH_OUTSIDE * _area(b)]
        stood = j - Rstart >= dwell + secs(FADE_SECONDS)  # fully on for DWELL_SECONDS
        if not inside and stood and not hold and len(outs) >= adopt and len(outs) >= 0.7 * len(fut):
            new = snap(frames_dir, src[min(m - 1, j + K)], _union(outs), size)
            if iou(new, R) < SWITCH_IOU:
                begin(new, j)
                continue
        if inside:
            Rlast = j
            if occ and occ[-1][0] == R:
                occ[-1][1].append(j)
            if (stood and not hold and _area(R) > 0.12 * W * H and len(fut) >= adopt
                    and _area(_union(fut)) < SHRINK_SHARE * _area(R)):
                new = snap(frames_dir, src[min(m - 1, j + K)], _union(fut), size)
                if _area(new) < 0.6 * _area(R) and iou(new, R) < SWITCH_IOU:
                    begin(new, j)

    # Pass 2: on-intervals, timed to the changes themselves.
    intervals = []
    for region, frames in occ:
        runs, cur = [], [frames[0]]
        for j in frames[1:]:
            if j - cur[-1] > secs(SAME_GAP_SECONDS):
                runs.append(cur)
                cur = []
            cur.append(j)
        runs.append(cur)
        for run in runs:
            def changed(j):
                return j > 0 and ch.moved(src[j - 1], src[j], region) > 0
            span0 = range(run[0], min(m, run[0] + K + 1))
            first = next((j for j in span0 if changed(j)), run[0])
            span1 = range(min(m - 1, run[-1] + K), run[-1], -1)
            last = next((j for j in span1 if changed(j)), run[-1])
            intervals.append({"region": region, "first": first, "last": max(first, last)})
    lead, fade = secs(LEAD_SECONDS), secs(FADE_SECONDS)
    for iv in intervals:
        iv["on_full"] = iv["first"] - lead
        iv["in_start"] = iv["on_full"] - fade
        iv["off_start"] = iv["last"] + secs(LINGER_SECONDS)
        iv["off_end"] = iv["off_start"] + secs(FADE_OUT_SECONDS)
        if clean is not None and iv["first"] >= clean[1] and iv["in_start"] < clean[1]:
            # Back after the reveal: never before its clean stretch ends, and never abrupt.
            iv["in_start"], iv["on_full"] = clean[1], clean[1] + fade
        if iv["in_start"] < 0:  # on from the clip's first frame (it opens on a cut)
            iv["in_start"], iv["on_full"] = 0, 0
    for a, b in zip(intervals, intervals[1:]):
        if b["region"] != a["region"] and b["in_start"] < a["off_end"]:
            # Cross-fade: the old region goes as the new one comes.
            a["off_start"] = min(a["off_start"], b["in_start"])
            a["off_end"] = max(a["off_start"] + 1, min(a["off_end"], b["on_full"]))
    for iv in intervals:
        if clean is not None and iv["first"] < clean[0] and iv["off_end"] > clean[0]:
            iv["off_end"] = clean[0]
            iv["off_start"] = min(iv["off_start"], clean[0] - secs(PRE_REVEAL_SECONDS))
    return intervals, clean


def focus_path(frames_dir, segs, beats, size, insert=False, focus_dirs=None):
    """Rows (src, amount, [(region, alpha), ...]) per output frame: the frame never
    moves; the regions of focus_plan fade in ahead of their action and out after it."""
    src, _ = source_frames(segs)
    amount = (INSERT_DIM, INSERT_DESAT) if insert else (TREAT_DIM, TREAT_DESAT)
    found = load_script(frames_dir, focus_dirs)
    if found is not None:
        nframes = len([f for f in os.listdir(frames_dir + "/frames") if f.endswith(".png")])
        return script_path(found[0], found[1], frames_dir, segs, beats, amount, nframes, size)
    intervals, _clean = focus_plan(frames_dir, segs, beats, size)
    rows = []
    for j in range(len(src)):
        alpha = {}
        for iv in intervals:
            if not iv["in_start"] <= j < iv["off_end"]:
                continue
            up = (1.0 if iv["on_full"] <= iv["in_start"] else
                  _smooth((j - iv["in_start"] + 1) / (iv["on_full"] - iv["in_start"])))
            down = 1.0 - _smooth((j - iv["off_start"] + 1) / max(1, iv["off_end"] - iv["off_start"]))
            a = min(up, down if j >= iv["off_start"] else 1.0)
            if a > 1e-3:
                alpha[iv["region"]] = max(alpha.get(iv["region"], 0.0), a)
        rows.append((src[j], amount, [((r,), a) for r, a in sorted(alpha.items())]))
    return rows


# --- Focus scripts: what is lit, and when, as named targets ---
CROSS_SECONDS = 0.35     # a scripted shot fades (or cross-fades) in over this long, ending at its at
TARGET_PAD_PX = 12       # a lit target's region: its box plus this much each way
FOCUS_DIRS = [os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "vignettes")]


def vignette_id(frames_dir):
    b = os.path.basename(os.path.normpath(frames_dir))
    return b[:-len("_frames")] if b.endswith("_frames") else b


def load_script(frames_dir, dirs=None):
    """The vignette's focus script (web/reel/focus.ts documents it): the copy its
    recording carries (focus.json), else <id>.focus.json in one of dirs (by
    default web/reel/vignettes/ beside this composer); None without one."""
    own = os.path.join(frames_dir, "focus.json")
    paths = [own] + [os.path.join(d, vignette_id(frames_dir) + ".focus.json") for d in (dirs or FOCUS_DIRS)]
    for path in paths:
        if os.path.exists(path):
            with open(path, encoding="utf-8") as f:
                return json.load(f), path
    return None


def load_rects(frames_dir):
    """{target: [(frame, box or None), ...]} from the recording's focus-rects.json."""
    path = os.path.join(frames_dir, "focus-rects.json")
    if not os.path.exists(path):
        return {}
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    return {k: [(int(fr), tuple(b) if b else None) for fr, b in v] for k, v in data["targets"].items()}


def _box_at(changes, f):
    """The box a change list gives at frame f."""
    box = None
    for fr, b in changes:
        if fr > f:
            break
        box = b
    return box


def script_path(script, where, frames_dir, segs, beats, amount, nframes, size):
    """Rows for a focus script. Each shot lights its targets: each target's region
    is the union of its recorded boxes over the shot (or its override), plus
    TARGET_PAD_PX, inside the frame. A shot is fully on from its at (after a
    CROSS_SECONDS fade or cross-fade ending there) until its until (default:
    the next shot's at; the last one holds to the clip's end), and fades over
    the CROSS_SECONDS before that. A shot at the clip's start is on from its
    first frame, so no clip opens or ends on a fade. A clean span holds no shot."""
    W, H = size
    src, _ = source_frames(segs)
    m = len(src)
    out_of = _out_of(segs)
    fade = max(1, secs(CROSS_SECONDS))
    rects = load_rects(frames_dir)
    overrides = {k: tuple(v) for k, v in (script.get("overrides") or {}).items()}
    import re

    def when(spec, after):
        """A time spec as a source frame (see web/reel/focus.ts)."""
        if isinstance(spec, int):
            return spec
        mt = re.fullmatch(r"(@?)([a-z][a-z0-9_]*)([+-][0-9.]+)?", spec)
        if not mt:
            raise SystemExit("%s: bad time %r" % (where, spec))
        off = secs(float(mt.group(3) or 0))
        if not mt.group(1):
            return max(0, beat_frame(beats, mt.group(2)) + off)
        changes = rects.get(mt.group(2))
        if not changes:
            raise SystemExit("%s: %r waits for target %s, which the recording has no boxes of"
                             % (where, spec, mt.group(2)))
        f = next((fr for fr in range(after, nframes) if _box_at(changes, fr) is not None), None)
        if f is None:
            raise SystemExit("%s: target %s never shows from frame %d" % (where, mt.group(2), after))
        return max(0, f + off)

    def out(f):
        return min(m, out_of(f)) if f >= segs[0][0] else 0

    shots = script["shots"]
    starts, prev = [], 0
    for sh in shots:
        prev = when(sh["at"], prev)
        starts.append(prev)
    timed = []
    for k, sh in enumerate(shots):
        a = starts[k]
        b = when(sh["until"], a) if "until" in sh else (starts[k + 1] if k + 1 < len(shots) else nframes)
        regions = []
        for name in sh["targets"]:
            if name in overrides:
                x, y, w, h = overrides[name]
                box = (x, y, x + w, y + h)
            else:
                seen = [_box_at(rects.get(name, []), f) for f in range(a, max(a + 1, b))]
                seen = [(x, y, x + w, y + h) for x, y, w, h in (s_ for s_ in seen if s_)]
                if not seen:
                    sys.stderr.write("camera.py: %s: shot %r: target %s is not on screen in it\n"
                                     % (where, sh.get("label"), name))
                    continue
                box = _union(seen)
            p = TARGET_PAD_PX
            regions.append((max(0.0, box[0] - p), max(0.0, box[1] - p), min(float(W), box[2] + p), min(float(H), box[3] + p)))
        on = out(a)
        off = None if "until" not in sh and k + 1 == len(shots) else out(b)
        if off is not None and off <= on:
            raise SystemExit("%s: shot %r ends before it starts" % (where, sh.get("label")))
        if regions:
            timed.append((tuple(regions), on, off, sh))
    for c in script.get("clean") or []:
        a, b = out(when(c["at"], 0)), out(when(c["until"], 0))
        for regions, on, off, sh in timed:
            if on - fade < b and (off is None or off > a):
                raise SystemExit("%s: shot %r overlaps the clean span %r" % (where, sh.get("label"), c.get("label")))
    rows = []
    for j in range(m):
        live = []
        for regions, on, off, _sh in timed:
            if j < on - fade or (off is not None and j >= off):
                continue
            up = 1.0 if on - fade <= 0 else _smooth((j - (on - fade) + 1) / fade)
            down = 1.0 if off is None or off >= m else _smooth((off - j) / fade)
            a = min(up, down)
            if a > 1e-3:
                live.append((regions, round(a, 4)))
        rows.append((src[j], amount, live))
    return rows


def focus_report(frames_dir, segs, beats, size):
    """Lines: per interval, when it is fully on vs its first change, and its fade-out
    vs its last change (seconds of the clip)."""
    intervals, clean = focus_plan(frames_dir, segs, beats, size)
    t = lambda j: j / FPS
    lines = ["region\tin\ton\tfirst\tlead\tlast\toff\tgap\tend"]
    for iv in intervals:
        lines.append("%s\t%.2f\t%.2f\t%.2f\t%+.2f\t%.2f\t%.2f\t%.2f\t%.2f" % (
            ",".join("%d" % v for v in iv["region"]), t(iv["in_start"]), t(iv["on_full"]),
            t(iv["first"]), t(iv["first"] - iv["on_full"]), t(iv["last"]), t(iv["off_start"]),
            t(iv["off_start"] - iv["last"]), t(iv["off_end"])))
    if clean:
        lines.append("clean\t%.2f\t%.2f" % (t(clean[0]), t(clean[1])))
    return lines


def write_focus(rows, size, f):
    W, H = size
    f.write("# treatment: out src dim desat [x0,y0,x1,y1;...|alpha]... (source pixels; %dx%d)\n" % (W, H))
    for j, (s, (dim, desat), live) in enumerate(rows):
        cells = ["%d" % j, "%d" % s, "%.3f" % dim, "%.3f" % desat]
        cells += [";".join("%.1f,%.1f,%.1f,%.1f" % tuple(q) for q in rects) + "|%.4f" % a for rects, a in live]
        f.write("\t".join(cells) + "\n")


def _sdf(region, W, H):
    """Signed distance (px) to the rounded rectangle region: negative inside."""
    import numpy as np
    x0, y0, x1, y1 = region
    r = CORNER_PX
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    hx, hy = (x1 - x0) / 2 - r, (y1 - y0) / 2 - r
    px = np.abs(np.arange(W, dtype=np.float32) + 0.5 - cx) - hx
    py = np.abs(np.arange(H, dtype=np.float32) + 0.5 - cy) - hy
    qx, qy = np.maximum(px, 0)[None, :], np.maximum(py, 0)[:, None]
    outside = np.sqrt(qx * qx + qy * qy)
    inside = np.minimum(np.maximum(px[None, :], py[:, None]), 0)
    return outside + inside - r


def render_focus(frames_dir, rows):
    import numpy as np
    from PIL import Image
    outp = sys.stdout.buffer
    cache = {}
    neutral = np.array(NEUTRAL, np.float32)
    ring_rgb = np.array(RING_RGB, np.float32)
    for r in rows:
        src, dim, desat = int(r[1]), float(r[2]), float(r[3])
        live = []
        for c in r[4:]:
            rects, alpha = c.split("|")
            live.append((tuple(tuple(float(v) for v in q.split(",")) for q in rects.split(";")), float(alpha)))
        im = Image.open(frame_path(frames_dir, src)).convert("RGB")
        if not live:
            outp.write(im.tobytes())
            continue
        W, H = im.size
        a = np.asarray(im, dtype=np.float32)
        t = np.zeros((H, W), np.float32)
        ring = np.zeros((H, W), np.float32)
        keep = {q for rects, _a in live for q in rects}
        cache = {k: v for k, v in cache.items() if k in keep}
        for rects, alpha in live:
            outside_all, ring_all = None, None
            for key in rects:
                if key not in cache:
                    d = _sdf(key, W, H)
                    u = np.clip(d / FEATHER_PX, 0, 1)
                    outside = u * u * (3 - 2 * u)  # 0 inside, 1 a feather beyond the edge
                    rg = RING_ALPHA * np.clip(RING_HALF_PX + 0.5 - np.abs(d), 0, 1)  # anti-aliased line
                    rg = np.maximum(rg, GLOW_ALPHA * np.exp(-np.maximum(d, 0) / GLOW_PX) * (d > 0))
                    cache[key] = (outside.astype(np.float32), np.clip(rg, 0, 1).astype(np.float32))
                outside, rg = cache[key]
                # Several rects light together: clear inside any of them.
                outside_all = outside if outside_all is None else np.minimum(outside_all, outside)
                ring_all = rg if ring_all is None else np.maximum(ring_all, rg)
            t += alpha * outside_all
            ring = np.maximum(ring, alpha * ring_all)
        t = np.clip(t, 0, 1)
        lum = a[:, :, 0] * 0.299 + a[:, :, 1] * 0.587 + a[:, :, 2] * 0.114  # (matmul warns spuriously on Accelerate)
        treated = a + (lum[:, :, None] - a) * desat
        treated = treated * (1 - dim) + neutral * dim
        out = a + (treated - a) * t[:, :, None]
        out += (ring_rgb - out) * ring[:, :, None]
        outp.write(np.clip(out + 0.5, 0, 255).astype(np.uint8).tobytes())

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
        segs, beats = read_plan(argv[3]), json.load(open(frames_dir + "/beats.json"))
        if "--camera-motion" in argv or "--no-camera" in argv:
            rows = path(frames_dir, segs, beats, size, camera="--no-camera" not in argv,
                        insert="--insert" in argv)
            write_path(rows, size, sys.stdout)
        else:
            dirs = [x.split("=", 1)[1] for x in argv if x.startswith("--focus-dir=")]
            write_focus(focus_path(frames_dir, segs, beats, size, insert="--insert" in argv
                                   or any(x.startswith("--insert-from=") for x in argv), focus_dirs=dirs or None),
                        size, sys.stdout)
    elif cmd == "poster":
        # poster FRAMES_DIR AT OUT.jpg WIDTH: that source frame, full colour, as a JPEG.
        from PIL import Image
        frames_dir = argv[2]
        f = beat_frame(json.load(open(frames_dir + "/beats.json")), argv[3])
        im = Image.open(frame_path(frames_dir, f)).convert("RGB")
        w = int(argv[5])
        im.resize((w, round(im.size[1] * w / im.size[0])), Image.LANCZOS).save(
            argv[4], "JPEG", quality=90, optimize=True, progressive=True)
    elif cmd == "focus-report":
        from PIL import Image
        frames_dir = argv[2]
        size = Image.open(frame_path(frames_dir, 0)).size
        print("\n".join(focus_report(frames_dir, read_plan(argv[3]),
                                      json.load(open(frames_dir + "/beats.json")), size)))
    elif cmd == "render":
        if open(argv[3]).readline().startswith("# treatment"):
            render_focus(argv[2], [l.rstrip("\n").split("\t") for l in open(argv[3]) if not l.startswith("#")])
        else:
            render(argv[2], argv[3])
    else:
        sys.exit("camera.py: unknown command " + cmd)


if __name__ == "__main__":
    main(sys.argv)
