import { describe, expect, it, vi } from "vitest";
import { type PushEnv, keyBytes, pushState, turnOff, turnOn } from "./push";

// A made-up VAPID public key (public keys are not secrets; this one is not even a real point).
const KEY = "BPn2v0eQ2nqDp2wWv3eLqHk9YH2mxKkQ1oXc8mQh1gR8wq0Yl0E8Ff3E1a2b3c4d5e6f7g8h9i0j1k2l3m4n5o6"; // gitleaks:allow

/** A browser with a service worker and a push manager, and a daemon answering the push routes. */
function env(o: { permission?: NotificationPermission; answer?: NotificationPermission; subscribed?: boolean; ua?: string; push?: boolean; standalone?: boolean } = {}) {
  const calls: { url: string; method: string; body: unknown }[] = [];
  let sub: { endpoint: string; options: { applicationServerKey: ArrayBuffer | null }; unsubscribe: ReturnType<typeof vi.fn>; toJSON(): unknown } | null = null;
  const make = (key: ArrayBuffer | null) => ({
    endpoint: "https://web.push.apple.com/dev1",
    options: { applicationServerKey: key },
    unsubscribe: vi.fn(async () => { sub = null; return true; }),
    toJSON: () => ({ endpoint: "https://web.push.apple.com/dev1", keys: { p256dh: "p", auth: "a" } }),
  });
  if (o.subscribed) sub = make(keyBytes(KEY).buffer);
  const subscribe = vi.fn(async (opts: PushSubscriptionOptionsInit) => { sub = make((opts.applicationServerKey as Uint8Array).buffer as ArrayBuffer); return sub; });
  const reg = { pushManager: { getSubscription: async () => sub, subscribe } };
  const N = { permission: o.permission ?? "default", requestPermission: vi.fn(async () => { N.permission = o.answer ?? "granted"; return N.permission; }) };
  const e: PushEnv = {
    nav: { userAgent: o.ua ?? "Mozilla/5.0 (Macintosh)", platform: "MacIntel", maxTouchPoints: 0,
      serviceWorker: o.push === false ? undefined : { getRegistration: async () => reg, register: async () => reg, ready: Promise.resolve(reg) } } as unknown as Navigator,
    N: N as unknown as typeof Notification,
    hasPushManager: o.push !== false,
    standalone: o.standalone,
    fetch: vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
      calls.push({ url: String(url), method: init?.method ?? "GET", body: init?.body ? JSON.parse(String(init.body)) : undefined });
      if (String(url) === "/api/web-push") return new Response(JSON.stringify({ public_key: KEY, devices: 0 }));
      return new Response(JSON.stringify({ devices: 1 }), { status: init?.method === "POST" ? 201 : 200 });
    }) as typeof fetch,
  };
  return { e, calls, N, subscribe, sub: () => sub };
}

describe("Web Push on this device", () => {
  it("decodes the daemon's base64url key", () => {
    expect([...keyBytes("AQID_-8")]).toEqual([1, 2, 3, 255, 239]);
  });

  it("asks permission from the click, subscribes with the daemon's key and hands the subscription over", async () => {
    const t = env();
    expect(await pushState(t.e)).toBe("off");
    expect(await turnOn(t.e)).toBe("on");
    expect(t.N.requestPermission).toHaveBeenCalledOnce();
    expect(t.subscribe).toHaveBeenCalledWith({ userVisibleOnly: true, applicationServerKey: keyBytes(KEY) });
    expect(t.calls).toEqual([
      { url: "/api/web-push", method: "GET", body: undefined },
      { url: "/api/web-push/subscriptions", method: "POST", body: { endpoint: "https://web.push.apple.com/dev1", keys: { p256dh: "p", auth: "a" } } },
    ]);
    expect(await pushState(t.e)).toBe("on");
    // Off: the daemon forgets it and the subscription ends.
    const sub = t.sub()!;
    expect(await turnOff(t.e)).toBe("off");
    expect(t.calls.at(-1)).toEqual({ url: "/api/web-push/subscriptions", method: "DELETE", body: { endpoint: "https://web.push.apple.com/dev1" } });
    expect(sub.unsubscribe).toHaveBeenCalledOnce();
    expect(await pushState(t.e)).toBe("off");
  });

  it("subscribes nothing when permission is refused, and says so", async () => {
    const t = env({ answer: "denied" });
    expect(await turnOn(t.e)).toBe("denied");
    expect(t.subscribe).not.toHaveBeenCalled();
    expect(t.calls).toEqual([]);
    expect(await pushState(env({ permission: "denied" }).e)).toBe("denied");
  });

  it("replaces a subscription made with another key", async () => {
    const t = env({ subscribed: true });
    const old = t.sub()!;
    old.options.applicationServerKey = new Uint8Array([4, 1, 2]).buffer;
    expect(await turnOn(t.e)).toBe("on");
    expect(old.unsubscribe).toHaveBeenCalledOnce();
    expect(t.subscribe).toHaveBeenCalledOnce();
  });

  it("on an iPhone's Safari tab, says to add Clax to the Home Screen", async () => {
    const iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)";
    expect(await pushState(env({ push: false, ua: iphone, standalone: false }).e)).toBe("install");
    expect(await pushState(env({ push: false }).e)).toBe("unsupported");
    expect(await turnOn(env({ push: false }).e)).toBe("unsupported");
  });

  it("a refusal from the daemon is an error the page shows", async () => {
    const t = env();
    t.e.fetch = vi.fn(async () => new Response(JSON.stringify({ error: { code: "forbidden", message: "only the owner reads the push key" } }), { status: 403 })) as typeof fetch;
    await expect(turnOn(t.e)).rejects.toThrow("only the owner reads the push key");
  });
});
