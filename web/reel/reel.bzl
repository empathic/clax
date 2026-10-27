"""The reel's recording targets, one set per vignette.

`reel_vignette(name = "<id>")` records `vignettes/<id>.ts` with the web
project's `reel:record` script (web/reel/record.ts):

- `<id>_fast`: the vignette in fast mode, `<id>_fast/clip.webm` and
  `<id>_fast/beats.json` (Playwright's `recordVideo`, real time).
- `<id>_frames`: the vignette in deterministic mode,
  `<id>_frames/frames/NNNNNN.png` and `<id>_frames/beats.json` (Chromium's
  virtual time, a screenshot per frame): the same frames each run on one
  machine.

Each reads the recorder, its own script, the web project's files and
packages, the e2e build of the web UI, the warmed `clax` and the pinned
Chromium, so a change to the UI or the daemon re-records it, and an edit to
another vignette does not. Every target is tagged `manual` (no `//...`, `just
ci` or `just test-affected` builds one), `no-sandbox` (Chromium, as the
browser tests run it: docs/bazel.md, "Browser tests"), `no-remote` and
`cpu:4`.
"""

load("@bazel_lib//lib:copy_to_bin.bzl", "copy_to_bin")
load("//bazel/js:defs.bzl", "npm_script_build")

_BROWSERS = "@chromium//:browsers"

_TAGS = [
    "manual",
    "no-remote",
    "no-sandbox",
    "cpu:4",
]

# As //web:BUILD.bazel's E2E_ENV gives the browser tests, with the binary
# named as the recorder reads it: paths from the execution root, which
# record.ts makes absolute.
_ENV = {
    "CLAX_E2E_BIN": "$(execpath //crates/clax-cli:clax)",
    "CLAX_E2E_WEB_DIST": "$(execpath //web:dist_e2e)",
    "CLAX_GALLERY__QUICKSTART": "false",
    "PLAYWRIGHT_BROWSERS_PATH": "$(execpath %s)" % _BROWSERS,
}

def reel_vignette(name):
    """The recording targets of vignette `name` (vignettes/<name>.ts).

    Args:
      name: the vignette's ID, as reel.toml names it.
    """
    copy_to_bin(
        name = name + "_script",
        srcs = ["vignettes/%s.ts" % name],
    )
    srcs = [
        ":recorder",
        ":%s_script" % name,
        "//web:files",
        "//web:node_modules",
    ]

    # Read where Bazel put them, built as the browser tests' are: the debug
    # clax (its --web-dist is a debug build's), run once by :clax_warm before
    # the recording runs it (macOS assesses each new executable on its first
    # run), the web UI with the test clock, and the pinned browsers.
    in_place = [
        "//crates/clax-cli:clax",
        "//crates/clax-cli:clax_warm",
        "//web:dist_e2e",
        _BROWSERS,
    ]
    for mode, out in [("fast", name + "_fast"), ("deterministic", name + "_frames")]:
        npm_script_build(
            name = out,
            project = "web",
            script = "reel:record",
            args = [
                "--vignette",
                name,
                "--mode",
                mode,
                "--out",
                "reel/" + out,
            ],
            srcs = srcs,
            in_place = in_place,
            out_dirs = [out],
            env = _ENV,
            tags = _TAGS,
        )
