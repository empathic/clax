// Waiting without sleeping. The shell's timing rules read its clock
// (shell/src/clock.ts), which the browser tests' build lets a test advance;
// everything else is waited on as an event or a state. Each call into the
// shell here goes through the DevTools protocol with no user gesture:
// Playwright's own evaluate runs as one, and would grant the shell the user
// activation the gesture rules judge.
import { expect, type CDPSession, type Page } from "@playwright/test";

/** `f`'s value once it has one: anything but null, undefined or false
 * (so a count of 0 is a value). Checked again after 10, 20 and 50 ms, then
 * every 100 ms, with no deadline of its own: only the test's hang guard
 * (its timeout) ends the wait, a little before it ends the test, with a
 * failure naming `what`. An error `f` throws ends the wait with it. */
export async function until<T>(what: string, f: () => Promise<T | null | undefined | false> | T | null | undefined | false): Promise<T> {
  let got: T | null | undefined | false = null;
  // timeout 0: until the test's own timeout.
  await expect.poll(async () => {
    got = await f();
    return got !== null && got !== undefined && got !== false;
  }, { message: `waiting for ${what}`, timeout: 0, intervals: [10, 20, 50, 100] }).toBe(true);
  return got as T;
}

/** The fence an absence check stands behind. Waiting a while and seeing
 * nothing proves nothing on a loaded machine; instead `step` does something
 * later through the same channel as what must not happen (another input to
 * the same frame, an event of the test's own on the same stream, a read
 * after the write), and `seen` is waited on ([`until`]) until its effect
 * shows. What came before `step` on that channel has been handled by then,
 * so the caller asserts the absence after this returns. */
export async function fence(what: string, step: () => Promise<unknown>, seen: () => Promise<unknown> | unknown): Promise<void> {
  await step();
  await until(`the fence: ${what}`, async () => (await seen()) ? true : null);
}

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
 * granting any. It lapses on Chromium's own clock, which no test can
 * advance, and a loaded machine can report it well past 5 s: the test's
 * timeout is the only bound. */
export async function activationLapsed(page: Page): Promise<void> {
  await until("the shell's user activation to lapse", async () => !(await quietEval<boolean>(page, "navigator.userActivation.isActive")));
}

/** Stops time in every frame of `page`'s context, until the test moves it
 * (`page.clock.runFor`) or lets it run again (`page.clock.resume()`):
 * Playwright's clock, installed (`page.clock.install()`) before the pages
 * load, so the timers they start are its own. It reaches the main world of
 * each frame of each page, the artifact's cross-origin frame too, each frame
 * on a clock of its own moved in step; it does not reach workers (the
 * shell's stream hub, a page's own workers, an extension's service worker).
 * While it stands still no timer runs: not a frame's animation frames
 * either, so the shell does not boot paused, and a page loaded meanwhile
 * starts paused. Each of its calls evaluates in every frame as Playwright's
 * evaluate does, with a user gesture. No frame's clock may stop at a time
 * it has passed, so the stop is set a little ahead of the page's clock,
 * further on each try that some frame had passed it. */
export async function pauseTime(page: Page): Promise<void> {
  for (let ahead = 20; ; ahead *= 2) {
    const at = await quietEval<number>(page, "Date.now()");
    try {
      await page.clock.pauseAt(at + ahead);
      return;
    } catch (e) {
      if (!String(e).includes("Cannot fast-forward to the past")) throw e;
    }
  }
}

/** The shell's clock now (`performance.now()` plus what tests advanced it). */
export async function shellNow(page: Page): Promise<number> {
  return quietEval<number>(page, "performance.now() + globalThis.claxTestClock.skew");
}
