import "./e2e/bazel-env";
import { cpus } from "node:os";
import { join } from "node:path";
import { defineConfig } from "@playwright/test";

// Every test runs in parallel with the others: each worker has its own daemon
// (e2e/fixtures.ts), shared by its tests, which publish their own artifacts.
// CLAX_E2E_WORKERS overrides the worker count: by default four more than the
// cores, as a test spends much of its time waiting on the browser, the
// daemon, or Chromium's user activation to lapse.
// No test sleeps to wait for something: the shell's timing rules run on a
// clock the tests advance (shell/src/clock.ts), and the rest is waited on as
// events (the typing tests' key schedules are input, not waits).
const workers = Number(process.env.CLAX_E2E_WORKERS) || cpus().length + 4;
// Under Bazel a failed test's page (its error context and a screenshot) is
// kept with the test's outputs, which CI uploads with the logs of a failed run.
const outputs = process.env.TEST_UNDECLARED_OUTPUTS_DIR;
// Hang guards, for every test and every expectation: each ends a test that
// is stuck and never decides whether one passes, so a condition that holds
// late on a loaded machine slows its test rather than failing it. Unloaded,
// the slowest tests take about 17 s: a seats test, and the gesture tests that wait out Chromium's
// real 5 s user activation twice. HANG_MS is some seven times that, the Rust
// test kit's HANG (crates/clax-testkit/src/lib.rs), and well inside Bazel's
// 300 s limit on a whole target (a medium test's, and `test:ci` in
// .bazelrc), so a stuck test is reported by Playwright, not cut off with the
// rest of the run. An expectation waits WAIT_MS, a little less, so one that
// is never met fails with its own message (what it waited for, and what it
// saw) before its test's limit ends it.
const HANG_MS = 120_000;
const WAIT_MS = HANG_MS - 10_000;

export default defineConfig({
  testDir: "e2e",
  globalSetup: "./e2e/global-setup.ts",
  fullyParallel: true,
  workers,
  timeout: HANG_MS,
  expect: { timeout: WAIT_MS },
  use: { browserName: "chromium", screenshot: "only-on-failure" },
  ...(outputs ? { outputDir: join(outputs, "test-results") } : {}),
  // Projects schedule the work: the gesture tests take up to five eighths of
  // the workers. Several wait out Chromium's 5 s user activation, which no
  // clock can shorten, so they run beside the busier tests rather than all
  // at once. The Chrome overlay's tests launch browsers of their own, each
  // with the extension in a fresh profile (e2e/extension-fixtures.ts).
  // `shots` renders the app's web pieces for a person to look at, on
  // demand only (//web:e2e_shots, e2e/app-shots.spec.ts). `queues` is the
  // report queues' and the feedback loop's end to end tests
  // (//web:e2e_queues).
  projects: [
    { name: "gesture", testMatch: /gesture\.spec\.ts$/, workers: Math.ceil(workers * 5 / 8) },
    { name: "chrome-overlay", testMatch: /chrome-overlay\.spec\.ts$/ },
    { name: "shots", testMatch: /app-shots\.spec\.ts$/ },
    // Report queues end to end: a hosted daemon, a deployer's site on
    // another origin and the overlay it embeds (e2e/queues.spec.ts), the
    // feedback loop from capture to verified (e2e/feedback-loop.spec.ts),
    // and the shell reporting problems with Clax into its own queue
    // (e2e/feedback.spec.ts).
    { name: "queues", testMatch: /(queues|feedback-loop|feedback)\.spec\.ts$/ },
    { name: "rest", testIgnore: /(gesture|chrome-overlay|app-shots|queues|feedback-loop|feedback)\.spec\.ts$/ },
  ],
  reporter: "list",
});
