// The service worker's entry: everything is wired on Chrome's own API at
// the top level (background.ts), so a worker Chrome restarts for an event
// hears it.
import { startBackground } from "./background";

const bg = startBackground(chrome);

if (__CLAX_EXT_TEST__) {
  (globalThis as unknown as { claxTest: unknown }).claxTest = {
    /** What the command does: turns Clax on in the tab (its panel enabled, the overlay injected), or flips comment mode where it is on. */
    comment: (tabId: number, url: string) => bg.comment(tabId, url),
    /** What the icon does in a tab Clax is on. */
    off: (tabId: number) => bg.off(tabId),
    state: (tabId: number) => bg.tabs.state(tabId),
    pairer: bg.pairer,
  };
}
