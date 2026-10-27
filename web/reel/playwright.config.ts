import "../e2e/bazel-env";
import { defineConfig } from "@playwright/test";

// The recorder's browser tests (pnpm run reel:test, //web/reel:camera_test):
// the cameras against small pages of their own, on the pinned Chromium
// (PLAYWRIGHT_BROWSERS_PATH, made absolute under Bazel by e2e/bazel-env.ts).
// One worker: a deterministic camera drives Chromium's virtual time, and
// nothing here needs a daemon. A test that hangs stops at 60 s.
export default defineConfig({
  testDir: ".",
  testMatch: "*.spec.ts",
  workers: 1,
  timeout: 60_000,
  reporter: "list",
  use: { browserName: "chromium" },
});
