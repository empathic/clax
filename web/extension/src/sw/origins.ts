// Turning Clax on per origin (spec 2026-10-05 O4, L8): the origin's
// optional host permission (Chrome's own prompt, none when it is held), a
// content script registered for it that survives restarts, and the
// overlay injected into the tab at once.
export type OriginsEnv = {
  permissions: Pick<typeof chrome.permissions, "request" | "remove">;
  scripting: Pick<typeof chrome.scripting, "registerContentScripts" | "unregisterContentScripts" | "getRegisteredContentScripts" | "executeScript">;
  local: { get(k: string): Promise<Record<string, unknown>>; set(v: Record<string, unknown>): Promise<void> };
};

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

/** Registers the loader for the origin (once) and records it. */
export async function remember(env: OriginsEnv, origin: string): Promise<void> {
  const id = scriptId(origin);
  const have = await env.scripting.getRegisteredContentScripts({ ids: [id] });
  if (!have.length) {
    await env.scripting.registerContentScripts([{ id, matches: [patternOf(origin)], js: ["loader.js"], runAt: "document_idle", allFrames: false, persistAcrossSessions: true }]);
  }
  const all = await list(env);
  if (!all.includes(origin)) await env.local.set({ origins: [...all, origin] });
}

/** "Turn off on this site": the loader, the permission and the record go. */
export async function forget(env: OriginsEnv, origin: string): Promise<void> {
  await env.scripting.unregisterContentScripts({ ids: [scriptId(origin)] }).catch(() => {});
  await env.permissions.remove({ origins: [patternOf(origin)] }).catch(() => false);
  await env.local.set({ origins: (await list(env)).filter(o => o !== origin) });
}

/** Marks the document as having Clax's overlay, in the extension's
 * isolated world (out of the page's reach); true when it already had it. */
function markOverlay(): boolean {
  const g = globalThis as { claxOverlayLoaded?: boolean };
  const had = g.claxOverlayLoaded === true;
  g.claxOverlayLoaded = true;
  return had;
}

/** Injects the overlay into the tab's top frame, once per document: the
 * worker's record of a tab can lag a reload, or be lost with a restart. */
export async function injectOverlay(env: OriginsEnv, tabId: number): Promise<void> {
  const target = { tabId, allFrames: false };
  const [probe] = await env.scripting.executeScript({ target, func: markOverlay });
  if (probe?.result === true) return;
  await env.scripting.executeScript({ target, files: ["overlay.js"] });
}
