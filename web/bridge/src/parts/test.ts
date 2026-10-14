// The test harness (`harness.ts`): loaded only when the app's inspect view
// asks a draft's page to run its tests or an app bundle's migration.
import { type HarnessTimers, answerMigration, answerTests } from "../harness";
import type { BridgeToShell, ShellToBridge } from "../protocol";

/** The bridge's `clax:test` order `m`, answered on `post`; each test file is
 * imported as a module. */
export const test = (win: Window, m: Extract<ShellToBridge, { type: "clax:test" }>, post: (m: BridgeToShell) => void, clock: HarnessTimers & { now(): number }): Promise<void> =>
  answerTests(win, m, post, { timers: clock, now: () => clock.now(), load: u => import(/* @vite-ignore */ u) });

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
