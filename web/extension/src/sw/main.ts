// The service worker (spec 2026-10-05 §6.4): the only holder of the
// credential. It pairs through the native host, talks to the daemon, keeps
// one event stream for every tab with Clax on, and answers the overlays,
// the composers and the side panels. Every listener is registered at the
// top level, so a worker Chrome restarts for an event hears it. Every
// message passes its receiver's validator before anything acts on it.
import { isFromOverlay, isFromPanel } from "../messages";
import * as origins from "./origins";
import { PairError } from "./pairing";
import { createWorker } from "./worker";

const originsEnv: origins.OriginsEnv = { permissions: chrome.permissions, scripting: chrome.scripting, local: chrome.storage.local };
const { pairer, tabs } = createWorker({
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
});

/** Tabs a gesture granted activeTab since their last full load. */
const gestured = new Set<number>();

/** A gesture that grants activeTab (spec L8). The side panel (icon only)
 * and the origin's permission are asked for before any await. */
function gesture(tab: chrome.tabs.Tab, panel: boolean): void {
  const origin = tab.url ? origins.originOf(tab.url) : null;
  if (tab.id === undefined || !origin || !tab.url) return;
  if (panel) void chrome.sidePanel.open({ tabId: tab.id }).catch(() => {});
  const asked = origins.ask(originsEnv, origin);
  const tabId = tab.id, url = tab.url;
  gestured.add(tabId);
  void (async () => {
    if (await asked) await origins.remember(originsEnv, origin).catch(() => {});
    await tabs.toggle(tabId, url);
  })();
}

chrome.action.onClicked.addListener(tab => gesture(tab, true));
chrome.commands.onCommand.addListener((cmd, tab) => { if (cmd === "comment" && tab) gesture(tab, false); });
chrome.runtime.onInstalled.addListener(() => chrome.contextMenus.create({ id: "clax-comment", title: "Comment with Clax", contexts: ["page", "selection", "link", "image"] }));
chrome.contextMenus.onClicked.addListener((_info, tab) => { if (tab) gesture(tab, false); });
chrome.tabs.onRemoved.addListener(tabId => { gestured.delete(tabId); tabs.close(tabId); });
chrome.tabs.onUpdated.addListener((tabId, change) => { if (change.status === "loading" && change.url) gestured.delete(tabId); });

/** An overlay or loader message comes from a tab's top frame whose origin
 * Clax is on, or which a gesture granted activeTab (spec §9.4). */
async function admitted(sender: chrome.runtime.MessageSender): Promise<boolean> {
  const tabId = sender.tab?.id;
  const origin = sender.url ? origins.originOf(sender.url) : null;
  if (tabId === undefined || !origin) return false;
  return gestured.has(tabId) || origins.enabled(originsEnv, origin);
}

chrome.runtime.onMessage.addListener((m, sender, reply) => {
  if (sender.id !== chrome.runtime.id || sender.tab?.id === undefined || sender.frameId !== 0 || !isFromOverlay(m)) return false;
  const tab = sender.tab;
  void (async () => {
    if (!(await admitted(sender))) return null;
    return tabs.fromOverlay(tab.id!, tab.windowId, m);
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
  // Task 13 attaches the pick flow's composer ports; until then every other port is refused.
  port.disconnect();
});

/** A side panel's action; Task 14 fills the cases. */
async function panelAction(_tabId: number | null, _m: unknown): Promise<void> {}

if (__CLAX_EXT_TEST__) {
  (globalThis as unknown as { claxTest: unknown }).claxTest = {
    comment: (tabId: number, url: string) => tabs.toggle(tabId, url),
    state: (tabId: number) => tabs.state(tabId),
    pairer,
  };
}
