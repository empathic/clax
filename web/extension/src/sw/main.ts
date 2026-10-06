// The service worker (spec 2026-10-05 §6.4): the only holder of the
// credential. It pairs through the native host, talks to the daemon, keeps
// one event stream for every tab with Clax on, and answers the overlays,
// the composers and the side panels. Every listener is registered at the
// top level, so a worker Chrome restarts for an event hears it. Every
// message passes its receiver's validator before anything acts on it.
import { type OverlayToWorker, isFromOverlay, isFromPanel } from "../messages";
import { captureClip, chromeCapture } from "./capture";
import * as origins from "./origins";
import { PairError } from "./pairing";
import { composerTab, createWorker } from "./worker";

const originsEnv: origins.OriginsEnv = { permissions: chrome.permissions, scripting: chrome.scripting, local: chrome.storage.local };
// The pairing (the credential) and the tabs' record stay out of content scripts' reach (spec §10.2).
void chrome.storage.session.setAccessLevel({ accessLevel: "TRUSTED_CONTEXTS" }).catch(() => {});
const { pairer, tabs, picks, fromOverlay } = createWorker({
  pair: {
    sendNative: async (host, msg) => {
      try { return await chrome.runtime.sendNativeMessage(host, msg); }
      catch (e) { throw new PairError(/not found/i.test(String(e)) ? "host_missing" : "host_failed", String(e)); }
    },
    session: chrome.storage.session, local: chrome.storage.local,
    manifestVersion: chrome.runtime.getManifest().version,
    reload: () => chrome.runtime.reload(),
    now: () => Date.now(),
  },
  toOverlay: (tabId, m) => void chrome.tabs.sendMessage(tabId, m, { frameId: 0 }).catch(() => {}),
  inject: tabId => origins.injectOverlay(originsEnv, tabId),
  present: tabId => origins.overlayPresent(originsEnv, tabId),
  store: chrome.storage.session,
  capture: (windowId, rect, dpr) => captureClip(chromeCapture, windowId, rect, dpr),
});

/** A gesture that grants activeTab (spec L8). The side panel (icon only)
 * and the origin's permission are asked for before any await. */
function gesture(tab: chrome.tabs.Tab, panel: boolean): void {
  const origin = tab.url ? origins.originOf(tab.url) : null;
  if (tab.id === undefined || !origin || !tab.url) return;
  if (panel) void chrome.sidePanel.open({ tabId: tab.id }).catch(() => {});
  const asked = origins.ask(originsEnv, origin);
  const tabId = tab.id, url = tab.url;
  void (async () => {
    await tabs.ready();
    tabs.activate(tabId, url);
    let failed: unknown = null;
    if (await asked) await origins.remember(originsEnv, origin).catch(e => { failed = e; });
    await tabs.toggle(tabId, url);
    if (failed) tabs.fail(tabId, failed);
  })();
}

chrome.action.onClicked.addListener(tab => gesture(tab, true));
chrome.commands.onCommand.addListener((cmd, tab) => { if (cmd === "comment" && tab) gesture(tab, false); });
chrome.runtime.onInstalled.addListener(() => chrome.contextMenus.create({ id: "clax-comment", title: "Comment with Clax", contexts: ["page", "selection", "link", "image"] }));
chrome.contextMenus.onClicked.addListener((_info, tab) => { if (tab) gesture(tab, false); });
chrome.tabs.onRemoved.addListener(tabId => { picks.close(tabId); void tabs.ready().then(() => tabs.close(tabId)); });
// Possibly a new document: if its overlay is gone, so is comment mode (spec §11).
chrome.tabs.onUpdated.addListener((tabId, change) => { if (change.status === "loading") void tabs.ready().then(() => tabs.navigated(tabId)); });

/** An overlay or loader message comes from a tab's top frame whose origin
 * Clax is on, or which a gesture granted activeTab (spec §9.4); a URL it
 * names is of its own origin. */
async function admitted(sender: chrome.runtime.MessageSender, m: OverlayToWorker): Promise<boolean> {
  const tabId = sender.tab?.id;
  const origin = sender.url ? origins.originOf(sender.url) : null;
  if (tabId === undefined || !origin) return false;
  if ("url" in m && !origins.sameOrigin(m.url, sender.url)) return false;
  await tabs.ready();
  return tabs.admits(tabId) || origins.enabled(originsEnv, origin);
}

chrome.runtime.onMessage.addListener((m, sender, reply) => {
  if (sender.id !== chrome.runtime.id || sender.tab?.id === undefined || sender.frameId !== 0 || !isFromOverlay(m)) return false;
  const tab = sender.tab;
  void (async () => {
    if (!(await admitted(sender, m))) return null;
    return fromOverlay(tab.id!, tab.windowId, m, sender.url);
  })().then(r => reply(r ?? null), e => reply({ error: String(e) }));
  return true;
});

chrome.runtime.onConnect.addListener(port => {
  const s = port.sender;
  if (s?.id !== chrome.runtime.id) { port.disconnect(); return; }
  let page = "";
  try { page = s.url ? new URL(s.url).pathname : ""; } catch { /* no page */ }
  if (port.name.startsWith("panel:") && page === "/sidepanel.html" && s.tab === undefined) {
    tabs.attachPanel(port, (tabId, m) => { if (isFromPanel(m)) void panelAction(tabId, m); });
    return;
  }
  // A composer frame: the worker then takes it only for its tab's current pick.
  const tabId = composerTab(port, chrome.runtime.id);
  if (tabId !== null) { picks.attachComposer(port, tabId); return; }
  port.disconnect();
});

/** A side panel's action; Task 14 fills the cases. */
async function panelAction(_tabId: number | null, _m: unknown): Promise<void> {}

if (__CLAX_EXT_TEST__) {
  (globalThis as unknown as { claxTest: unknown }).claxTest = {
    comment: (tabId: number, url: string) => tabs.toggle(tabId, url),
    /** What a toolbar click records besides activeTab, which the browser tests' build holds through `<all_urls>`. */
    activate: (tabId: number, url: string) => tabs.activate(tabId, url),
    state: (tabId: number) => tabs.state(tabId),
    pairer,
  };
}
