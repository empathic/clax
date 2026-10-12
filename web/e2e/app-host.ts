// The Clax app's side of app mode (`/a/<id>?app=1`), as the app's WebView
// plays it: the channel the shell posts its host messages to
// (`window.ClaxHost`), the `init` with this load's nonce, and calls into
// `window.claxHost`. Every call goes in through the DevTools protocol with no
// user gesture (`quietEval`), as the app's `runJavaScript` does.
import type { Page } from "@playwright/test";
import { expect } from "./fixtures";
import { quietEval } from "./time";

export type HostMessage = { type: string; [k: string]: unknown };

/** Installs the app's channel on the shell's window before it loads: every
 * message the view posts to its host is kept, parsed, in `claxToHost`. */
export async function hostChannel(page: Page) {
  await page.addInitScript(() => {
    if (window.top !== window) return;
    (window as unknown as { claxToHost: unknown[] }).claxToHost = [];
    (window as unknown as { ClaxHost: unknown }).ClaxHost = { postMessage: (m: string) => (window as unknown as { claxToHost: unknown[] }).claxToHost.push(JSON.parse(m)) };
  });
}

/** This load's nonce, as the app makes one per load of the shell. */
export const NONCE = "e2e-nonce-0123456789abcdef";

/** What the shell posted with this load's nonce (the app drops the rest). */
export const toHost = (page: Page) => quietEval<HostMessage[]>(page, `window.claxToHost.filter(m => m.nonce === ${JSON.stringify(NONCE)}).map(({ nonce, ...m }) => m)`);

/** The app's `init`, once the shell's host module has loaded. */
export async function initHost(page: Page) {
  await expect.poll(() => quietEval<string>(page, "typeof window.claxHost")).toBe("object");
  await quietEval(page, `window.claxHost.init({ nonce: ${JSON.stringify(NONCE)} })`);
}

/** The host messages of `type` posted so far, oldest first. */
export const ofType = async (page: Page, type: string) => (await toHost(page)).filter(m => m.type === type);

/** Calls `window.claxHost.<call>` as the app does. */
export const host = (page: Page, call: string) => quietEval<unknown>(page, `window.claxHost.${call}`);
