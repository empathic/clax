// Waiting without sleeping. The shell's timing rules read its clock
// (shell/src/clock.ts), which the browser tests' build lets a test advance;
// everything else is waited on as an event or a state. Each call into the
// shell here goes through the DevTools protocol with no user gesture:
// Playwright's own evaluate runs as one, and would grant the shell the user
// activation the gesture rules judge.
import { expect, type CDPSession, type Page } from "@playwright/test";

const sessions = new WeakMap<Page, Promise<CDPSession>>();

/** A DevTools session on `page`'s main frame, made once per page. */
function session(page: Page): Promise<CDPSession> {
  let s = sessions.get(page);
  if (!s) {
    s = page.context().newCDPSession(page);
    sessions.set(page, s);
  }
  return s;
}

/** Evaluates `expression` in the shell's main frame with no user gesture. */
export async function quietEval<T>(page: Page, expression: string): Promise<T> {
  const cdp = await session(page);
  const r = await cdp.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, userGesture: false });
  if (r.exceptionDetails) throw new Error(`${expression}: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
  return r.result.value as T;
}

/** Moves the shell's clock `ms` forward, running every shell timer due by then. */
export async function advance(page: Page, ms: number): Promise<void> {
  await quietEval(page, `globalThis.claxTestClock.advance(${Number(ms)})`);
}

/** Waits until the shell has painted twice, so every task and message the
 * input before this call queued has run and its effect is on screen. */
export async function settle(page: Page): Promise<void> {
  await quietEval(page, "new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(r, 0))))");
}

/** Waits until the shell window's transient user activation has lapsed (as
 * Chromium keeps it: 5 s from the input that granted it), read without
 * granting any. */
export async function activationLapsed(page: Page): Promise<void> {
  await expect.poll(() => quietEval<boolean>(page, "navigator.userActivation.isActive"), { timeout: 10_000, intervals: [100] }).toBe(false);
}

/** The shell's clock now (`performance.now()` plus what tests advanced it). */
export async function shellNow(page: Page): Promise<number> {
  return quietEval<number>(page, "performance.now() + globalThis.claxTestClock.skew");
}
