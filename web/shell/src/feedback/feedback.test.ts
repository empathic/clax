import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Component } from "svelte";
import type { OverlayHandle, OverlayOptions } from "../../../overlay/overlay";
import type { ClaxGlobal, FeedbackConfig } from "./feedback";

type Call = { url: string; method: string; body: unknown; credentials?: RequestCredentials };

const CONFIG = {
  enabled: true,
  instance: "https://clax.dev/",
  queue: "q_feedback",
  context: {
    release: { version: "0.6.0", commit: "abc123", id: "b42", channel: "prod" },
    flags: { claxbot: true, claxbot_unenforced: false, feedback: true },
    app: { repo: "https://github.com/empathic/clax" },
  },
};
const OFF = { enabled: false, instance: null, queue: null, context: null };

/** Stubs `fetch` with `routes` (`"METHOD path"` → body, or [status, body]); unknown routes answer 404. */
function stubFetch(routes: Record<string, unknown>): Call[] {
  const calls: Call[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    calls.push({ url, method, body: init?.body ? JSON.parse(String(init.body)) : undefined, credentials: init?.credentials });
    const r = routes[`${method} ${url}`];
    if (r === undefined) return new Response(JSON.stringify({ error: { code: "not_found" } }), { status: 404 });
    if (Array.isArray(r) && typeof r[0] === "number") return new Response(JSON.stringify(r[1]), { status: r[0] });
    return new Response(JSON.stringify(r));
  }));
  return calls;
}

/** A stand-in for the overlay script's global: records what the shell asks of it. */
function fakeClax() {
  const order: string[] = [];
  let opts: OverlayOptions | null = null;
  let badge: ((n: number) => void) | null = null;
  const handle = {
    report: vi.fn(),
    showReports: vi.fn(),
    close: vi.fn(),
    on: vi.fn((event: string, fn: (n: number) => void) => { if (event === "badge") badge = fn; return () => { badge = null; }; }),
  };
  const Clax = {
    CONTEXT_TIMEOUT_MS: 2000,
    diagnostics: { install: vi.fn(() => { order.push("diagnostics"); return { lines: () => [], count: () => 0, stop() {} }; }) },
    overlay: vi.fn((o: OverlayOptions) => { order.push("overlay"); opts = o; return handle as unknown as OverlayHandle; }),
  } as unknown as ClaxGlobal;
  return { Clax, handle, order, opts: () => opts!, badge: (n: number) => badge?.(n) };
}

// Each test loads the shell's modules afresh (`vi.resetModules`), Svelte's
// runtime and the test mount helper with them, so a component and its mount share one runtime.
const svelteKit = () => import("../test/svelte");
const settle = async () => { for (let i = 0; i < 8; i++) await new Promise(r => setTimeout(r, 0)); (await svelteKit()).flush(); };
/** What each test started, ended after it (the shortcut listens on the window). */
const running: { close(): void }[] = [];
/** The entry point, the headers' slot and the running state, as one. */
const shell = async () => ({ ...(await import("./boot")), ...(await import("./slot.svelte")), ...(await import("./state.svelte")) });
/** `shell()` with the integration running once `bootFeedback` has started it. */
async function booted(where?: () => { view: string; artifact: string | null }) {
  const s = await shell();
  s.bootFeedback(where);
  await vi.waitFor(() => { expect(s.feedbackMenu.current).not.toBeNull(); });
  running.push(s.feedback.current!);
  return s;
}
const key = (init: KeyboardEventInit, target: EventTarget = document.body) => {
  const e = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  target.dispatchEvent(e);
  return e;
};

beforeEach(() => { vi.resetModules(); });
afterEach(() => {
  for (const r of running.splice(0)) r.close();
  vi.unstubAllGlobals();
  delete (window as { Clax?: unknown }).Clax;
  document.head.querySelectorAll("script").forEach(s => s.remove());
  document.body.replaceChildren();
});

