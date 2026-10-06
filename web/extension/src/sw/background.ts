// The service worker's wiring (spec 2026-10-05 §6.4): the only holder of the
// credential. It pairs through the native host, talks to the daemon, keeps
// one event stream for every tab with Clax on, and answers the overlays,
// the composers and the side panels. Every message passes its receiver's
// validator before anything acts on it.
//
// Clax is on per tab (spec O4): the toolbar icon, the command or the
// context menu turns it on in that tab only, and the icon turns it off
// again. A tab that is on has the side panel (its own options; the panel is
// disabled everywhere else), the overlay in its top document, and its
// record in session storage, so a restarted worker picks it up again. It
// stays on through reloads and navigations within its origin, the overlay
// injected again into each new document; it turns off when the tab leaves
// the origin, when the person turns it off, or when the tab closes. Holding
// an origin's permission turns Clax on nowhere by itself. An overlay message
// the worker does not admit is answered `{off: true}`, and the overlay
// stops: one in a page restored from the back/forward cache after Clax
// turned off, or one in a document Clax was never on for.
import { type OverlayToWorker, type WorkerToOverlay, isComposerNote, isFromOverlay, isFromPanel } from "../messages";
import { captureClip, chromeCapture } from "./capture";
import * as origins from "./origins";
import { PairError } from "./pairing";
import { type PanelDeps, panelAction } from "./panel";
import { composerFrameTab, composerTab, createWorker } from "./worker";

/** The side panel's page, set as each on tab's own panel. */
export const PANEL_PATH = "sidepanel.html";
/** Each on tab's panel: the page pinned to its tab (`?tab=`), so it acts on that tab wherever the tab goes. */
export const panelPath = (tabId: number) => `${PANEL_PATH}?tab=${tabId}`;
/** The worker's answer to an overlay message it does not admit: the overlay stops. */
export const REFUSED = { off: true } as const;

export type Background = ReturnType<typeof startBackground>;

/** Registers every listener on `c` (Chrome's API, or the tests' fake) at
 * once, so a worker Chrome restarts for an event hears it. */
