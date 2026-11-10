"""The Chromium the browser tests run, pinned by checksum.

`@chromium//:browsers` is a directory laid out as PLAYWRIGHT_BROWSERS_PATH
expects for the locked @playwright/test: the host's Chromium, Chromium
headless shell and Playwright's ffmpeg (which `recordVideo` encodes with)
from browsers.bzl, which scripts/bazel-sync.sh derives from
web/pnpm-lock.yaml. Nothing is downloaded while a test runs.
"""

load(":browsers.bzl", "BROWSERS")

def _host(rctx):
    os = rctx.os.name.lower()
    arch = rctx.os.arch
    arm = arch in ("aarch64", "arm64")
    if os.startswith("mac"):
        return "mac-arm64" if arm else "mac-x64"
    if os.startswith("linux"):
        return "linux-arm64" if arm else "linux-x64"
    fail("no pinned Chromium for %s %s (bazel/playwright/browsers.bzl)" % (os, arch))

def _chromium_impl(rctx):
    for archive in BROWSERS[_host(rctx)]:
        rctx.download_and_extract(
            url = archive["url"],
            sha256 = archive["sha256"],
            output = "browsers/" + archive["dir"],
        )
        # What Playwright's installer writes when an install is complete.
        rctx.file("browsers/%s/INSTALLATION_COMPLETE" % archive["dir"], "")
        rctx.file("browsers/%s/DEPENDENCIES_VALIDATED" % archive["dir"], "")
    rctx.file("BUILD.bazel", """\
# The browsers directory, as one target (PLAYWRIGHT_BROWSERS_PATH).
exports_files(["browsers"], visibility = ["//visibility:public"])
""")

chromium_repository = repository_rule(
    implementation = _chromium_impl,
    doc = "The host's pinned Chromium builds for the locked Playwright.",
)

def _playwright_impl(_mctx):
    chromium_repository(name = "chromium")

playwright = module_extension(
    implementation = _playwright_impl,
    doc = "The pinned Chromium (repository `chromium`).",
)
