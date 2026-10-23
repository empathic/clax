import { afterEach, describe, expect, it, vi } from "vitest";
import type { CapEnv } from "./host";
import { siteHandler } from "./site";
import { isAvailable } from "./availability";

const AID = "7q3k9mzx2b4t";
const env = { aid: AID } as unknown as CapEnv;
const handler = () => siteHandler(env, undefined as never);
const req = (o: Record<string, unknown>) => ({ method: "PUT", path: "/_clax/api/docs/notes/n1", headers: [["content-type", "application/json"]], body: new TextEncoder().encode("{}").buffer, ...o });

function inApp() {
  Object.defineProperty(window, "claxTransport", { value: { embed: { transport: "host", shell: "https://app.clax.invalid", content: "https://{id}.c.clax.invalid" } }, configurable: true });
}

describe("site", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    delete (window as unknown as { claxTransport?: unknown }).claxTransport;
  });

  it("is served only where the Clax app hosts the view", () => {
    expect(isAvailable("site", {}, true, {})).toBe(false);
    expect(isAvailable("site", { site: {} }, true, {})).toBe(false);
    expect(isAvailable("site", {}, false, { site: true })).toBe(true);
  });

  it("relays a page's call to its own artifact's site, with only its Content-Type and Accept, and answers its status, headers and body", async () => {
    inApp();
    const f = vi.fn(async (_u: string, _i: RequestInit) => new Response('{"ok":1}', { status: 200, headers: { "content-type": "application/json" } }));
    vi.stubGlobal("fetch", f);
    const r = (await handler().call("fetch", [req({ headers: [["content-type", "application/json"], ["accept", "*/*"], ["cookie", "x=y"], ["origin", "https://evil.example"], ["authorization", "Bearer x"]] })])) as { status: number; headers: [string, string][]; body: ArrayBuffer };
    expect(f).toHaveBeenCalledTimes(1);
    const [url, init] = f.mock.calls[0];
    expect(url).toBe(`https://${AID}.c.clax.invalid/_clax/api/docs/notes/n1`);
    expect(init.method).toBe("PUT");
    expect(init.headers).toEqual([["content-type", "application/json"], ["accept", "*/*"]]);
    expect(r.status).toBe(200);
    expect(r.headers).toContainEqual(["content-type", "application/json"]);
    expect(new TextDecoder().decode(r.body)).toBe('{"ok":1}');
  });

  it("refuses a path outside the site's API, the room, a dot segment and an unknown method, before any request", async () => {
    inApp();
    const f = vi.fn();
    vi.stubGlobal("fetch", f);
    for (const o of [{ path: "/api/artifacts" }, { path: "/_clax/api/room" }, { path: "/_clax/api/../api/x" }, { path: "https://evil.example/_clax/api/x" }, { path: "/_clax/session" }, { method: "TRACE" }]) {
      await expect(handler().call("fetch", [req(o)])).rejects.toMatchObject({ code: "invalid_argument" });
    }
    expect(f).not.toHaveBeenCalled();
  });

  it("relays nothing outside the Clax app", async () => {
    const f = vi.fn();
    vi.stubGlobal("fetch", f);
    await expect(handler().call("fetch", [req({})])).rejects.toMatchObject({ code: "capability_removed" });
    expect(f).not.toHaveBeenCalled();
  });
});
