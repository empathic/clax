// Clip rendering and its screenshot library: loaded when comment mode turns
// on, and for the app's inspect view's first inspection (`inspect.ts`).
import { type ErrorRing, type InspectTimers, answerInspect } from "../inspect";
import { renderViewport } from "../inspect-render";
import type { BridgeToShell, ShellToBridge } from "../protocol";

export { renderAreaClip, renderTargetClip } from "../clip";

/** The bridge's `clax:inspect` order `m`, answered on `post` with a
 * screenshot of `win`'s viewport (`inspect.ts`, `answerInspect`). */
export const inspect = (win: Window, ring: ErrorRing, m: Extract<ShellToBridge, { type: "clax:inspect" }>, post: (m: BridgeToShell, t?: Transferable[]) => void, timers: InspectTimers, Observer: typeof MutationObserver): Promise<void> =>
  answerInspect(win, ring, m, post, { timers, Observer, render: s => renderViewport(win, s) });