describe("the shell's feedback", () => {
  it("no menu entry when disabled: one config request, no overlay, no shortcut", async () => {
    const calls = stubFetch({ "GET /_app/feedback/config": OFF });
    const fake = fakeClax();
    (window as { Clax?: unknown }).Clax = fake.Clax;
    const s = await shell();
    s.bootFeedback();
    await settle();
    expect(calls.map(c => `${c.method} ${c.url}`)).toEqual(["GET /_app/feedback/config"]);
    expect(s.feedbackMenu.current).toBeNull();
    expect(s.feedback.current).toBeNull();
    expect(s.feedbackKeyRows()).toEqual([]);
    expect(fake.Clax.overlay).not.toHaveBeenCalled();
    expect(fake.Clax.diagnostics.install).not.toHaveBeenCalled();
    expect(document.head.querySelector("script")).toBeNull();
    expect(key({ key: "B", ctrlKey: true, shiftKey: true }).defaultPrevented).toBe(false);
  });

  it("is off when the config route fails or answers without a queue", async () => {
    const { loadConfig } = await import("./boot");
    stubFetch({ "GET /_app/feedback/config": [401, { error: { code: "unauthorized" } }] });
    expect(await loadConfig()).toBeNull();
    stubFetch({ "GET /_app/feedback/config": { ...CONFIG, queue: null } });
    expect(await loadConfig()).toBeNull();
    stubFetch({});
    expect(await loadConfig()).toBeNull();
    vi.stubGlobal("fetch", vi.fn(async () => { throw new TypeError("offline"); }));
    expect(await loadConfig()).toBeNull();
  });

  it("loads the overlay from the instance's /_clax/overlay.js when the page has none", async () => {
    const { loadOverlay } = await import("./feedback");
    const p = loadOverlay(document, "https://clax.dev");
    const s = document.head.querySelector("script")!;
    expect(s.src).toBe("https://clax.dev/_clax/overlay.js");
    expect(s.hasAttribute("data-clax-ignore")).toBe(true);
    const fake = fakeClax();
    (window as { Clax?: unknown }).Clax = fake.Clax;
    s.dispatchEvent(new Event("load"));
    expect(await p).toBe(fake.Clax);
  });

  it("starts diagnostics before the overlay, with no launcher, the token route as auth and the config's context", async () => {
    const calls = stubFetch({ "GET /_app/feedback/config": CONFIG, "POST /_app/feedback/token": { token: "cxr_abc", expires_at: "2026-10-29T12:00:00Z" } });
    const fake = fakeClax();
    (window as { Clax?: unknown }).Clax = fake.Clax;
    const s = await booted(() => ({ view: "artifact", artifact: "a_123" }));
    expect(fake.order).toEqual(["diagnostics", "overlay"]);
    const o = fake.opts();
    expect(o).toMatchObject({ instance: "https://clax.dev", queue: "q_feedback", launcher: false });
    // No token is asked for until the overlay needs one.
    expect(calls.map(c => c.url)).toEqual(["/_app/feedback/config"]);
    expect(await o.auth("PUBKEY")).toEqual({ token: "cxr_abc", expires_at: "2026-10-29T12:00:00Z" });
    expect(calls[1]).toEqual({ url: "/_app/feedback/token", method: "POST", body: { public_key: "PUBKEY" }, credentials: "same-origin" });
    expect(s.feedbackKeyRows()).toEqual([{ keys: [expect.any(String), "Shift", "B"], what: "Report a problem with Clax" }]);
  });

  it("a refused token fails the overlay's auth with the route's code", async () => {
    const { tokenAuth } = await import("./feedback");
    stubFetch({ "POST /_app/feedback/token": [403, { error: { code: "no_flag", message: "…" } }] });
    await expect(tokenAuth()("PUBKEY")).rejects.toThrow("no_flag");
  });

  it("context carries the config's release and flags, and the view at capture", async () => {
    const { hostContext, viewOf } = await import("./feedback");
    let where = viewOf("/");
    const context = hostContext({ ...CONFIG, instance: "https://clax.dev" } as FeedbackConfig, () => where);
    expect(context()).toEqual({
      release: { version: "0.6.0", commit: "abc123", id: "b42", channel: "prod" },
      flags: { claxbot: true, claxbot_unenforced: false, feedback: true },
      app: { repo: "https://github.com/empathic/clax", view: "gallery", artifact: null },
    });
    where = viewOf("/a/a_9x/v/3/index.html");
    expect(context().app).toEqual({ repo: "https://github.com/empathic/clax", view: "artifact", artifact: "a_9x" });
    expect(viewOf("/queues/q1/t2")).toEqual({ view: "queues", artifact: null });
    expect(viewOf("/me")).toEqual({ view: "me", artifact: null });
  });

  it("the shortcut starts a report, but not typed into a field, with another modifier, or repeated", async () => {
    const { Feedback } = await import("./feedback");
    const fake = fakeClax();
    const handle = fake.Clax.overlay({} as OverlayOptions);
    const fb = new Feedback(handle, window, false);
    running.push(fb);
    expect(fb.shortcut.label).toBe("Ctrl+Shift+B");
    expect(key({ key: "B", ctrlKey: true, shiftKey: true }).defaultPrevented).toBe(true);
    expect(fake.handle.report).toHaveBeenCalledTimes(1);
    const input = document.body.appendChild(document.createElement("input"));
    const area = document.body.appendChild(document.createElement("textarea"));
    const editable = document.body.appendChild(document.createElement("div"));
    editable.setAttribute("contenteditable", "true");
    const inner = editable.appendChild(document.createElement("span"));
    for (const t of [input, area, editable, inner]) key({ key: "B", ctrlKey: true, shiftKey: true }, t);
    key({ key: "B", metaKey: true, shiftKey: true });
    key({ key: "B", ctrlKey: true, shiftKey: true, altKey: true });
    key({ key: "b", ctrlKey: true });
    key({ key: "B", ctrlKey: true, shiftKey: true, repeat: true });
    expect(fake.handle.report).toHaveBeenCalledTimes(1);
    fb.close();
    key({ key: "B", ctrlKey: true, shiftKey: true });
    expect(fake.handle.report).toHaveBeenCalledTimes(1);
    expect(fake.handle.close).toHaveBeenCalled();

    const mac = new Feedback(fake.Clax.overlay({} as OverlayOptions), window, true);
    running.push(mac);
    expect(mac.shortcut).toEqual({ keys: ["⌘", "Shift", "B"], label: "⌘⇧B", aria: "Meta+Shift+B" });
    key({ key: "B", ctrlKey: true, shiftKey: true });
    expect(fake.handle.report).toHaveBeenCalledTimes(1);
    key({ key: "B", metaKey: true, shiftKey: true });
    expect(fake.handle.report).toHaveBeenCalledTimes(2);
    mac.close();
  });

  it("menu entry calls report(), shows its shortcut, and My reports carries the badge", async () => {
    stubFetch({ "GET /_app/feedback/config": CONFIG });
    const fake = fakeClax();
    (window as { Clax?: unknown }).Clax = fake.Clax;
    const s = await booted();
    const { flush, mount } = await svelteKit();
    const m = mount(s.feedbackMenu.current as Component<Record<string, never>>, {});
    const btn = m.root.querySelector<HTMLButtonElement>("button.fb-btn")!;
    expect(btn.getAttribute("aria-label")).toBe("Feedback");
    expect(m.root.querySelector(".fb-count")).toBeNull();

    flush(() => fake.badge(2));
    expect(btn.getAttribute("aria-label")).toBe("Feedback: 2 reports wait on you");
    expect(btn.querySelector(".fb-count")?.textContent).toBe("2");

    btn.click();
    await settle();
    const items = [...m.root.querySelectorAll<HTMLButtonElement>("[role=menuitem]")];
    expect(items.map(i => i.textContent?.replace(/\s+/g, " ").trim())).toEqual([`Report a problem${s.feedback.current!.shortcut.label}`, "My reports2"]);
    expect(items[0].getAttribute("aria-label")).toBe("Report a problem");
    expect(items[0].getAttribute("aria-keyshortcuts")).toBe(s.feedback.current!.shortcut.aria);
    expect(items[1].getAttribute("aria-label")).toBe("My reports: 2 reports wait on you");
    expect(document.activeElement).toBe(items[0]);
    items[0].click();
    flush();
    expect(fake.handle.report).toHaveBeenCalledTimes(1);
    expect(m.root.querySelector("[role=menu]")).toBeNull();

    btn.click();
    await settle();
    m.root.querySelectorAll<HTMLButtonElement>("[role=menuitem]")[1].click();
    flush();
    expect(fake.handle.showReports).toHaveBeenCalledTimes(1);

    flush(() => fake.badge(1));
    expect(btn.getAttribute("aria-label")).toBe("Feedback: 1 report waits on you");
    flush(() => fake.badge(0));
    expect(m.root.querySelector(".fb-count")).toBeNull();
  });
});
