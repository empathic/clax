import { defineConfig } from "@playwright/test";
// One worker, never alongside the e2e run (quality_gates.sh runs the two in turn).
// Two retries absorb a sample run that a burst of machine load spoiled; the
// budgets also scale with the in-run control (usable.perf.ts).
export default defineConfig({ testDir: ".", testMatch: "*.perf.ts", timeout: 600_000, retries: 2, workers: 1, reporter: "list", use: { browserName: "chromium" } });
