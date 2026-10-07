// Origins and the overlay's injection (spec 2026-10-05 O4, L8): a tab's
// origin, the origin's optional host permission (Chrome's own prompt, none
// when it is held), which lets the worker read and script the origin's
// tabs without a gesture's activeTab grant (Chrome keeps that grant through
// reloads within the origin), and the injection itself, once per document.
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

/** How long a probe of a site's address waits for its server. */
export const PROBE_MS = 1500;
/** Origins the worker may fetch (its connect-src): others cannot be probed, and count as answering. */
const PROBED = /^http:\/\/(localhost|127\.0\.0\.1)(:\d+)?$/;
/** Whether `url`'s server answers within PROBE_MS (owner decision
 * 2026-10-06: a thread of a joined site opens on its most recently used
 * address that answers). An opaque answer of any status counts; a refused
 * connection or a timeout does not. */
export async function probeServer(url: string, fetchFn: typeof fetch = fetch): Promise<boolean> {
  const o = originOf(url);
  if (!o) return false;
  if (!PROBED.test(o)) return true;
  try {
    await fetchFn(url, { method: "HEAD", mode: "no-cors", credentials: "omit", cache: "no-store", redirect: "manual", signal: AbortSignal.timeout(PROBE_MS) });
    return true;
  } catch {
    return false;
  }
}

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

/** What the worker reads of a tab's top document before it injects: whether
 * it has a live overlay of this load of the extension (content/presence.ts:
 * the overlay's own check of its context and its boot nonce), and its
 * origin. Runs in the tab's isolated world, out of the page's reach; it is
 * serialized, so it names the globals rather than importing them. */
export function probeDocument(boot: string): { present: boolean; origin: string } {
  let present = false;
  try {
    const r = (globalThis as { claxOverlayStarted?: { alive?: unknown } }).claxOverlayStarted;
    present = !!r && typeof r.alive === "function" && r.alive(boot) === true;
  } catch {
    // No overlay of this load.
  }
  return { present, origin: location.origin };
}
/** Names this load's boot nonce to the overlay about to start. */
export function setBoot(boot: string): void {
  (globalThis as { claxBoot?: string }).claxBoot = boot;
}

type Probe = { present: boolean; origin: string; documentId: string };

/** The tab's top document, probed; throws when the worker cannot reach it. */
async function probe(env: OriginsEnv, tabId: number): Promise<Probe | null> {
  const [r] = await env.scripting.executeScript({ target: { tabId, frameIds: [0] }, func: probeDocument, args: [await env.boot()] });
  const v = r?.result as Partial<Probe> | undefined;
  return v && typeof v.present === "boolean" && typeof v.origin === "string" && typeof r.documentId === "string"
    ? { present: v.present, origin: v.origin, documentId: r.documentId } : null;
}

/** Whether the tab's current document has a live overlay; false when the
 * worker cannot reach the tab (no permission left after a navigation). */
export async function overlayPresent(env: OriginsEnv, tabId: number): Promise<boolean> {
  return (await probe(env, tabId).catch(() => null))?.present === true;
}

/** Chrome's refusal to script a document the tab no longer shows (it
 * navigated between the probe and the injection, or mid-probe). */
const GONE = /No document with id|frame was removed|Frame with ID 0/i;

/** Injects the overlay into the tab's top document, once per document (the
 * worker's record of a tab can lag a reload, or be lost with a restart),
 * and only into a document of `origin`, the one Clax is on for in the tab:
 * the document probed is the one injected (`documentIds`), so a navigation
 * meanwhile gets nothing. True when it injected it now; false when the
 * document has it; null when the tab's document is not of `origin`, or
 * when the tab moved to another document between probe and injection twice
 * running (its next load brings the overlay). An overlay left over from an
 * earlier load of the extension is not present, so it is injected again;
 * the new one stops the old. Presence is the overlay's own mark, set once
 * it started, so a failed injection is tried again. */
export async function injectOverlay(env: OriginsEnv, tabId: number, origin: string): Promise<boolean | null> {
  for (let attempt = 0; ; attempt++) {
    try {
      const p = await probe(env, tabId);
      if (!p || p.origin !== origin) return null;
      if (p.present) return false;
      const target = { tabId, documentIds: [p.documentId] };
      await env.scripting.executeScript({ target, func: setBoot, args: [await env.boot()] });
      await env.scripting.executeScript({ target, files: ["overlay.js"] });
      return true;
    } catch (e) {
      if (!GONE.test(String((e as { message?: unknown })?.message ?? e))) throw e;
      if (attempt >= 1) return null;
    }
  }
}
