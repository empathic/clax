// The worker's parts wired together: the pairer, the API client over it,
// the shell's stream hub with requests through the API client (so the
// stream carries the credential and pairs again like any request), the
// tabs, one hub client each, and the picks. A new pairing reconnects the
// hub: the old credential's stream cannot be changed or resumed by the new
// one.
import { Hub } from "../../../shell/src/stream-hub";
import type { OverlayToWorker, Rect, WorkerToOverlay } from "../messages";
import { Api } from "./api";
import { type PairEnv, Pairer } from "./pairing";
import { Picks } from "./picks";
import { Tabs } from "./tabs";

export type WorkerDeps = {
  pair: PairEnv;
  fetch?: typeof fetch;
  toOverlay(tabId: number, m: WorkerToOverlay): void;
  inject(tabId: number): Promise<boolean>;
  present?(tabId: number): Promise<boolean>;
  /** Where the tabs are kept across worker restarts (chrome.storage.session). */
  store?: { get(k: string): Promise<Record<string, unknown>>; set(v: Record<string, unknown>): Promise<void> };
  /** A pick's screenshot (`captureClip`). */
  capture(windowId: number, rect: Rect, dpr: number): Promise<{ png: Blob } | { error: string }>;
  /** Whether the tab is its window's active tab. */
  tabActive?(tabId: number): Promise<boolean>;
  now?(): number;
};
export type Worker = {
  pairer: Pairer; api: Api; hub: Hub; tabs: Tabs; picks: Picks;
  /** An admitted overlay message from tab `tabId` in window `windowId`; what it answers is the reply. */
  fromOverlay(tabId: number, windowId: number, m: OverlayToWorker, senderUrl?: string): Promise<unknown>;
};

/** The tab whose composer `port` is, when it may be one (spec §9.4): from
 * this extension, named `composer:…`, from its `composer.html` (under its ID
 * or its dynamic one, so the path is what is checked), framed in a tab.
 * `Picks.attachComposer` then checks the pick. */
export function composerTab(port: chrome.runtime.Port, extensionId: string): number | null {
  return port.name.startsWith("composer:") ? composerFrameTab(port.sender, extensionId) : null;
}

/** The tab whose composer frame `s` is: this extension's `composer.html`,
 * framed in a tab (`frameId > 0`); null for any other sender. */
export function composerFrameTab(s: chrome.runtime.MessageSender | undefined, extensionId: string): number | null {
  if (s?.id !== extensionId) return null;
  let u: URL;
  try { u = new URL(s.url ?? ""); } catch { return null; }
  if (u.protocol !== "chrome-extension:" || u.pathname !== "/composer.html") return null;
  const tabId = s.tab?.id;
  return tabId !== undefined && s.frameId !== undefined && s.frameId > 0 ? tabId : null;
}

export function createWorker(d: WorkerDeps): Worker {
  const pairer = new Pairer(d.pair);
  const api = new Api(pairer, d.fetch);
  let tabs: Tabs | null = null;
  const hub = new Hub({
    send: (ids, msg) => tabs?.fromHub(ids, msg),
    // The hub renews the shell's events cookie at `/api/token` after a
    // failure; the extension has no cookie, and the gateway refuses that route.
    fetch: (input, init) => (String(input) === "/api/token" ? Promise.resolve(new Response(null, { status: 204 })) : api.request(String(input), init)),
    base: "",
  });
  tabs = new Tabs({ api, hub, toOverlay: d.toOverlay, inject: d.inject, present: d.present, store: d.store });
  const t = tabs;
  api.onRepair = () => t.repaired();
  const picks = new Picks({
    api, capture: d.capture, toOverlay: d.toOverlay,
    pendingIds: tabId => t.pendingIds(tabId),
    posted: (tabId, page) => t.posted(tabId, page),
    tabActive: d.tabActive,
    now: d.now ?? (() => Date.now()),
  });
  async function fromOverlay(tabId: number, windowId: number, m: OverlayToWorker, senderUrl?: string): Promise<unknown> {
    switch (m.t) {
      // At once: the overlay hides its drawing until the screenshot is taken.
      case "capture": return picks.capture(tabId, windowId, m);
      case "cancel": picks.cancel(tabId, m.pickId); return null;
      // The tab's record first (a restarted worker looks it up again): its pending threads are read from it.
      case "pick": await t.fromOverlay(tabId, windowId, m, senderUrl); await picks.attach(tabId, m); return null;
      case "quiet": await t.fromOverlay(tabId, windowId, m, senderUrl); await picks.quiet(tabId, m); return null;
      default: return t.fromOverlay(tabId, windowId, m, senderUrl);
    }
  }
  return { pairer, api, hub, tabs: t, picks, fromOverlay };
}
