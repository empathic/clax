// The DevTools panel's page: the shell's theme, the side panel's link to
// the worker pinned to the inspected tab (as `?tab=` pins the side panel),
// the owner's queues through the same link, the page's settings hook (and
// "Show on the page") through `inspectedWindow.eval`, and the tab's
// requests from `chrome.devtools.network`. It uses only `runtime.connect`
// and `chrome.devtools.*` of Chrome's APIs (Panel's storage and permission
// calls already fall back when absent). The port's window id is 0: the
// worker finds the tab's window itself (`windowOf`).
import { mount } from "svelte";
import "../../../shell/src/theme.css";
import { PanelLink } from "../panel/link.svelte";
import DevtoolsPanel from "./DevtoolsPanel.svelte";
import { IntegrationLink } from "./integration.svelte";
import { type HarEntry, NetworkLog } from "./network.svelte";
import { panelTriage, showExpr } from "./queues";
import { SettingsLink, chromeInspect } from "./settings.svelte";

const tabId = chrome.devtools.inspectedWindow.tabId;
const inspect = chromeInspect();
const link = new PanelLink(0, { runtime: chrome.runtime, search: `?tab=${tabId}`, doc: document });
const settings = new SettingsLink(inspect);
const integration = new IntegrationLink(inspect);
const network = new NetworkLog();
const originOf = () => inspect.eval("location.origin").then(o => (typeof o === "string" && /^https?:\/\//.test(o) ? o : null));

void originOf().then(o => {
  network.origin = o;
  // The requests made before the panel first opened, then each as it finishes.
  chrome.devtools.network.getHAR(har => { for (const e of har?.entries ?? []) network.add(e as unknown as HarEntry); });
});
chrome.devtools.network.onRequestFinished.addListener(e => network.add(e as unknown as HarEntry));
chrome.devtools.network.onNavigated.addListener(url => {
  let o: string | null = null;
  try { o = /^https?:/.test(url) ? new URL(url).origin : null; } catch { /* not a page */ }
  network.navigated(o);
  void readPage();
});

const props = $state({ shown: true, inspected: { origin: null as string | null, page: null as string | null } });
/** The inspected page's address, read again on navigation and every 2 s while the panel shows (an app's own routes move it without one). */
async function readPage(): Promise<void> {
  const href = await inspect.eval("location.href");
  let page: string | null = null;
  try { page = typeof href === "string" && /^https?:\/\//.test(href) ? href : null; } catch { /* not a page */ }
  const origin = page ? new URL(page).origin : null;
  if (page !== props.inspected.page) props.inspected = { origin, page };
}
void readPage();
setInterval(() => { if (props.shown) void readPage(); }, 2000);
const queues = panelTriage(link);
(window as Window & { claxPanelShown?: (on: boolean) => void }).claxPanelShown = on => { props.shown = on; };
mount(DevtoolsPanel, {
  target: document.getElementById("app")!,
  props: {
    link, settings, integration, network, queues,
    get shown() { return props.shown; },
    get inspected() { return props.inspected; },
    show: async (selector: string) => (await inspect.eval(showExpr(selector))) === true,
    reload: () => chrome.devtools.inspectedWindow.reload({}),
  },
});
