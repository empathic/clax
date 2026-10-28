import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";

const SRC = readFileSync(join(__dirname, "../public/_clax/sw.js"), "utf8");

/** The worker run against a fake `self`: its listeners, and what it showed and opened. */
function worker(windows: { url: string; navigate?: (u: string) => Promise<unknown> }[] = []) {
  const on: Record<string, (e: unknown) => void> = {};
  const shown: { title: string; options: NotificationOptions & { data?: { url: string } } }[] = [];
  const opened: string[] = [];
  const focused: string[] = [];
  const self = {
    addEventListener: (t: string, f: (e: unknown) => void) => { on[t] = f; },
    skipWaiting: vi.fn(),
    location: { origin: "https://phobos.tail556f5b.ts.net" },
    registration: { showNotification: async (title: string, options: NotificationOptions) => { shown.push({ title, options }); } },
    clients: {
      claim: async () => {},
      matchAll: async () => windows.map(w => ({ ...w, focus: async () => { focused.push(w.url); } })),
      openWindow: async (u: string) => { opened.push(u); },
    },
  };
  new Function("self", SRC)(self);
  const run = async (type: string, e: object) => {
    let wait: Promise<unknown> = Promise.resolve();
    on[type]({ ...e, waitUntil: (p: Promise<unknown>) => { wait = p; } });
    await wait;
  };
  return { on, shown, opened, focused, run };
}

describe("the service worker", () => {
  it("handles no fetch: it caches nothing", () => {
    expect(Object.keys(worker().on).sort()).toEqual(["activate", "install", "notificationclick", "push"]);
    expect(SRC).not.toMatch(/caches\.|addEventListener\("fetch"/);
  });

  it("shows a push's title, tagged, and keeps its shell path", async () => {
    const w = worker();
    await w.run("push", { data: { json: () => ({ title: "claude asked you a question", url: "/inbox?q=q_1", tag: "clax-inbox-q_1" }) } });
    expect(w.shown).toEqual([{ title: "claude asked you a question", options: { tag: "clax-inbox-q_1", renotify: true, icon: "/_clax/app/icon-192.png", data: { url: "/inbox?q=q_1" } } }]);
  });

  it("shows something for a push it cannot read, and never another origin's URL", async () => {
    const w = worker();
    await w.run("push", { data: { json: () => { throw new SyntaxError("x"); } } });
    await w.run("push", { data: { json: () => ({ title: "t", url: "https://evil.example/" }) } });
    await w.run("push", { data: { json: () => ({ title: "t", url: "//evil.example/" }) } });
    expect(w.shown.map(s => [s.title, s.options.data!.url])).toEqual([["Clax", "/inbox"], ["t", "/inbox"], ["t", "/inbox"]]);
  });

  it("a tap opens the item: in an open Clax window, else a new one", async () => {
    const close = vi.fn();
    const none = worker();
    await none.run("notificationclick", { notification: { close, data: { url: "/a/7q3k9mzx2b4t?thread=t1" } } });
    expect(close).toHaveBeenCalled();
    expect(none.opened).toEqual(["https://phobos.tail556f5b.ts.net/a/7q3k9mzx2b4t?thread=t1"]);

    const navigate = vi.fn(async (u: string) => ({ url: u }));
    const one = worker([{ url: "https://phobos.tail556f5b.ts.net/", navigate }]);
    await one.run("notificationclick", { notification: { close, data: { url: "/inbox?q=q_1" } } });
    expect(one.focused).toEqual(["https://phobos.tail556f5b.ts.net/"]);
    expect(navigate).toHaveBeenCalledWith("https://phobos.tail556f5b.ts.net/inbox?q=q_1");
    expect(one.opened).toEqual([]);

    // A window it may not navigate: a new one.
    const stuck = worker([{ url: "https://phobos.tail556f5b.ts.net/", navigate: async () => { throw new TypeError("not controlled"); } }]);
    await stuck.run("notificationclick", { notification: { close, data: { url: "/inbox" } } });
    expect(stuck.opened).toEqual(["https://phobos.tail556f5b.ts.net/inbox"]);
  });
});
