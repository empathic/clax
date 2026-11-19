// The test harness (`harness.ts`): loaded only when the app's inspect view
// asks a draft's page to run its tests or an app bundle's migration, or a
// ClaxBot browser test its saved tests (`window.claxTest`).
import { type HarnessTimers, answerMigration, answerTests, runTests } from "../harness";
import type { BridgeToShell, ShellToBridge, TestResults, TestRun } from "../protocol";

/** The bridge's `clax:test` order `m`, answered on `post`; each test file is
 * imported as a module. */
export const test = (win: Window, m: Extract<ShellToBridge, { type: "clax:test" }>, post: (m: BridgeToShell) => void, clock: HarnessTimers & { now(): number }): Promise<void> =>
  answerTests(win, m, post, { timers: clock, now: () => clock.now(), load: u => import(/* @vite-ignore */ u) });

/** A browser test's run of the saved tests `o.files` (`window.claxTest.run`):
 * each a path of the artifact, imported as a module from the site's root
 * whatever page is open; files that are not a list of paths run none. */
export const run = (win: Window, o: TestRun, clock: HarnessTimers & { now(): number }): Promise<TestResults> =>
  runTests(win, Array.isArray(o?.files) ? o.files.filter(f => typeof f === "string") : [], { testMs: o?.testMs, totalMs: o?.totalMs, base: new URL("/", win.location.href).href }, { timers: clock, now: () => clock.now(), load: u => import(/* @vite-ignore */ u) });

/** The bridge's `clax:migrate` order `m`, answered on `post`: the file is
 * imported as a module and given the page's `window.claude.use("db")`. */
export const migrate = (win: Window, m: Extract<ShellToBridge, { type: "clax:migrate" }>, post: (m: BridgeToShell) => void, clock: HarnessTimers & { now(): number }): Promise<void> =>
  answerMigration(win, m, post, {
    timers: clock,
    now: () => clock.now(),
    load: u => import(/* @vite-ignore */ u),
    db: async () => {
      const claude = (win as Window & { claude?: { use?(name: string): Promise<unknown> } }).claude;
      if (typeof claude?.use !== "function") throw new Error("window.claude is missing");
      return claude.use("db");
    },
  });
