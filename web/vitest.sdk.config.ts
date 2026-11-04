import { defineConfig } from "vitest/config";
import { MOUNT_TIMEOUT_MS, WAIT_MS } from "./shell/src/test/timeouts";

// The SDK package's test (`pnpm run test:sdk`, //web:sdk_package): it reads
// the built web/dist-sdk, so it runs after the build and outside the unit
// tests (vitest.config.ts leaves it out). No plugins: it imports the
// package's own bundles as an app would. Its limits are the unit tests'
// hang guards (shell/src/test/timeouts.ts).
export default defineConfig({
  test: { environment: "jsdom", testTimeout: MOUNT_TIMEOUT_MS, hookTimeout: MOUNT_TIMEOUT_MS, expect: { poll: { timeout: WAIT_MS } }, include: ["sdk/package.test.ts"] },
});
