import { afterEach, describe, expect, it, vi } from "vitest";
import { isAvailable } from "./availability";
import type { CapEnv } from "./host";
import type { Grants, Prompt, PromptAnswer } from "./grants";
import { netHandler } from "./net";

const A = "7q3k9mzx2b4t";
const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });
type Req = { method: string; url: string; auth: string | null; body: unknown };

/** A daemon that fetches for A once its hosts are granted. */
function setup(opts: { token?: string | null; answer?: PromptAnswer; granted?: boolean; hosts?: unknown } = {}) {
  const state = { granted: opts.granted ?? false };
  const requests: Req[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit = {}) => {
    const h = (init.headers ?? {}) as Record<string, string>;
    const method = init.method ?? "GET";
    requests.push({ method, url, auth: h.authorization ?? null, body: init.body ? JSON.parse(String(init.body)) : null });
    if (url === `/api/artifacts/${A}`) return json({ artifact: { id: A, title: "Weather" } });
    if (method === "PUT" && url === `/api/artifacts/${A}/net-grants`) {
      state.granted = true;
      return json({ declared: ["api.example.com"], grants: [], granted: true });
    }
    if (method === "POST" && url === `/api/artifacts/${A}/net:fetch`) {
      if (!state.granted) return json({ error: { code: "not_granted", message: "not allowed" } }, 403);
      return json({ url: "https://api.example.com/x", status: 200, headers: { "content-type": "text/plain" }, body: "hi" });
    }
    return json({ error: { code: "not_found", message: "no" } }, 404);
  }));
  const prompts: Prompt[] = [];
  const prompt = vi.fn(async (p: Prompt) => { prompts.push(p); return opts.answer ?? "allow"; });
  const env = {
    aid: A, version: 1, pinned: false, token: opts.token === undefined ? "tok" : opts.token,
    declared: { net: { hosts: opts.hosts ?? ["api.example.com", "cdn.example.com"] } }, prompt, post: () => {},
  } as unknown as CapEnv;
  return { h: netHandler(env, {} as Grants), state, requests, prompt, prompts };
}

const fetchCall = (url: string, init: Record<string, unknown> = {}) => ({ url, ...init });

