// The overlay's layout test in a real Chromium (`pnpm exec playwright test
// -c overlay/layout.config.ts`): the built overlay (`dist/_clax/overlay.js`,
// `vite build -c vite.overlay.config.ts`) on a page and an instance the test
// answers itself, no daemon.
import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  testMatch: /layout\.spec\.ts$/,
  timeout: 30_000,
  expect: { timeout: 5_000 },
  use: { browserName: "chromium" },
});
