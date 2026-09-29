import { defineConfig } from "@playwright/test";
// Generous bounds so a loaded machine slows the suite rather than failing it:
// every wait polls, and these only cap how long a real failure takes to show.
export default defineConfig({ testDir: "e2e", timeout: 120_000, expect: { timeout: 20_000 }, use: { browserName: "chromium" }, workers: 1, reporter: "list" });
