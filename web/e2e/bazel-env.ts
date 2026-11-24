// Under Bazel (bazel/js/npm_script.mjs runs the tests in a copy of the
// project), the paths Bazel hands the tests name files relative to its
// runfiles root: the warmed clax, the test build of the web UI and the
// pinned Chromium, none of which the copy holds. They are made absolute
// here, against that root, before Playwright or a setup reads them.
import { isAbsolute, join } from "node:path";

const runfiles = process.env.JS_BINARY__RUNFILES;
if (runfiles) {
  const root = join(runfiles, process.env.JS_BINARY__WORKSPACE ?? "_main");
  for (const name of ["CLAX_TEST_BIN", "CLAX_E2E_WEB_DIST", "PLAYWRIGHT_BROWSERS_PATH"]) {
    const value = process.env[name];
    if (value && !isAbsolute(value)) process.env[name] = join(root, value);
  }
}

// CLAX_E2E_BROWSERS (`--test_env`, an absolute path) names a directory of the
// locked Playwright's builds to run instead of the pinned Chromium alone: one
// that holds its WebKit and Firefox too runs the cases written for every
// engine, which skip an engine Playwright has no build of (docs/bazel.md,
// "Browser tests").
if (process.env.CLAX_E2E_BROWSERS) process.env.PLAYWRIGHT_BROWSERS_PATH = process.env.CLAX_E2E_BROWSERS;
