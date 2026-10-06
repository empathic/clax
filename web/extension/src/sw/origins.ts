// Turning Clax on per origin (spec 2026-10-05 O4, L8): the origin's
// optional host permission (Chrome's own prompt, none when it is held), a
// content script registered for it that survives restarts, and the
// overlay injected into the tab at once.
export type OriginsEnv = {
  permissions: Pick<typeof chrome.permissions, "request" | "remove" | "contains">;
  scripting: Pick<typeof chrome.scripting, "registerContentScripts" | "unregisterContentScripts" | "getRegisteredContentScripts" | "executeScript">;
  local: { get(k: string): Promise<Record<string, unknown>>; set(v: Record<string, unknown>): Promise<void> };
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
export const scriptId = (origin: string) => `clax-loader-${[...new TextEncoder().encode(origin)].map(b => b.toString(16).padStart(2, "0")).join("")}`;

/** Asks for the origin's permission. Call it before any `await` in the
 * gesture's handler, so the gesture still holds. */
export function ask(env: OriginsEnv, origin: string): Promise<boolean> {
  try {
    return env.permissions.request({ origins: [patternOf(origin)] }).catch(() => false);
  } catch {
    return Promise.resolve(false);
  }
}

async function list(env: OriginsEnv): Promise<string[]> {
  const v = (await env.local.get("origins")).origins;
  return Array.isArray(v) ? v.filter((o): o is string => typeof o === "string") : [];
}

/** Whether Clax is on for the origin. */
export async function enabled(env: OriginsEnv, origin: string): Promise<boolean> {
  return (await list(env)).includes(origin);
}

const loader = (origin: string): chrome.scripting.RegisteredContentScript =>
  ({ id: scriptId(origin), matches: [patternOf(origin)], js: ["loader.js"], runAt: "document_idle", allFrames: false, persistAcrossSessions: true });

/** Registers the loader for the origin (once) and records it. */
export async function remember(env: OriginsEnv, origin: string): Promise<void> {
  const have = await env.scripting.getRegisteredContentScripts({ ids: [scriptId(origin)] });
  if (!have.length) await env.scripting.registerContentScripts([loader(origin)]);
  const all = await list(env);
  if (!all.includes(origin)) await env.local.set({ origins: [...all, origin] });
}

/** Registers again the loader of each origin Clax is on whose permission
 * is still held; run at each worker start. Chromium can drop registered
 * content scripts while the record and the permissions stay: the browser
 * test saw it across a restart of an extension loaded from the command
 * line, and an update or reload (`clax init` installing a newer build, the
 * worker's reload for the daemon's version) is the same kind of load. */
export async function restoreLoaders(env: OriginsEnv): Promise<void> {
  const all = await list(env);
  if (!all.length) return;
  const have = new Set((await env.scripting.getRegisteredContentScripts()).map(r => r.id));
  for (const o of all) {
    if (have.has(scriptId(o)) || !(await env.permissions.contains({ origins: [patternOf(o)] }).catch(() => false))) continue;
    // "Turn off on this site" may have forgotten it meanwhile.
    if (!(await enabled(env, o))) continue;
    // One at a time: a click's `remember` may have registered one meanwhile (a duplicate ID, refused).
    await env.scripting.registerContentScripts([loader(o)]).catch(() => {});
  }
}

/** "Turn off on this site": the loader, the permission and the record go. */
export async function forget(env: OriginsEnv, origin: string): Promise<void> {
  await env.scripting.unregisterContentScripts({ ids: [scriptId(origin)] }).catch(() => {});
  await env.permissions.remove({ origins: [patternOf(origin)] }).catch(() => false);
  await env.local.set({ origins: (await list(env)).filter(o => o !== origin) });
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
