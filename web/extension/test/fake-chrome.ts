// A fake `chrome` for the extension's unit tests: storage areas in memory,
// recorded calls for scripting, permissions, tabs and runtime, and events
// tests can fire. Only what the extension uses is here.
type Listener<A extends unknown[]> = (...a: A) => unknown;
export class FakeEvent<A extends unknown[]> {
  listeners: Listener<A>[] = [];
  addListener(l: Listener<A>) { this.listeners.push(l); }
  removeListener(l: Listener<A>) { this.listeners = this.listeners.filter(x => x !== l); }
  fire(...a: A) { return this.listeners.map(l => l(...a)); }
}
function area() {
  const data: Record<string, unknown> = {};
  return {
    data,
    async get(keys?: string | string[]) { const ks = keys === undefined ? Object.keys(data) : Array.isArray(keys) ? keys : [keys]; return Object.fromEntries(ks.filter(k => k in data).map(k => [k, structuredClone(data[k])])); },
    async set(items: Record<string, unknown>) { Object.assign(data, structuredClone(items)); },
    async remove(keys: string | string[]) { for (const k of Array.isArray(keys) ? keys : [keys]) delete data[k]; },
  };
}
export function fakeChrome() {
  const calls: { api: string; args: unknown[] }[] = [];
  const menus = new Set<string>();
  const menuErrors: string[] = [];
  const lastError: { value: { message: string } | undefined } = { value: undefined };
  const rec = (api: string, ret: unknown = undefined) => async (...args: unknown[]) => { calls.push({ api, args }); return typeof ret === "function" ? (ret as (...a: unknown[]) => unknown)(...args) : ret; };
  const granted = new Set<string>();
  const native: { reply: unknown } = { reply: { type: "error", v: 1, code: "daemon_unavailable", message: "not set" } };
  return {
    calls, granted, native, menus, menuErrors, lastError,
    runtime: { get lastError() { return lastError.value; }, id: "test-extension", getManifest: () => ({ version: "0.9.0" }), getURL: (p: string) => `chrome-extension://test-extension/${p}`,
      reload: rec("runtime.reload"), sendNativeMessage: rec("runtime.sendNativeMessage", () => native.reply),
      onMessage: new FakeEvent<[unknown, chrome.runtime.MessageSender, (r: unknown) => void]>(), onConnect: new FakeEvent<[chrome.runtime.Port]>(),
      onInstalled: new FakeEvent<[]>(), onStartup: new FakeEvent<[]>() },
    storage: { local: area(), session: { ...area(), setAccessLevel: rec("storage.session.setAccessLevel") } },
    permissions: { request: rec("permissions.request", (p: { origins: string[] }) => { p.origins.forEach(o => granted.add(o)); return true; }),
      contains: rec("permissions.contains", (p: { origins: string[] }) => p.origins.every(o => granted.has(o))),
      remove: rec("permissions.remove", (p: { origins: string[] }) => { p.origins.forEach(o => granted.delete(o)); return true; }) },
    scripting: { registerContentScripts: rec("scripting.registerContentScripts"), unregisterContentScripts: rec("scripting.unregisterContentScripts"),
      getRegisteredContentScripts: rec("scripting.getRegisteredContentScripts", []), executeScript: rec("scripting.executeScript", [{ result: undefined }]) },
    tabs: { captureVisibleTab: rec("tabs.captureVisibleTab", "data:image/png;base64,"), sendMessage: rec("tabs.sendMessage"), update: rec("tabs.update"), get: rec("tabs.get"),
      onUpdated: new FakeEvent<[number, chrome.tabs.TabChangeInfo, chrome.tabs.Tab]>(), onRemoved: new FakeEvent<[number]>(), onReplaced: new FakeEvent<[number, number]>(), onActivated: new FakeEvent<[{ tabId: number; windowId: number }]>() },
    sidePanel: { open: rec("sidePanel.open"), setOptions: rec("sidePanel.setOptions") },
    action: { onClicked: new FakeEvent<[chrome.tabs.Tab]>() },
    commands: { onCommand: new FakeEvent<[string, chrome.tabs.Tab]>() },
    contextMenus: {
      // Chrome keeps menu items across some updates, and refuses a second item with the same ID.
      create: (props: { id: string }, done?: () => void) => { calls.push({ api: "contextMenus.create", args: [props] }); if (menus.has(props.id)) menuErrors.push(`Cannot create item with duplicate id ${props.id}`); else menus.add(props.id); done?.(); return props.id; },
      removeAll: async () => { calls.push({ api: "contextMenus.removeAll", args: [] }); menus.clear(); },
      onClicked: new FakeEvent<[chrome.contextMenus.OnClickData, chrome.tabs.Tab]>() },
  };
}
export type FakeChrome = ReturnType<typeof fakeChrome>;
