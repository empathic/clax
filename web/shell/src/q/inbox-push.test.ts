// The inbox's push controls, in the owner's browser (the token is served).
import { afterEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { flush, mount } from "../test/svelte";
import { InboxFeed, QuestionFeed } from "./feed.svelte";
import InboxPage from "./InboxPage.svelte";
import type { PushEnv } from "./push";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const settle = async () => { for (let i = 0; i < 8; i++) await new Promise(r => setTimeout(r, 0)); flush(); };

function stubOwner() {
  vi.stubGlobal("fetch", vi.fn(async (url: string) => {
    const ok = (b: unknown) => new Response(JSON.stringify(b));
    if (url === "/api/token") return ok({ token: "tok" });
    if (url === "/api/artifacts") return ok({ artifacts: [] });
    if (url === "/api/inbox?group=agent") return ok({ agents: [], unread: 0 });
    return ok({ items: [], next_cursor: null, unread: 0, total: 0 });
  }));
}

/** A browser whose push turns on and off as asked, starting `on` or not. */
function device(start: "off" | "on" | "install") {
  let subscribed = start === "on";
  const N = { permission: start === "on" ? "granted" : "default", requestPermission: vi.fn(async () => { N.permission = "granted"; return "granted"; }) };
  const reg = { pushManager: {
    getSubscription: async () => (subscribed ? { endpoint: "https://web.push.apple.com/d", options: { applicationServerKey: null }, toJSON: () => ({ endpoint: "https://web.push.apple.com/d" }), unsubscribe: async () => { subscribed = false; return true; } } : null),
    subscribe: vi.fn(async () => { subscribed = true; return { endpoint: "https://web.push.apple.com/d", options: { applicationServerKey: null }, toJSON: () => ({ endpoint: "https://web.push.apple.com/d" }) }; }),
  } };
  const calls: string[] = [];
  const env: PushEnv = {
    nav: { userAgent: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)", platform: "iPhone", maxTouchPoints: 5,
      serviceWorker: start === "install" ? undefined : { getRegistration: async () => reg, register: async () => reg, ready: Promise.resolve(reg) } } as unknown as Navigator,
    N: N as unknown as typeof Notification,
    hasPushManager: start !== "install",
    standalone: start !== "install",
    fetch: vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
      calls.push(`${init?.method ?? "GET"} ${String(url)}`);
      return new Response(JSON.stringify(String(url) === "/api/web-push" ? { public_key: "BAAA", devices: 0 } : { devices: 1 }));
    }) as typeof fetch,
  };
  return { env, N, calls };
}

const buttons = (root: Element) => [...root.querySelectorAll<HTMLButtonElement>(".ihead button")].map(b => b.textContent);

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("InboxPage push controls", () => {
  it("turns this device's notifications on from Notify me on this device, and off again", async () => {
    stubOwner();
    const d = device("off");
    const m = mount(InboxPage, { inbox: new InboxFeed(), questions: new QuestionFeed(), pushEnv: d.env });
    await settle();
    expect(buttons(m.root)).toEqual(["Notify me on this device", "Mark all read"]);
    expect(d.N.requestPermission).not.toHaveBeenCalled();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Notify me on this device")!);
    await settle();
    expect(d.N.requestPermission).toHaveBeenCalledOnce();
    expect(d.calls).toEqual(["GET /api/web-push", "POST /api/web-push/subscriptions"]);
    expect(m.root.querySelector(".ihead")!.textContent).toContain("Notifications on");
    expect(buttons(m.root)).toEqual(["Turn off notifications", "Mark all read"]);
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Turn off notifications")!);
    await settle();
    expect(d.calls.at(-1)).toBe("DELETE /api/web-push/subscriptions");
    expect(buttons(m.root)).toEqual(["Notify me on this device", "Mark all read"]);
  });

  it("in an iPhone's Safari tab, says how to get notifications instead of offering them", async () => {
    stubOwner();
    const d = device("install");
    const m = mount(InboxPage, { inbox: new InboxFeed(), questions: new QuestionFeed(), pushEnv: d.env });
    await settle();
    expect(buttons(m.root)).toEqual(["Mark all read"]);
    expect(m.root.querySelector(".quiet")!.textContent).toContain("add Clax to the Home Screen");
  });

  it("shows why turning them on failed", async () => {
    stubOwner();
    const d = device("off");
    d.env.fetch = vi.fn(async () => new Response(JSON.stringify({ error: { message: "push notifications are unavailable" } }), { status: 500 })) as typeof fetch;
    const m = mount(InboxPage, { inbox: new InboxFeed(), questions: new QuestionFeed(), pushEnv: d.env });
    await settle();
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Notify me on this device")!);
    await settle();
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("Could not turn notifications on: push notifications are unavailable");
  });
});
