"""Tests for camera.py: pacing, the crop path's rules and its determinism.

  target/reel-venv/bin/python -I web/reel/compose/camera_test.py
  REEL_FRAMES=<a FRAMES_DIR> ... also checks a real recording's path twice.

Synthetic frames: a grey page whose only change is a dark square moving
across it, so where the camera should look is known.
"""
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import camera  # noqa: E402

W, H = 800, 500


def make_frames(d, n, square_at):
    from PIL import Image, ImageDraw
    os.makedirs(d + "/frames")
    for i in range(n):
        im = Image.new("RGB", (W, H), (240, 240, 236))
        pos = square_at(i)
        if pos:
            ImageDraw.Draw(im).rectangle([pos[0], pos[1], pos[0] + 60, pos[1] + 40], fill=(20, 20, 20))
        im.save("%s/frames/%06d.png" % (d, i))


class PlanTest(unittest.TestCase):
    def test_ask_and_deliver(self):
        b = {"beats": [{"name": "ask", "frame": 30}, {"name": "working", "frame": 150},
                       {"name": "reveal", "frame": 240}, {"name": "touch", "frame": 300}]}
        segs, _ = camera.plan(b, 400)
        self.assertEqual([(a, n) for a, n, _v, _m in segs], [(0, 30), (30, 120), (150, 90), (240, 135)])
        self.assertEqual(segs[1][3], round(120 / camera.ASK_SPEED))
        self.assertEqual(segs[2][3], round(camera.WORK_SECONDS * camera.FPS))  # fixed, whatever its length
        self.assertEqual(segs[3][2], 1.0)  # reveal and touch at 1x, to touch + TOUCH_SECONDS

    def test_claxbot_beats_play_the_ask_roles(self):
        b = {"beats": [{"name": "brief", "frame": 6}, {"name": "building", "frame": 202},
                       {"name": "done", "frame": 252}]}
        segs, _ = camera.plan(b, 297)
        self.assertEqual(segs[1][:2], (6, 196))

    def test_other_vignettes_stay_under_their_cap(self):
        b = {"beats": [{"name": "a", "frame": 3}, {"name": "b", "frame": 171}, {"name": "c", "frame": 200}]}
        segs, _ = camera.plan(b, 270)
        out = sum(m for *_x, m in segs) / camera.FPS
        self.assertLessEqual(out, camera.OTHER_CAP_SECONDS + 0.1)
        self.assertTrue(all(camera.OTHER_SPEED_MIN <= v <= camera.OTHER_SPEED_MAX for i, (_a, _n, v, _m) in
                            enumerate(segs) if len(segs) == 1 or i != 1))

    def test_beat_frame(self):
        b = {"beats": [{"name": "touch", "frame": 300}]}
        self.assertEqual(camera.beat_frame(b, "touch+1"), 330)
        self.assertEqual(camera.beat_frame(b, "touch-0.5"), 285)
        self.assertEqual(camera.beat_frame(b, "42"), 42)


class PathTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.dir = cls.tmp.name + "/demo_frames"
        n = 300
        # The square sits still until frame 20, slides along the lower right
        # (the "ask"), vanishes for the reveal (frame 200 on), and comes back.
        def at(i):
            if i < 200:
                return (450 + min(max(i - 20, 0), 120), 380)
            if i >= 250:
                return (100 + (i - 250) * 2, 120)
            return None
        make_frames(cls.dir, n, at)
        cls.beats = {"fps": 30, "frames": n, "beats": [
            {"name": "ask", "frame": 10}, {"name": "working", "frame": 160},
            {"name": "reveal", "frame": 200}, {"name": "touch", "frame": 250}]}
        json.dump(cls.beats, open(cls.dir + "/beats.json", "w"))
        cls.segs, _ = camera.plan(cls.beats, n)
        cls.rows = camera.path(cls.dir, cls.segs, cls.beats, (W, H))

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def text(self, rows):
        f = io.StringIO()
        camera.write_path(rows, (W, H), f)
        return f.getvalue()

    def test_deterministic(self):
        again = camera.path(self.dir, self.segs, self.beats, (W, H))
        self.assertEqual(self.text(self.rows), self.text(again))

    def test_crop_keeps_aspect_and_stays_inside(self):
        for s, x0, y0, cw, *_ in self.rows:
            ch = cw * H / W
            self.assertGreaterEqual(x0, -1e-6)
            self.assertGreaterEqual(y0, -1e-6)
            self.assertLessEqual(x0 + cw, W + 1e-6)
            self.assertLessEqual(y0 + ch, H + 1e-6)
            self.assertGreaterEqual(W / cw, 1.0 - 1e-9)
            self.assertLessEqual(W / cw, camera.ZOOM_MAX + 1e-6)

    def test_motion_is_eased(self):
        # No jerks: frame-to-frame change of the crop's centre and width varies smoothly.
        cx = [x0 + cw / 2 for _s, x0, _y0, cw, *_ in self.rows]
        cw = [r[3] for r in self.rows]
        for series in (cx, cw):
            v = [b - a for a, b in zip(series, series[1:])]
            acc = [abs(b - a) for a, b in zip(v, v[1:])]
            self.assertLess(max(acc), 3.0, "a jerk in the camera path")

    def test_follows_the_ask(self):
        # Mid-ask, the crop is zoomed and holds the moving square.
        src = camera.source_frames(self.segs)[0]
        j = src.index(min(f for f in src if f >= 100))
        s, x0, y0, cw, *_ = self.rows[j]
        sx = 450 + (s - 20)
        self.assertGreater(W / cw, 1.2)
        self.assertTrue(x0 <= sx and sx + 60 <= x0 + cw and y0 <= 380 and 420 <= y0 + cw * H / W)

    def test_reveal_shows_the_whole_frame(self):
        src, _ = camera.source_frames(self.segs)
        rev = src.index(200)
        lands = rev + camera.secs(camera.EASE_SECONDS)
        for j in range(lands, lands + camera.secs(camera.REVEAL_WIDE_SECONDS)):
            self.assertAlmostEqual(self.rows[j][3], W, places=6)
        self.assertLess(self.rows[rev][3], W + 1e-9)

    def test_no_camera_is_the_whole_frame(self):
        rows = camera.path(self.dir, self.segs, self.beats, (W, H), camera=False)
        self.assertTrue(all(r[1:4] == (0.0, 0.0, float(W)) and r[4] == 0.0 for r in rows))

    def test_render_matches_the_path(self):
        with tempfile.NamedTemporaryFile("w", suffix=".tsv", delete=False) as f:
            camera.write_path(self.rows[:5], (W, H), f)
        try:
            out = subprocess.run([sys.executable, "-I", HERE + "/camera.py", "render", self.dir, f.name],
                                 check=True, capture_output=True).stdout
        finally:
            os.unlink(f.name)
        self.assertEqual(len(out), 5 * W * H * 3)


@unittest.skipUnless(os.environ.get("REEL_FRAMES"), "REEL_FRAMES names no recording")
class RealPathTest(unittest.TestCase):
    def test_same_path_twice(self):
        d = os.environ["REEL_FRAMES"]
        beats = json.load(open(d + "/beats.json"))
        n = len([f for f in os.listdir(d + "/frames") if f.endswith(".png")])
        segs, _ = camera.plan(beats, n)
        from PIL import Image
        size = Image.open(camera.frame_path(d, 0)).size
        texts = []
        for _ in range(2):
            f = io.StringIO()
            camera.write_path(camera.path(d, segs, beats, size), size, f)
            texts.append(f.getvalue())
        self.assertEqual(texts[0], texts[1])


if __name__ == "__main__":
    unittest.main()
