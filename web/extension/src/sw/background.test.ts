// Clax on per tab (spec 2026-10-05 O4): the worker's wiring against a fake
// Chrome. A gesture turns Clax on in its tab only; the tab keeps it through
// reloads and navigations within its origin, and loses it when it leaves the
// origin, when the icon is clicked again, or when it closes. The side panel
// is enabled only for the tabs Clax is on.
import { beforeEach, describe, expect, it } from "vitest";
import { fakeChrome, type FakeChrome } from "../../test/fake-chrome";
import { panelPath, startBackground } from "./background";

const ORIGIN = "http://localhost:5173";
const URL1 = `${ORIGIN}/`;
const tab = (id: number, ...url: [string?]) => ({ id, url: url.length ? url[0] : URL1, windowId: 1, active: true }) as chrome.tabs.Tab;
const settle = async () => { for (let i = 0; i < 5; i++) await new Promise(r => setTimeout(r, 0)); };

let c: FakeChrome;
/** The tabs whose current document has the overlay, per fake Chrome. */
const docs = new WeakMap<FakeChrome, Set<number>>();
/** Each tab's top document's origin where it is not ORIGIN, per fake Chrome. */
const elsewhere = new WeakMap<FakeChrome, Map<number, string>>();
/** Runs while the fake injects `overlay.js` (a test's hook into the moment of injection). */
let onInject: ((tabId: number) => void) | null = null;
/** The fake's scripting runs the probe and the overlay's injection against `docs`: each tab's top document is `doc-<tabId>`. */
function withDocs(fake: FakeChrome): FakeChrome {
  const has = new Set<number>();
  const where = new Map<number, string>();
  docs.set(fake, has);
  elsewhere.set(fake, where);
  fake.scripting.executeScript = (async (inj: { target: { tabId: number }; files?: string[]; func?: { name: string } }) => {
    fake.calls.push({ api: "scripting.executeScript", args: [inj] });
    const id = inj.target.tabId;
    if (inj.files?.includes("overlay.js")) { has.add(id); onInject?.(id); }
    return [{ documentId: `doc-${id}`, result: inj.func?.name === "probeDocument" ? { present: has.has(id), origin: where.get(id) ?? ORIGIN } : undefined }];
  }) as unknown as typeof fake.scripting.executeScript;
  return fake;
}
/** The tab loads a new document: its overlay is gone. */
const reload = (tabId: number, fake = c) => docs.get(fake)!.delete(tabId);
const start = (fake: FakeChrome) => startBackground(fake as unknown as typeof chrome);
const options = (fake = c) => fake.calls.filter(x => x.api === "sidePanel.setOptions").map(x => x.args[0] as chrome.sidePanel.PanelOptions);
/** The last side panel options set for the tab. */
const panelOf = (tabId: number, fake = c) => options(fake).filter(o => o.tabId === tabId).at(-1) ?? null;
const injections = (tabId: number, fake = c) => fake.calls.filter(x => x.api === "scripting.executeScript")
  .map(x => x.args[0] as { target: { tabId: number }; files?: string[] }).filter(a => a.target.tabId === tabId && a.files?.includes("overlay.js")).length;
const toOverlay = (tabId: number, fake = c) => fake.calls.filter(x => x.api === "tabs.sendMessage" && x.args[0] === tabId).map(x => x.args[1] as { t: string });
/** A message from the overlay in the top frame of tab `tabId`, at `url`; resolves once the worker answered. */
function fromPage(tabId: number, m: unknown, url = URL1, fake = c): Promise<unknown> {
  return new Promise(resolve => {
    const sender = { id: fake.runtime.id, tab: tab(tabId, url), frameId: 0, url } as chrome.runtime.MessageSender;
    const async = fake.runtime.onMessage.fire(m, sender, resolve).some(r => r === true);
    if (!async) resolve(undefined);
  });
}
const click = (t: chrome.tabs.Tab, fake = c) => fake.action.onClicked.fire(t);
const command = (t: chrome.tabs.Tab, fake = c) => fake.commands.onCommand.fire("comment", t);
const updated = (tabId: number, change: chrome.tabs.TabChangeInfo, url: string | undefined, fake = c) => fake.tabs.onUpdated.fire(tabId, change, tab(tabId, url));

