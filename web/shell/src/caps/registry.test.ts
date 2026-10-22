import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { deviceLink as DeviceLink } from "./device-link";
import type { CapEnv, HandlerFactory } from "./host";

// The viewer's gesture (gesture.ts's own checks are gesture.test.ts's), and a
// gate that holds each lazy chunk's load until the test opens it.
const at = {
  verdict: "ok" as "ok" | "no_gesture" | "shell_input_recent",
  open: () => {},
  unwatch: [] as (() => void)[],
};

const DOC = "<!doctype html><html><body>v2</body></html>";

function env() {
  return {
    aid: "7q3k9mzx2b4t", version: 3, pinned: false, token: "tok",
    reload: vi.fn(), post: vi.fn(), ownPublish: { active: 0, settled: vi.fn() },
  } as unknown as CapEnv;
}

function stubDaemon() {
  const fetched = vi.fn(async (_url: string, init: RequestInit = {}) => init.method === "POST"
    ? new Response(JSON.stringify({ version: { n: 4 } }), { status: 201 })
    : new Response(JSON.stringify({ artifact: { id: "7q3k9mzx2b4t", capabilities: { artifact: {} }, current_version: 3 }, versions: [{ n: 3, files: {} }] })));
  vi.stubGlobal("fetch", fetched);
  return fetched;
}

/** The registry, fresh, so each test's first call loads its chunk again,
 * through a gate the test opens. */
async function registry(): Promise<{ REGISTRY: Record<string, HandlerFactory>; link: typeof DeviceLink }> {
  vi.resetModules();
  const gate = new Promise<void>(r => { at.open = r; });
  vi.doMock("./gesture", async orig => ({
    ...(await orig<typeof import("./gesture")>()),
    frameGesture: () => at.verdict !== "no_gesture",
    frameGestureStrict: () => at.verdict,
  }));
  vi.doMock("./artifact", async orig => { await gate; return orig(); });
  vi.doMock("./device", async orig => { await gate; return orig(); });
  at.unwatch.push((await import("./gesture")).unwatchShell);
  const { REGISTRY } = await import("./registry");
  return { REGISTRY, link: (await import("./device-link")).deviceLink };
}

/** Lets the call that is waiting on its chunk run to the gate. */
const settle = () => new Promise(r => setTimeout(r, 0));

describe("a lazy capability judges the viewer's gesture when the call arrives, not when its chunk has loaded", () => {
  beforeEach(() => { sessionStorage.clear(); at.verdict = "ok"; });
  afterEach(() => {
    at.open();
    for (const f of at.unwatch.splice(0)) f();
    vi.unstubAllGlobals();
  });

  it("artifact.publish from a click passes though the chunk loads after the click's activation lapsed", async () => {
    const fetched = stubDaemon();
    const { REGISTRY } = await registry();
    const call = REGISTRY.artifact(env(), null as never).call("publish", [DOC]);
    await settle();
    at.verdict = "no_gesture";
    at.open();
    await expect(call).resolves.toEqual({ version: "4" });
    expect(fetched.mock.calls.some(([, init]) => init?.method === "POST")).toBe(true);
  });

  it("artifact.publish with no gesture is refused though the viewer clicks while the chunk loads", async () => {
    const fetched = stubDaemon();
    const { REGISTRY } = await registry();
    at.verdict = "no_gesture";
    const call = REGISTRY.artifact(env(), null as never).call("publish", [DOC]);
    await settle();
    at.verdict = "ok";
    at.open();
    await expect(call).rejects.toMatchObject({ code: "rate_limited" });
    expect(fetched).not.toHaveBeenCalled();
  });

  it("artifact.publish with shell input just before it is refused shell_input_recent though it is quiet by the load", async () => {
    stubDaemon();
    const { REGISTRY } = await registry();
    at.verdict = "shell_input_recent";
    const call = REGISTRY.artifact(env(), null as never).call("publish", [DOC]);
    await settle();
    at.verdict = "ok";
    at.open();
    await expect(call).rejects.toMatchObject({ code: "shell_input_recent" });
  });

  it("a device call and a home call are judged at arrival the same way", async () => {
    const sent: string[] = [];
    const run = async (arrival: typeof at.verdict, loaded: typeof at.verdict) => {
      const { REGISTRY, link } = await registry();
      link.call = async (cap, method) => { sent.push(`${cap}.${method}`); return null; };
      at.verdict = arrival;
      const calls = [REGISTRY.share(env(), null as never).call("text", ["hi"]), REGISTRY.home(env(), null as never).call("openInbox", [])];
      await settle();
      at.verdict = loaded;
      at.open();
      return (await Promise.allSettled(calls)).map(r => r.status === "fulfilled" ? r.value : (r.reason as { code?: string }).code);
    };
    expect(await run("ok", "no_gesture")).toEqual([null, null]);
    expect(sent.sort()).toEqual(["home.openInbox", "share.text"]);
    expect(await run("no_gesture", "ok")).toEqual(["no_gesture", "no_gesture"]);
    expect(sent).toHaveLength(2);
  });
});