export function startBackground(c: typeof chrome) {
  const originsEnv: origins.OriginsEnv = { permissions: c.permissions, scripting: c.scripting, boot: origins.bootNonce(c.storage.session) };
  /** Each tab's top document whose overlay last wrote: the worker's
   * messages go to that document, never to a newer one the tab loaded meanwhile. */
  const docs = new Map<number, string>();
  // The pairing (the credential) and the tabs' record stay out of content scripts' reach (spec §10.2).
  void c.storage.session.setAccessLevel({ accessLevel: "TRUSTED_CONTEXTS" }).catch(() => {});
  // The side panel shows only in the tabs Clax is on, through each tab's own options.
  quietly(() => c.sidePanel.setOptions({ enabled: false }));
  // Loaders an earlier build registered per origin would bring Clax to every tab of the origin.
  void origins.dropLoaders(c.scripting, c.storage.local).catch(() => {});

  const toOverlay = (tabId: number, m: WorkerToOverlay) => {
    const documentId = docs.get(tabId);
    quietly(() => c.tabs.sendMessage(tabId, m, documentId ? { documentId } : { frameId: 0 }));
  };
  const { pairer, api, tabs, picks, fromOverlay } = createWorker({
    pair: {
      sendNative: async (host, msg) => {
        try { return await c.runtime.sendNativeMessage(host, msg); }
        catch (e) { throw new PairError(/not found/i.test(String(e)) ? "host_missing" : "host_failed", String(e)); }
      },
      session: c.storage.session, local: c.storage.local,
      manifestVersion: c.runtime.getManifest().version,
      // Not mid-request, and not while a pick is open: a reload tears down the
      // composer (an extension page) and the person's text with it.
      reload: () => picks.whenIdle(() => api.whenIdle(() => c.runtime.reload())),
      now: () => Date.now(),
    },
    toOverlay,
    inject: (tabId, origin) => origins.injectOverlay(originsEnv, tabId, origin),
    present: tabId => origins.overlayPresent(originsEnv, tabId),
    store: c.storage.session,
    capture: (windowId, rect, dpr) => captureClip(chromeCapture, windowId, rect, dpr),
    tabActive: async tabId => (await c.tabs.get(tabId)).active,
    windowOf: async tabId => (await c.tabs.get(tabId)).windowId,
  });

  const showPanel = (tabId: number) => quietly(() => c.sidePanel.setOptions({ tabId, path: panelPath(tabId), enabled: true }));

  /** Turns Clax off in the tab: its panel hidden, its overlay stopped, its pick and record gone. */
  function off(tabId: number): void {
    quietly(() => c.sidePanel.setOptions({ tabId, enabled: false }));
    // To the document whose overlay last wrote, and to the tab's top frame
    // now, which may hold an overlay that has not written yet.
    toOverlay(tabId, { t: "off" });
    if (docs.has(tabId)) quietly(() => c.tabs.sendMessage(tabId, { t: "off" } satisfies WorkerToOverlay, { frameId: 0 }));
    docs.delete(tabId);
    picks.close(tabId);
    tabs.close(tabId);
  }

  /** Turns Clax on in the tab (or keeps it on), records the gesture's
   * activeTab grant, and makes sure of the overlay: comment mode turns on
   * when it was just injected, else flips. */
  async function comment(tabId: number, url: string, origin: string): Promise<void> {
    await tabs.ready();
    showPanel(tabId);
    tabs.turnOn(tabId, url, origin);
    await tabs.toggle(tabId, url);
  }

  /** A gesture that grants activeTab (spec L8). The side panel (icon only)
   * and the origin's permission are asked for before any await: the
   * gesture holds only until then. The icon in a tab Clax is on turns it
   * off. A worker that has not read its tabs back yet cannot tell, and the
   * panel must be opened before any await: it opens it, and closes it again
   * once it finds the tab was on. */
  function gesture(tab: chrome.tabs.Tab, icon: boolean): void {
    const origin = tab.url ? origins.originOf(tab.url) : null;
    if (tab.id === undefined || !origin || !tab.url) return;
    const tabId = tab.id, url = tab.url;
    const known = tabs.isReady;
    if (icon && known && tabs.onOrigin(tabId) === origin) { off(tabId); return; }
    if (icon) {
      showPanel(tabId);
      quietly(() => c.sidePanel.open({ tabId }));
    }
    // The permission lets the overlay be injected again after a reload; held, Chrome asks nothing.
    const asked = origins.ask(originsEnv, origin);
    void (async () => {
      await tabs.ready();
      // A worker that had not read its tabs back at the click: the tab was on, so the icon turns it off.
      if (icon && !known && tabs.onOrigin(tabId) === origin) { off(tabId); return; }
      await comment(tabId, url, origin);
      if (!(await asked)) tabs.declined(tabId);
    })();
  }

  c.action.onClicked.addListener(tab => gesture(tab, true));
  c.commands.onCommand.addListener((cmd, tab) => { if (cmd === "comment" && tab) gesture(tab, false); });
  c.runtime.onInstalled.addListener(() => c.contextMenus.create({ id: "clax-comment", title: "Comment with Clax", contexts: ["page", "selection", "link", "image"] }));
  c.runtime.onStartup.addListener(() => {});
  c.contextMenus.onClicked.addListener((_info, tab) => { if (tab) gesture(tab, false); });
  const gone = (tabId: number) => { docs.delete(tabId); picks.close(tabId); void tabs.ready().then(() => tabs.close(tabId, true)); };
  c.tabs.onRemoved.addListener(tabId => gone(tabId));
  // A prerendered or discarded tab swapped in under a new ID: the new tab starts off.
  c.tabs.onReplaced.addListener((_added, removed) => gone(removed));
  // A tab Clax is on that comes to the front gets its panel (its options
  // outlive the worker; this mends any a restart missed).
  c.tabs.onActivated.addListener(({ tabId }) => { void tabs.ready().then(() => { if (tabs.onOrigin(tabId) !== null) showPanel(tabId); }); });
  void tabs.ready().then(() => { for (const tabId of tabs.onTabs()) showPanel(tabId); });
  // A navigation of a tab Clax is on. Another origin turns it off: the URL
  // says so where the extension may read it, and a tab whose URL it can no
  // longer read has left every origin it holds. Within the origin, a new
  // document gets the overlay again once loaded (spec §11); at `loading`
  // the old document may still answer the probe. A URL change with no load
  // (an activated prerender, a page restored from the back/forward cache,
  // or an in-page navigation, whose overlay is still there) is checked too.
  c.tabs.onUpdated.addListener((tabId, change, tab) => {
    if (change.url === undefined && change.status === undefined) return;
    void tabs.ready().then(() => {
      const on = tabs.onOrigin(tabId);
      if (on === null) return;
      const url = change.url ?? tab.url;
      if (url === undefined || origins.originOf(url) !== on) { off(tabId); return; }
      if (change.status === undefined) void tabs.navigated(tabId, true);
      else if (change.status === "loading" || change.status === "complete") void tabs.navigated(tabId, change.status === "complete");
    });
  });

  /** An overlay message comes from the top frame of a tab Clax is on, of
   * the origin it is on for (spec §9.4); a URL it names is of that origin. */
  async function admitted(sender: chrome.runtime.MessageSender, m: OverlayToWorker): Promise<boolean> {
    const tabId = sender.tab?.id;
    const origin = sender.url ? origins.originOf(sender.url) : null;
    if (tabId === undefined || !origin) return false;
    if ("url" in m && m.url !== null && !origins.sameOrigin(m.url, sender.url)) return false;
    await tabs.ready();
    return tabs.onOrigin(tabId) === origin;
  }

  c.runtime.onMessage.addListener((m, sender, reply) => {
    if (sender.id !== c.runtime.id) return false;
    // A composer frame whose port is gone (spec §11 "worker restarted").
    const composer = composerFrameTab(sender, c.runtime.id);
    if (composer !== null) {
      if (isComposerNote(m)) picks.note(composer, m);
      return false;
    }
    if (sender.tab?.id === undefined || sender.frameId !== 0 || !isFromOverlay(m)) return false;
    const tab = sender.tab;
    void (async () => {
      // Not on in this tab, or not for this origin: the overlay stops.
      if (!(await admitted(sender, m))) return REFUSED;
      if (sender.documentId) docs.set(tab.id!, sender.documentId);
      return fromOverlay(tab.id!, tab.windowId, m, sender.url);
    })().then(r => reply(r ?? null), e => reply({ error: String(e) }));
    return true;
  });

  /** What a side panel's actions reach (spec §9.4). */
  const panelDeps: PanelDeps = {
    api, tabs, pairer,
    allUrls: () => c.permissions.contains({ origins: ["<all_urls>"] }),
    navigate: async (tabId, url) => { await c.tabs.update(tabId, { url }); },
    turnOff: async tabId => off(tabId),
  };

  c.runtime.onConnect.addListener(port => {
    const s = port.sender;
    if (s?.id !== c.runtime.id) { port.disconnect(); return; }
    let page = "";
    try { page = s.url ? new URL(s.url).pathname : ""; } catch { /* no page */ }
    if (port.name.startsWith("panel:") && page === `/${PANEL_PATH}` && s.tab === undefined) {
      tabs.attachPanel(port, (tabId, m) => { if (isFromPanel(m)) void panelAction(panelDeps, tabId, m, r => { try { port.postMessage(r); } catch { /* the panel closed */ } }); });
      return;
    }
    // A composer frame: the worker then takes it only for its tab's current pick.
    const tabId = composerTab(port, c.runtime.id);
    if (tabId !== null) { picks.attachComposer(port, tabId); return; }
    port.disconnect();
  });

  return {
    pairer, tabs,
    /** What the command does in the tab (the browser tests' stand-in for a gesture, which they cannot make). */
    comment: (tabId: number, url: string) => { const o = origins.originOf(url); return o ? comment(tabId, url, o) : Promise.resolve(); },
    off,
  };
}

/** Runs a Chrome call whose failure (a closed tab, a missing frame) changes nothing. */
function quietly(f: () => unknown): void {
  try { Promise.resolve(f()).catch(() => {}); } catch { /* as above */ }
}
