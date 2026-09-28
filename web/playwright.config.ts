import { defineConfig } from "@playwright/test";
export default defineConfig({ testDir: "e2e", timeout: 60_000, use: { browserName: "chromium" }, workers: 1, reporter: "list" });
