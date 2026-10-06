// Origins and the overlay's injection (spec 2026-10-05 O4, L8): a tab's
// origin, the origin's optional host permission (Chrome's own prompt, none
// when it is held), which lets the worker inject the overlay again after a
// reload of a tab Clax is on, and the injection itself, once per document.
// Holding the permission turns Clax on in no tab.
export type OriginsEnv = {
  permissions: Pick<typeof chrome.permissions, "request">;
  scripting: Pick<typeof chrome.scripting, "executeScript">;
  /** This load of the extension's boot nonce (`bootNonce`). */
  boot(): Promise<string>;
};

type Area = { get(k: string): Promise<Record<string, unknown>>; set(v: Record<string, unknown>): Promise<void> };
const NONCE = /^[0-9a-f]{32}$/;
/** A nonce for this load of the extension, kept in session storage: the
 * same across the worker's restarts, new after the extension is reloaded or
 * updated (which clears session storage). An overlay started under another
 * nonce is left over from an earlier load and does not count as present. */
export function bootNonce(session: Area, random: () => string = () => [...crypto.getRandomValues(new Uint8Array(16))].map(b => b.toString(16).padStart(2, "0")).join("")): () => Promise<string> {
  let p: Promise<string> | null = null;
  return () => (p ??= (async () => {
    const v = (await session.get("boot")).boot;
    if (typeof v === "string" && NONCE.test(v)) return v;
    const n = random();
    await session.set({ boot: n });
    return n;
  })().catch(e => { p = null; throw e; }));
}

/** The origin of an http or https URL; null for any other. */
export function originOf(url: string): string | null {
  try {
    const u = new URL(url);
    return u.protocol === "http:" || u.protocol === "https:" ? u.origin : null;
  } catch {
    return null;
  }
}
/** Whether `url` is an http or https URL of `senderUrl`'s origin: a page names only its own pages. */
export function sameOrigin(url: string, senderUrl: string | undefined): boolean {
  const o = originOf(url);
  return o !== null && senderUrl !== undefined && o === originOf(senderUrl);
}
export const patternOf = (origin: string) => `${origin}/*`;

/** Asks for the origin's permission. Call it before any `await` in the
 * gesture's handler, so the gesture still holds. */
export function ask(env: OriginsEnv, origin: string): Promise<boolean> {
  try {
    return env.permissions.request({ origins: [patternOf(origin)] }).catch(() => false);
  } catch {
    return Promise.resolve(false);
  }
}

/** The prefix of the loaders an earlier build registered per origin. */
const LOADER = "clax-loader-";

/** Unregisters the loaders an earlier build registered per origin, and
 * forgets its list of origins: a loader would bring Clax to every tab of
 * its origin, where Clax is now on per tab. */
export async function dropLoaders(scripting: Pick<typeof chrome.scripting, "getRegisteredContentScripts" | "unregisterContentScripts">, local: { remove(k: string): Promise<void> }): Promise<void> {
  const ids = (await scripting.getRegisteredContentScripts()).map(r => r.id).filter(id => id.startsWith(LOADER));
  if (ids.length) await scripting.unregisterContentScripts({ ids });
  await local.remove("origins");
}

/** Whether this document has a live overlay of this load of the extension
 * (content/presence.ts: the overlay's own check of its context and its boot
 * nonce). Runs in the tab's isolated world, out of the page's reach; it is
 * serialized, so it names the globals rather than importing them. */
export function hasOverlay(boot: string): boolean {
  try {
    const r = (globalThis as { claxOverlayStarted?: { alive?: unknown } }).claxOverlayStarted;
    return !!r && typeof r.alive === "function" && r.alive(boot) === true;
  } catch {
    return false;
  }
}
/** Names this load's boot nonce to the overlay about to start. */
export function setBoot(boot: string): void {
  (globalThis as { claxBoot?: string }).claxBoot = boot;
}

/** Whether the tab's current document has a live overlay; false when the
 * worker cannot reach the tab (no permission left after a navigation). */
export async function overlayPresent(env: OriginsEnv, tabId: number): Promise<boolean> {
  try {
    const [probe] = await env.scripting.executeScript({ target: { tabId, allFrames: false }, func: hasOverlay, args: [await env.boot()] });
    return probe?.result === true;
  } catch {
    return false;
  }
}

/** Injects the overlay into the tab's top frame, once per document (the
 * worker's record of a tab can lag a reload, or be lost with a restart);
 * true when it injected it now. An overlay left over from an earlier load
 * of the extension is not present, so it is injected again; the new one
 * stops the old. Presence is the overlay's own mark, set once it started,
 * so a failed injection is tried again. */
export async function injectOverlay(env: OriginsEnv, tabId: number): Promise<boolean> {
  if (await overlayPresent(env, tabId)) return false;
  const target = { tabId, allFrames: false };
  await env.scripting.executeScript({ target, func: setBoot, args: [await env.boot()] });
  await env.scripting.executeScript({ target, files: ["overlay.js"] });
  return true;
}
