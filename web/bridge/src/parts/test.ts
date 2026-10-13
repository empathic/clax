// The test harness (`harness.ts`): loaded only when the app's inspect view
// asks a draft's page to run its tests.
import { type HarnessTimers, answerTests } from "../harness";
import type { BridgeToShell, ShellToBridge } from "../protocol";

/** The bridge's `clax:test` order `m`, answered on `post`; each test file is
 * imported as a module. */
export const test = (win: Window, m: Extract<ShellToBridge, { type: "clax:test" }>, post: (m: BridgeToShell) => void, clock: HarnessTimers & { now(): number }): Promise<void> =>
  answerTests(win, m, post, { timers: clock, now: () => clock.now(), load: u => import(/* @vite-ignore */ u) });
