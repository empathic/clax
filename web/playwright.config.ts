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
// events (the typing tests' key schedules are input, not waits). A failing
// expectation fails in 5 s; a test that hangs stops at 30 s (90 s for the
// gesture tests).
const workers = Number(process.env.CLAX_E2E_WORKERS) || cpus().length + 4;
// Under Bazel a failed test's page (its error context and a screenshot) is
// kept with the test's outputs, which CI uploads with the logs of a failed run.
const outputs = process.env.TEST_UNDECLARED_OUTPUTS_DIR;

export default defineConfig({
  testDir: "e2e",
  globalSetup: "./e2e/global-setup.ts",
  fullyParallel: true,
  workers,
  timeout: 30_000,
  expect: { timeout: 5_000 },
  use: { browserName: "chromium", screenshot: "only-on-failure" },
  ...(outputs ? { outputDir: join(outputs, "test-results") } : {}),
  // Projects schedule the work: the gesture tests take up to five eighths of
  // the workers. Several wait out Chromium's 5 s user activation, which no
  // clock can shorten, so they run beside the busier tests rather than all
  // at once. The Chrome overlay's tests launch browsers of their own, each
  // with the extension in a fresh profile (e2e/extension-fixtures.ts).
  // `shots` renders the app's web pieces for a person to look at, on
  // demand only (//web:e2e_shots, e2e/app-shots.spec.ts).
  projects: [
    // A gesture test waits out Chromium's real 5 s activation, some twice,
    // so one counts as hung only after 90 s.
    { name: "gesture", testMatch: /gesture\.spec\.ts$/, workers: Math.ceil(workers * 5 / 8), timeout: 90_000 },
    { name: "chrome-overlay", testMatch: /chrome-overlay\.spec\.ts$/ },
    { name: "shots", testMatch: /app-shots\.spec\.ts$/ },
    { name: "rest", testIgnore: /(gesture|chrome-overlay|app-shots)\.spec\.ts$/ },
  ],
  reporter: "list",
});
