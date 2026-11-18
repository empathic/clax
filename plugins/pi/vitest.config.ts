import { defineConfig } from "vitest/config";

// Hang guards: each ends a test that is stuck and never decides whether one
// passes. Many tests drive a real `clax serve` (test/daemon-fixture.ts),
// whose answers a loaded machine slows many times over, so a test may take
// up to HANG_MS, the Rust test kit's HANG (crates/clax-testkit/src/lib.rs).
// `expect.poll` waits up to WAIT_MS unless told otherwise, a little less, so
// a poll fails on a condition that never comes, with its own message, before
// the test's limit ends it. A hook may take longer than a test: the daemon's
// start, which the fixture bounds at 300 s, runs in one.
const HANG_MS = 120_000;
const WAIT_MS = HANG_MS - 10_000;

export default defineConfig({
  test: { include: ["test/**/*.test.ts"], testTimeout: HANG_MS, hookTimeout: 320_000, expect: { poll: { timeout: WAIT_MS } } },
});
