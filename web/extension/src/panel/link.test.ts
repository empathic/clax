import { afterEach, describe, expect, it, vi } from "vitest";
import { FakeEvent } from "../../test/fake-chrome";
import { PanelLink } from "./link.svelte";

function fakes(active = 5) {
  const ports: { name: string; sent: unknown[]; onMessage: FakeEvent<[unknown]>; onDisconnect: FakeEvent<[]> }[] = [];
  const runtime = {
    connect: ({ name }: { name: string }) => {
      const p = { name, sent: [] as unknown[], onMessage: new FakeEvent<[unknown]>(), onDisconnect: new FakeEvent<[]>(), postMessage(m: unknown) { p.sent.push(m); }, disconnect() {} };
      ports.push(p);
      return p;
    },
  };
  const tabs = { query: async () => [{ id: active }], onActivated: new FakeEvent<[{ tabId: number; windowId: number }]>(), onUpdated: new FakeEvent<[number, { url?: string }, { active: boolean; windowId: number }]>() };
  return { ports, runtime, tabs };
}
const settle = async () => { for (let i = 0; i < 5; i++) await Promise.resolve(); };
let link: PanelLink | null = null;
afterEach(() => { link?.close(); link = null; vi.useRealTimers(); });

describe("PanelLink", () => {
  it("watches the window's active tab and follows it, saying whether the panel is visible", async () => {
    const f = fakes();
    link = new PanelLink(2, { runtime: f.runtime as never, tabs: f.tabs as never, search: "", doc: { visibilityState: "visible", addEventListener() {}, removeEventListener() {} } as never });
    await settle();
    expect(f.ports[0].name).toBe("panel:2");
    expect(f.ports[0].sent).toEqual([{ t: "watch-tab", tabId: 5 }, { t: "visible", on: true }]);
    f.tabs.query = async () => [{ id: 8 }];
    f.tabs.onActivated.fire({ tabId: 8, windowId: 9 });
    await settle();
    expect(f.ports[0].sent).toHaveLength(2);
    f.tabs.onActivated.fire({ tabId: 8, windowId: 2 });
    await settle();
    expect(f.ports[0].sent.at(-1)).toEqual({ t: "watch-tab", tabId: 8 });
  });

  it("takes only the worker's well-formed messages", async () => {
    const f = fakes();
    link = new PanelLink(2, { runtime: f.runtime as never, tabs: f.tabs as never, search: "", doc: { visibilityState: "visible", addEventListener() {}, removeEventListener() {} } as never });
    const p = f.ports[0];
    p.onMessage.fire({ t: "tab", state: { tabId: 5, error: null } });
    expect(link.state).toEqual({ tabId: 5, error: null });
    p.onMessage.fire({ t: "failed", code: "no_page", message: "No live page." });
    expect(link.state?.error).toEqual({ code: "no_page", message: "No live page." });
    p.onMessage.fire({ t: "stream-status", up: false });
    expect(link.up).toBe(false);
    p.onMessage.fire({ t: "tab", state: "<script>" });
    p.onMessage.fire({ t: "stream-status", up: "no" });
    expect(link.up).toBe(false);
    expect(link.state?.tabId).toBe(5);
  });

  it("connects again when the worker goes away, and pings while open", async () => {
    vi.useFakeTimers();
    const f = fakes();
    link = new PanelLink(2, { runtime: f.runtime as never, tabs: f.tabs as never, search: "?tab=7", doc: { visibilityState: "hidden", addEventListener() {}, removeEventListener() {} } as never });
    await vi.advanceTimersByTimeAsync(0);
    expect(f.ports[0].sent).toEqual([{ t: "watch-tab", tabId: 7 }, { t: "visible", on: false }]);
    await vi.advanceTimersByTimeAsync(20_000);
    expect(f.ports[0].sent.at(-1)).toEqual({ t: "ping" });
    f.ports[0].onDisconnect.fire();
    await vi.advanceTimersByTimeAsync(1000);
    expect(f.ports).toHaveLength(2);
    expect(f.ports[1].sent).toEqual([{ t: "watch-tab", tabId: 7 }, { t: "visible", on: false }]);
  });
});
