"""The ffmpeg and ffprobe the reel's composer encodes with, pinned by checksum.

`@reel_ffmpeg//:ffmpeg` and `@reel_ffmpeg//:ffprobe` are the host's static
FFmpeg 9.0.2 builds from https://ffmpeg.martin-riedl.de (GPL, with libx264:
the composer is a developer tool and is never shipped, as
docs/superpowers/specs/2026-10-27-sizzle-reel-design.md D9 accepts).
Playwright's own ffmpeg (@chromium//:browsers) encodes VP8 alone.
"""

_BASE = "https://ffmpeg.martin-riedl.de/download/"

# By host: the build's directory, then each program's zip checksum, as the
# site publishes them beside each zip (<zip>.sha256).
_BUILDS = {
    "linux-arm64": ("linux/arm64/1789931697_9.0.2", {
        "ffmpeg": "93a76ae90db5474eecdf951a729857c64f3de23567228d6a7d5e6e8e3cd1021b",
        "ffprobe": "bcbe80fb741c180083327afaf5434812e006b33cacde2016b9aeaf6936128330",
    }),
    "linux-x64": ("linux/amd64/1789931100_9.0.2", {
        "ffmpeg": "fa8ecf4abbd290d98f7d188b8649cc6b391ae209a98452be955a15aab1909d7f",
        "ffprobe": "3f428c49070be3d24ec338602b76d412e401ffcb8a5641ef0e729181a232fc32",
    }),
    "mac-arm64": ("macos/arm64/1789931890_9.0.2", {
        "ffmpeg": "c8ed4c4e6978a03c485edbfe4e0a5dc2380f8a30bba5150531b31b094492d924",
        "ffprobe": "fcbe839537485eaee7a7a8bc5cbc0f90d53617e80943e8a5b2e31cb851197ea6",
    }),
    "mac-x64": ("macos/amd64/1789931006_9.0.2", {
        "ffmpeg": "7c6b4125b191cbf773832dc51f424cf2b6bb7da43007d1e066f95909e47cacd4",
        "ffprobe": "2322438ed2f6319a691291b247d09c69dcaa3a982460d1f269a7e1af335cfdfd",
    }),
}

def _host(rctx):
    os = rctx.os.name.lower()
    arm = rctx.os.arch in ("aarch64", "arm64")
    if os.startswith("mac"):
        return "mac-arm64" if arm else "mac-x64"
    if os.startswith("linux"):
        return "linux-arm64" if arm else "linux-x64"
    fail("no pinned ffmpeg for %s %s (bazel/reel/ffmpeg.bzl)" % (os, rctx.os.arch))

def _ffmpeg_impl(rctx):
    path, sums = _BUILDS[_host(rctx)]
    for program, sha256 in sums.items():
        rctx.download_and_extract(
            url = _BASE + path + "/" + program + ".zip",
            sha256 = sha256,
        )
    rctx.file("BUILD.bazel", """\
# The host's pinned ffmpeg and ffprobe (bazel/reel/ffmpeg.bzl).
exports_files(["ffmpeg", "ffprobe"], visibility = ["//visibility:public"])
""")

ffmpeg_repository = repository_rule(
    implementation = _ffmpeg_impl,
    doc = "The host's pinned static ffmpeg and ffprobe.",
)

def _reel_ffmpeg_impl(_mctx):
    ffmpeg_repository(name = "reel_ffmpeg")

reel_ffmpeg = module_extension(
    implementation = _reel_ffmpeg_impl,
    doc = "The pinned ffmpeg (repository `reel_ffmpeg`).",
)
