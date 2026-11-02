// The DevTools panel's page: the shell's theme, the side panel's link to
// the worker pinned to the inspected tab (as `?tab=` pins the side panel),
// the page's settings hook through `inspectedWindow.eval`, and the tab's
// requests from `chrome.devtools.network`. It uses only `runtime.connect`
// and `chrome.devtools.*` of Chrome's APIs (Panel's storage and permission
// calls already fall back when absent). The port's window id is 0: the
// worker finds the tab's window itself (`windowOf`).
import { mount } from "svelte";
import "../../../shell/src/theme.css";
import { PanelLink } from "../panel/link.svelte";
import DevtoolsPanel from "./DevtoolsPanel.svelte";
import { type HarEntry, NetworkLog } from "./network.svelte";
import { SettingsLink, chromeInspect } from "./settings.svelte";

const tabId = chrome.devtools.inspectedWindow.tabId;
const inspect = chromeInspect();
const link = new PanelLink(0, { runtime: chrome.runtime, search: `?tab=${tabId}`, doc: document });
const settings = new SettingsLink(inspect);
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
});

const props = $state({ shown: true });
(window as Window & { claxPanelShown?: (on: boolean) => void }).claxPanelShown = on => { props.shown = on; };
mount(DevtoolsPanel, {
  target: document.getElementById("app")!,
  props: {
    link, settings, network,
    get shown() { return props.shown; },
    reload: () => chrome.devtools.inspectedWindow.reload({}),
  },
});