describe("net in the shell", () => {
  afterEach(() => { vi.unstubAllGlobals(); });

  it("is served to the owner shell when declared, never to other viewers", () => {
    expect(isAvailable("net", { net: { hosts: ["api.example.com"] } }, true)).toBe(true);
    expect(isAvailable("net", { net: { hosts: ["api.example.com"] } }, false)).toBe(false);
    expect(isAvailable("net", {}, true)).toBe(false);
  });

  it("refuses an undeclared host, a URL that is not https, and malformed requests before any request", async () => {
    const { h, requests, prompt } = setup();
    for (const [arg, code] of [
      [fetchCall("https://evil.example.com/"), "not_declared"],
      [fetchCall("http://api.example.com/"), "https_only"],
      [fetchCall("api.example.com/x"), "invalid_argument"],
      [fetchCall("https://api.example.com/", { headers: { a: 1 } }), "invalid_argument"],
      [fetchCall("https://api.example.com/", { body: { a: 1 } }), "invalid_argument"],
      [fetchCall("https://api.example.com/", { method: "GE T" }), "invalid_argument"],
      [{}, "invalid_argument"],
    ] as const) {
      await expect(h.call("fetch", [arg]), JSON.stringify(arg)).rejects.toMatchObject({ code });
    }
    expect(requests).toEqual([]);
    expect(prompt).not.toHaveBeenCalled();
  });

  it("asks the owner once per view on not_granted, grants the hosts, and fetches again", async () => {
    const { h, requests, prompts } = setup();
    const res = await h.call("fetch", [fetchCall("https://api.example.com/x", { method: "post", headers: { "content-type": "application/json" }, body: "{}" })]);
    expect(res).toEqual({ url: "https://api.example.com/x", status: 200, headers: { "content-type": "text/plain" }, body: "hi" });
    expect(prompts).toHaveLength(1);
    expect(prompts[0]).toMatchObject({ kind: "capability", title: "Let “Weather” use the internet?", always: "Always for this app" });
    expect(prompts[0].body).toContain("api.example.com, cdn.example.com");
    const posts = requests.filter(r => r.url.endsWith("net:fetch"));
    expect(posts).toHaveLength(2);
    expect(posts[0]).toMatchObject({ auth: "Bearer tok", body: { url: "https://api.example.com/x", method: "POST", headers: { "content-type": "application/json" }, body: "{}" } });
    expect(requests.filter(r => r.method === "PUT" && r.url.endsWith("/net-grants")).map(r => r.body)).toEqual([{ hosts: ["api.example.com", "cdn.example.com"] }]);
    await h.call("fetch", [fetchCall("https://cdn.example.com/y")]);
    expect(prompts).toHaveLength(1);
  });

  it("shows every host it asks for, however many, and grants exactly those", async () => {
    const many = Array.from({ length: 12 }, (_, i) => `h${i}.example.com`);
    const { h, requests, prompts } = setup({ hosts: many });
    await h.call("fetch", [fetchCall("https://h0.example.com/")]);
    for (const host of many) expect(prompts[0].body).toContain(host);
    expect(prompts[0].body).not.toContain("more");
    expect(requests.find(r => r.method === "PUT")?.body).toEqual({ hosts: many });
  });

  it("a denial stands for the view without asking again; a granted app is never asked", async () => {
    const denied = setup({ answer: "deny" });
    await expect(denied.h.call("fetch", [fetchCall("https://api.example.com/")])).rejects.toMatchObject({ code: "not_granted" });
    await expect(denied.h.call("fetch", [fetchCall("https://api.example.com/")])).rejects.toMatchObject({ code: "not_granted" });
    expect(denied.prompt).toHaveBeenCalledTimes(1);
    expect(denied.requests.some(r => r.method === "PUT")).toBe(false);

    const granted = setup({ granted: true });
    await expect(granted.h.call("fetch", [fetchCall("https://api.example.com/")])).resolves.toMatchObject({ status: 200 });
    expect(granted.prompt).not.toHaveBeenCalled();
  });

  it("asks again when a grant its Allow made is revoked, while a denial still stands", async () => {
    const { h, state, prompt } = setup();
    await h.call("fetch", [fetchCall("https://api.example.com/")]);
    expect(prompt).toHaveBeenCalledTimes(1);
    state.granted = false;
    await h.call("fetch", [fetchCall("https://api.example.com/")]);
    expect(prompt).toHaveBeenCalledTimes(2);
    const denied = setup({ answer: "deny" });
    for (let i = 0; i < 3; i++) await expect(denied.h.call("fetch", [fetchCall("https://api.example.com/")])).rejects.toMatchObject({ code: "not_granted" });
    expect(denied.prompt).toHaveBeenCalledTimes(1);
  });

  it("never asks in the app's inspect view", async () => {
    const { h, prompt } = setup();
    const was = location.href;
    history.replaceState(null, "", "/a/x?app=1&inspect=1");
    try {
      await expect(h.call("fetch", [fetchCall("https://api.example.com/")])).rejects.toMatchObject({ code: "not_granted" });
    } finally {
      history.replaceState(null, "", was);
    }
    expect(prompt).not.toHaveBeenCalled();
  });

  it("passes the daemon's codes through and maps a lost session", async () => {
    const { h } = setup({ granted: true });
    vi.stubGlobal("fetch", vi.fn(async () => json({ error: { code: "blocked_address", message: "private" } }, 403)));
    await expect(h.call("fetch", [fetchCall("https://api.example.com/")])).rejects.toMatchObject({ code: "blocked_address" });
    vi.stubGlobal("fetch", vi.fn(async () => json({ error: { code: "unauthorized", message: "x" } }, 401)));
    await expect(h.call("fetch", [fetchCall("https://api.example.com/")])).rejects.toMatchObject({ code: "session_expired" });
    vi.stubGlobal("fetch", vi.fn(async () => { throw new TypeError("down"); }));
    await expect(h.call("fetch", [fetchCall("https://api.example.com/")])).rejects.toMatchObject({ code: "unavailable" });
  });
});