beforeEach(() => { c = withDocs(fakeChrome()); onInject = null; });

describe("Clax on per tab", () => {
  it("disables the side panel everywhere at start, and enables and opens it for the clicked tab within the gesture", async () => {
    const bg = start(c);
    expect(options()).toEqual([{ enabled: false }]);
    click(tab(1));
    // Synchronously, in the click's handler: the panel's options, then its opening, then the permission.
    const sync = c.calls.map(x => x.api).filter(a => a.startsWith("sidePanel.") || a === "permissions.request");
    expect(sync).toEqual(["sidePanel.setOptions", "sidePanel.setOptions", "sidePanel.open", "permissions.request"]);
    expect(panelOf(1)).toEqual({ tabId: 1, path: panelPath(1), enabled: true });
    expect(c.calls.find(x => x.api === "sidePanel.open")?.args[0]).toEqual({ tabId: 1 });
    await settle();
    expect(injections(1)).toBe(1);
    expect(bg.tabs.state(1)).toMatchObject({ on: ORIGIN, overlay: true, commentMode: true, active: true });
  });

  it("brings nothing to another tab of the same origin, or to a new tab, though the origin's permission is held", async () => {
    const bg = start(c);
    click(tab(1));
    await settle();
    expect(c.granted.has(`${ORIGIN}/*`)).toBe(true);
    for (const id of [2, 3]) {
      updated(id, { status: "loading", url: URL1 }, URL1);
      updated(id, { status: "complete" }, URL1);
      c.tabs.onActivated.fire({ tabId: id, windowId: 1 });
    }
    await settle();
    expect([injections(2), injections(3)]).toEqual([0, 0]);
    expect([panelOf(2), panelOf(3)]).toEqual([null, null]);
    expect(c.calls.some(x => x.api === "scripting.registerContentScripts")).toBe(false);
    // An overlay in the other tab (say, left from before) is not heard.
    await fromPage(2, { t: "comment-mode", on: true });
    expect(bg.tabs.state(2)).toBeUndefined();
    expect(bg.tabs.panelState(2).enabled).toBe(false);
    // The tab Clax is on still is.
    await fromPage(1, { t: "comment-mode", on: false });
    expect(bg.tabs.state(1)?.commentMode).toBe(false);
  });

  it("keeps Clax on through a reload and a navigation within the origin, injecting the overlay into each loaded document", async () => {
    const bg = start(c);
    click(tab(1));
    await settle();
    reload(1);
    updated(1, { status: "loading" }, URL1);
    await settle();
    expect(injections(1)).toBe(1);
    updated(1, { status: "complete" }, URL1);
    await settle();
    expect(injections(1)).toBe(2);
    reload(1);
    updated(1, { status: "loading", url: `${ORIGIN}/settings` }, `${ORIGIN}/settings`);
    updated(1, { status: "complete" }, `${ORIGIN}/settings`);
    await settle();
    expect(injections(1)).toBe(3);
    expect(bg.tabs.state(1)).toMatchObject({ on: ORIGIN, overlay: true, commentMode: false });
    expect(panelOf(1)?.enabled).toBe(true);
    await fromPage(1, { t: "comment-mode", on: true }, `${ORIGIN}/settings`);
    expect(bg.tabs.state(1)?.commentMode).toBe(true);
  });

  it("turns Clax off when the tab leaves the origin, and its panel with it", async () => {
    const bg = start(c);
    click(tab(1));
    await settle();
    updated(1, { status: "loading", url: "http://127.0.0.1:5173/" }, "http://127.0.0.1:5173/");
    await settle();
    expect(bg.tabs.state(1)).toBeUndefined();
    expect(panelOf(1)).toEqual({ tabId: 1, enabled: false });
    expect(toOverlay(1).at(-1)).toEqual({ t: "off" });
    // Back on the origin, it stays off until the person turns it on again.
    updated(1, { status: "complete", url: URL1 }, URL1);
    await settle();
    expect(injections(1)).toBe(1);
    await fromPage(1, { t: "comment-mode", on: true });
    expect(bg.tabs.state(1)).toBeUndefined();
  });

  it("keeps Clax on across the origins of its joined site that Chrome allows, and turns it off, saying why, at one it does not", async () => {
    const O2 = "http://localhost:5174";
    const bg = start(c);
    // The tab's site joins ORIGIN and O2 (spec §7.2).
    (bg.sites as unknown as { origins(o: string): string[] }).origins = () => [O2, ORIGIN];
    click(tab(1));
    await settle();
    // Chrome allows O2: Clax follows the tab there.
    c.granted.add(`${O2}/*`);
    elsewhere.get(c)!.set(1, O2);
    updated(1, { status: "loading", url: `${O2}/` }, `${O2}/`);
    await settle();
    expect(bg.tabs.state(1)?.on).toBe(O2);
    // Its loading and complete events at once: the new document gets the overlay.
    reload(1);
    elsewhere.get(c)!.set(1, O2);
    updated(1, { status: "loading", url: `${ORIGIN}/a` }, `${ORIGIN}/a`);
    elsewhere.get(c)!.delete(1);
    updated(1, { status: "complete", url: `${ORIGIN}/a` }, `${ORIGIN}/a`);
    await settle();
    expect(bg.tabs.state(1)?.on).toBe(ORIGIN);
    expect(bg.tabs.state(1)?.overlay).toBe(true);
    // Back at ORIGIN, whose permission the click asked for, it follows again.
    elsewhere.get(c)!.delete(1);
    updated(1, { status: "loading", url: URL1 }, URL1);
    await settle();
    expect(bg.tabs.state(1)?.on).toBe(ORIGIN);
    // Not allowed at O2 any more: off, and the button says why.
    c.granted.delete(`${O2}/*`);
    updated(1, { status: "loading", url: `${O2}/x` }, `${O2}/x`);
    await settle();
    expect(bg.tabs.state(1)).toBeUndefined();
    expect(panelOf(1)).toEqual({ tabId: 1, enabled: false });
    const title = c.calls.filter(x => x.api === "action.setTitle").at(-1)?.args[0] as { tabId: number; title: string };
    expect(title).toEqual({ tabId: 1, title: "Clax turned off: Chrome has not allowed it on localhost:5174. Click to turn it on there." });
  });

  it("turns Clax off when a loaded tab's URL is out of the extension's reach (another origin it holds no permission for)", async () => {
    const bg = start(c);
    click(tab(1));
    await settle();
    // An update that is no navigation (no URL, no load) changes nothing.
    updated(1, { title: "T" } as chrome.tabs.TabChangeInfo, undefined);
    await settle();
    expect(bg.tabs.state(1)?.on).toBe(ORIGIN);
    // Any navigation that hides the URL turns it off at once, before the load completes (a bounce back cannot keep it on).
    reload(1);
    updated(1, { status: "loading" }, undefined);
    await settle();
    expect(bg.tabs.state(1)).toBeUndefined();
    expect(panelOf(1)).toEqual({ tabId: 1, enabled: false });
    updated(1, { status: "complete", url: URL1 }, URL1);
    await settle();
    expect(bg.tabs.state(1)).toBeUndefined();
    expect(injections(1)).toBe(1);
  });

  it("turns Clax off at a second click of the icon; the command flips comment mode instead", async () => {
    const bg = start(c);
    click(tab(1));
    await settle();
    command(tab(1));
    await settle();
    expect(bg.tabs.state(1)).toMatchObject({ on: ORIGIN, commentMode: false });
    command(tab(1));
    await settle();
    expect(bg.tabs.state(1)?.commentMode).toBe(true);
    click(tab(1));
    expect(panelOf(1)).toEqual({ tabId: 1, enabled: false });
    await settle();
    expect(bg.tabs.state(1)).toBeUndefined();
    expect(toOverlay(1).at(-1)).toEqual({ t: "off" });
    expect(injections(1)).toBe(1);
  });

  it("turns Clax on from the command too, with the panel enabled but not opened", async () => {
    const bg = start(c);
    command(tab(4));
    await settle();
    expect(panelOf(4)).toEqual({ tabId: 4, path: panelPath(4), enabled: true });
    expect(c.calls.some(x => x.api === "sidePanel.open")).toBe(false);
    expect(bg.tabs.state(4)).toMatchObject({ on: ORIGIN, commentMode: true });
  });

  it("forgets a closed tab", async () => {
    const bg = start(c);
    click(tab(1));
    await settle();
    expect(Object.keys(c.storage.session.data.tabs as object)).toEqual(["1"]);
    c.tabs.onRemoved.fire(1);
    await settle();
    expect(bg.tabs.state(1)).toBeUndefined();
    expect(c.storage.session.data.tabs).toEqual({});
  });

  it("enables the panel only for tabs Clax is on as tabs come to the front", async () => {
    start(c);
    click(tab(1));
    await settle();
    const before = options().length;
    c.tabs.onActivated.fire({ tabId: 2, windowId: 1 });
    c.tabs.onActivated.fire({ tabId: 1, windowId: 1 });
    await settle();
    expect(options().slice(before)).toEqual([{ tabId: 1, path: panelPath(1), enabled: true }]);
  });

  it("picks the tabs Clax is on up again after the worker restarts, and nothing else", async () => {
    start(c);
    click(tab(1));
    await settle();
    const again = withDocs(fakeChrome());
    Object.assign(again.storage.session.data, structuredClone(c.storage.session.data));
    const bg = start(again);
    await settle();
    expect(panelOf(1, again)).toEqual({ tabId: 1, path: panelPath(1), enabled: true });
    expect(bg.tabs.onTabs()).toEqual([1]);
    await fromPage(1, { t: "comment-mode", on: false }, URL1, again);
    expect(bg.tabs.state(1)?.commentMode).toBe(false);
    await fromPage(2, { t: "comment-mode", on: true }, URL1, again);
    expect(bg.tabs.state(2)).toBeUndefined();
    // A reload after the restart: the overlay comes again.
    reload(1, again);
    updated(1, { status: "complete" }, URL1, again);
    await settle();
    expect(injections(1, again)).toBe(1);
  });

  it("turns Clax off at a click the restarted worker hears before it read its tabs back", async () => {
    start(c);
    click(tab(1));
    await settle();
    const again = withDocs(fakeChrome());
    Object.assign(again.storage.session.data, structuredClone(c.storage.session.data));
    const bg = start(again);
    click(tab(1), again);
    await settle();
    expect(bg.tabs.state(1)).toBeUndefined();
    expect(panelOf(1, again)).toEqual({ tabId: 1, enabled: false });
  });

  it("drops the loaders an earlier build registered per origin", async () => {
    c.scripting.getRegisteredContentScripts = (async () => [{ id: "clax-loader-6874" }]) as unknown as typeof c.scripting.getRegisteredContentScripts;
    start(c);
    await settle();
    expect(c.calls.find(x => x.api === "scripting.unregisterContentScripts")?.args[0]).toEqual({ ids: ["clax-loader-6874"] });
  });

  it("answers an overlay it does not admit with `off`, so a page restored after Clax turned off stops its overlay", async () => {
    start(c);
    click(tab(1));
    await settle();
    expect(await fromPage(1, { t: "ping" })).toBeNull();
    // Another tab of the origin, and the same tab once Clax is off in it.
    expect(await fromPage(2, { t: "ping" })).toEqual({ off: true });
    updated(1, { url: "https://elsewhere.example/" }, "https://elsewhere.example/");
    await settle();
    expect(await fromPage(1, { t: "route", url: URL1 })).toEqual({ off: true });
    // An overlay at another origin than the one Clax is on for in its tab.
    click(tab(3));
    await settle();
    expect(await fromPage(3, { t: "ping" }, "https://elsewhere.example/")).toEqual({ off: true });
  });

  it("tells both the document that last wrote and the tab's top frame to stop at turn-off", async () => {
    start(c);
    click(tab(1));
    await settle();
    const sender = { id: c.runtime.id, tab: tab(1), frameId: 0, url: URL1, documentId: "doc-old" } as chrome.runtime.MessageSender;
    await new Promise(resolve => c.runtime.onMessage.fire({ t: "ping" }, sender, resolve));
    click(tab(1));
    const offs = c.calls.filter(x => x.api === "tabs.sendMessage" && (x.args[1] as { t: string }).t === "off").map(x => x.args[2]);
    expect(offs).toEqual([{ documentId: "doc-old" }, { frameId: 0 }]);
  });

  it("injects nothing into a document of another origin, and nothing once the tab turned off during the injection", async () => {
    const bg = start(c);
    elsewhere.get(c)!.set(1, "https://elsewhere.example");
    click(tab(1));
    await settle();
    expect(injections(1)).toBe(0);
    expect(bg.tabs.state(1)).toMatchObject({ on: ORIGIN, overlay: false });
    // The tab turns off while the overlay is being injected: no record comes back for it.
    elsewhere.get(c)!.delete(1);
    onInject = id => { if (id === 1) click(tab(1)); };
    command(tab(1));
    await settle();
    expect(injections(1)).toBe(1);
    expect(bg.tabs.state(1)).toBeUndefined();
    expect(bg.tabs.onTabs()).toEqual([]);
  });

  it("injects the overlay again on a URL change that brings a document without it (an activated prerender, a restored page)", async () => {
    start(c);
    click(tab(1));
    await settle();
    // An in-page navigation: the overlay is still there.
    updated(1, { url: `${ORIGIN}/a` }, `${ORIGIN}/a`);
    await settle();
    expect(injections(1)).toBe(1);
    reload(1);
    updated(1, { url: `${ORIGIN}/b` }, `${ORIGIN}/b`);
    await settle();
    expect(injections(1)).toBe(2);
  });

  it("forgets a tab Chrome replaced under a new ID; the new tab starts off", async () => {
    const bg = start(c);
    click(tab(1));
    await settle();
    c.tabs.onReplaced.fire(9, 1);
    await settle();
    expect(bg.tabs.state(1)).toBeUndefined();
    expect(bg.tabs.state(9)).toBeUndefined();
    expect(panelOf(9)).toBeNull();
  });

  it("pins each tab's panel to its tab, wherever the tab goes", async () => {
    start(c);
    click(tab(1));
    expect(panelOf(1)?.path).toBe("sidepanel.html?tab=1");
    await settle();
  });

  it("notes on the tab that a reload will turn Clax off when the person refused the site's permission", async () => {
    const bg = start(c);
    c.permissions.request = (async () => false) as typeof c.permissions.request;
    click(tab(1));
    await settle();
    expect(bg.tabs.panelState(1)).toMatchObject({ enabled: true, declined: true });
  });

  it("ignores a gesture on a page that is not http or https", async () => {
    start(c);
    click(tab(1, "chrome://extensions/"));
    await settle();
    expect(panelOf(1)).toBeNull();
    expect(injections(1)).toBe(0);
  });

  it("adds its context menu item once however often install and update fire, with no duplicate-ID error", async () => {
    start(c);
    await Promise.all(c.runtime.onInstalled.fire());
    await Promise.all(c.runtime.onInstalled.fire());
    expect([...c.menus]).toEqual(["clax-comment"]);
    expect(c.calls.filter(x => x.api === "contextMenus.create")).toHaveLength(2);
    expect(c.menuErrors).toEqual([]);
  });
});
