import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MAX_PAGE_BYTES, NO_GESTURE, SHELL_RECENT, artifactHandler } from "./artifact";
import { frameGestureStrict } from "./gesture";

// The viewer's gesture check itself is gesture.test.ts's; here it is a switch.
vi.mock("./gesture", () => ({ SHELL_QUIET_MS: 5_500, frameGestureStrict: vi.fn(() => "ok") }));
const gesture = (g: "ok" | "no_gesture" | "shell_input_recent") => vi.mocked(frameGestureStrict).mockReturnValue(g);
import { forgetBudgets } from "./budget";
import type { CapEnv } from "./host";

const DOC = "<!doctype html><html><body>v2</body></html>";
const INDEX = "<!doctype html><body>índex</body>";

function env(over: Partial<CapEnv> = {}) {
  return { aid: "7q3k9mzx2b4t", version: 3, pinned: false, token: "tok", reload: vi.fn(), ownPublish: { active: 0, settled: vi.fn() }, ...over } as unknown as CapEnv;
}

const FILES = { "index.html": { content_type: "text/html", size: 10 }, "notes/a b.html": { content_type: "text/html; charset=utf-8", size: 10 } };

function stub(publish: () => Response, caps: Record<string, unknown> = { artifact: {} }) {
  const bodies: { url: string; init: RequestInit }[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit = {}) => {
    bodies.push({ url, init });
    if (init.method === "POST") return publish();
    if (url.endsWith("/files/index.html")) return new Response(new TextEncoder().encode(INDEX), { headers: { "content-type": "text/html" } });
    return new Response(JSON.stringify({ artifact: { id: "7q3k9mzx2b4t", capabilities: caps, current_version: 3 }, versions: [{ n: 3, files: FILES }] }));
  }));
  return bodies;
}

/** Makes every Blob measure `pad` bytes more than its content, so a page a few
 * bytes long stands in for one at the size limit without allocating it. */
function padBlobs(pad: number) {
  const Real = globalThis.Blob;
  vi.stubGlobal("Blob", class extends Real { get size() { return super.size + pad; } });
}
/** The padding that puts `DOC` exactly at the page size limit. */
const AT_LIMIT = MAX_PAGE_BYTES - new Blob([DOC]).size;

const ok = () => new Response(JSON.stringify({ version: { n: 4 } }), { status: 201 });
const posts = (calls: { init: RequestInit }[]) => calls.filter(c => c.init.method === "POST").length;

describe("artifact.publish in the shell", () => {
  const fresh = () => { sessionStorage.clear(); forgetBudgets(); };
  beforeEach(fresh);
  afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); fresh(); gesture("ok"); });

  it("publishes with the shown version, marks the page as the publisher, and reloads", async () => {
    vi.useFakeTimers();
    const e = env();
    const calls = stub(() => new Response(JSON.stringify({ version: { n: 4 } }), { status: 201 }));
    await expect(artifactHandler(e, null as never).call("publish", [DOC])).resolves.toEqual({ version: "4" });
    const post = calls.find(c => c.init.method === "POST")!;
    expect(post.url).toBe("/api/artifacts/7q3k9mzx2b4t/versions");
    expect(JSON.parse(String(post.init.body))).toEqual({ if_version: 3, files: { "index.html": { content: DOC, encoding: "utf8" } } });
    expect((post.init.headers as Record<string, string>)["x-artifax-via"]).toBe("page");
    expect((post.init.headers as Record<string, string>).authorization).toBe("Bearer tok");
    vi.runAllTimers();
    expect(e.reload).toHaveBeenCalledTimes(1);
    expect(e.ownPublish!.active).toBe(1);
  });

  it("from a sub page replaces that page and carries the index (and every other file) forward", async () => {
    vi.useFakeTimers();
    const e = env({ page: () => "notes/a b.html" });
    const calls = stub(() => new Response(JSON.stringify({ version: { n: 4 } }), { status: 201 }));
    await expect(artifactHandler(e, null as never).call("publish", [DOC])).resolves.toEqual({ version: "4" });
    expect(calls.find(c => c.url.includes("/files/"))!.url).toBe("/api/artifacts/7q3k9mzx2b4t/versions/3/files/index.html");
    const body = JSON.parse(String(calls.find(c => c.init.method === "POST")!.init.body));
    expect(body.if_version).toBe(3);
    expect(Object.keys(body.files).sort()).toEqual(["index.html", "notes/a b.html"]);
    expect(body.files["notes/a b.html"]).toEqual({ content: DOC, encoding: "utf8", content_type: "text/html; charset=utf-8" });
    expect(body.files["index.html"].encoding).toBe("base64");
    expect(body.files["index.html"].content_type).toBe("text/html");
    const bytes = Uint8Array.from(atob(body.files["index.html"].content), c => c.charCodeAt(0));
    expect(new TextDecoder().decode(bytes)).toBe(INDEX);
    vi.runAllTimers();
    expect(e.reload).toHaveBeenCalledTimes(1);
  });

  it("refuses a call from a frame whose page is not known", async () => {
    const calls = stub(() => new Response("{}", { status: 201 }));
    await expect(artifactHandler(env({ page: () => null }), null as never).call("publish", [DOC])).rejects.toMatchObject({ code: "upstream_error" });
    expect(calls).toHaveLength(0);
  });

  it("a conflict rejects with the live version and reloads to it", async () => {
    vi.useFakeTimers();
    const e = env();
    stub(() => new Response(JSON.stringify({ error: { code: "conflict", message: "artifact is at version 5", current: 5 } }), { status: 409 }));
    await expect(artifactHandler(e, null as never).call("publish", [DOC])).rejects.toMatchObject({ code: "conflict", extra: { live: "5" } });
    vi.runAllTimers();
    expect(e.reload).toHaveBeenCalledTimes(1);
  });

  it("refuses a view without the token, an undeclared artifact, fragments, and oversized pages", async () => {
    stub(() => new Response("{}", { status: 201 }));
    await expect(artifactHandler(env({ token: null }), null as never).call("publish", [DOC])).rejects.toMatchObject({ code: "not_writer" });
    stub(() => new Response("{}", { status: 201 }), { db: {} });
    await expect(artifactHandler(env(), null as never).call("publish", [DOC])).rejects.toMatchObject({ code: "not_declared" });
    fresh();
    stub(() => new Response(JSON.stringify({ version: { n: 4 } }), { status: 201 }), { self: {} });
    await expect(artifactHandler(env(), null as never).call("publish", [DOC])).resolves.toEqual({ version: "4" });
    stub(() => new Response("{}", { status: 201 }));
    await expect(artifactHandler(env(), null as never).call("publish", ["<p>x"])).rejects.toMatchObject({ code: "invalid_content" });
    await expect(artifactHandler(env(), null as never).call("edit", [[]])).rejects.toMatchObject({ code: "invalid_content" });
  });

  it("refuses a page one byte over the size limit and makes no request for it, and publishes one at the limit", async () => {
    padBlobs(AT_LIMIT);
    let calls = stub(() => new Response(JSON.stringify({ version: { n: 4 } }), { status: 201 }));
    await expect(artifactHandler(env(), null as never).call("publish", [DOC + " "])).rejects.toMatchObject({ code: "too_large" });
    await expect(artifactHandler(env(), null as never).call("publish", [DOC.replace("v2", "vé")])).rejects.toMatchObject({ code: "too_large" });
    expect(calls, "an oversized page is refused before any request").toHaveLength(0);
    fresh();
    calls = stub(() => new Response(JSON.stringify({ version: { n: 4 } }), { status: 201 }));
    await expect(artifactHandler(env(), null as never).call("publish", [DOC])).resolves.toEqual({ version: "4" });
    expect(posts(calls)).toBe(1);
  });

  it("makes no request for hostile arguments", async () => {
    padBlobs(AT_LIMIT);
    const calls = stub(() => new Response("{}", { status: 201 }));
    const h = artifactHandler(env(), null as never);
    for (const args of [[], [null], [{ "../../x": "y" }], [["<!doctype html>"]], [42], ["<p>no doctype</p>"], [DOC + " "]]) {
      await expect(h.call("publish", args)).rejects.toHaveProperty("code");
    }
    await expect(h.call("__proto__", [DOC])).rejects.toMatchObject({ code: "capability_removed" });
    expect(calls).toHaveLength(0);
  });

  it("maps daemon refusals to the contract's codes", async () => {
    const cases: [number, unknown, string][] = [
      [413, { error: { code: "body_too_large", message: "big" } }, "too_large"],
      [400, { error: { code: "file_too_large", message: "big" } }, "too_large"],
      [401, { error: { code: "unauthorized", message: "no" } }, "not_writer"],
      [400, { error: { code: "missing_index", message: "no index" } }, "invalid_content"],
      [500, { error: { code: "internal", message: "oops" } }, "upstream_error"],
    ];
    for (const [status, body, code] of cases) {
      stub(() => new Response(JSON.stringify(body), { status }));
      fresh();
      const e = env();
      await expect(artifactHandler(e, null as never).call("publish", [DOC])).rejects.toMatchObject({ code });
      expect(e.ownPublish!.active).toBe(0);
    }
    fresh();
    vi.stubGlobal("fetch", vi.fn(async (_u: string, init: RequestInit = {}) => { if (init.method === "POST") throw new TypeError("offline"); return new Response(JSON.stringify({ artifact: { capabilities: { artifact: {} } }, versions: [] })); }));
    await expect(artifactHandler(env(), null as never).call("publish", [DOC])).rejects.toMatchObject({ code: "upstream_error" });
  });

  it("after dispose schedules no reload and makes no request", async () => {
    vi.useFakeTimers();
    const e = env();
    let release!: () => void;
    const gate = new Promise<void>(r => { release = r; });
    const calls = stub(() => new Response(JSON.stringify({ version: { n: 4 } }), { status: 201 }));
    const inner = globalThis.fetch;
    vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit = {}) => { await gate; return inner(url, init); }));
    const h = artifactHandler(e, null as never);
    const p = h.call("publish", [DOC]);
    h.dispose!();
    release();
    await expect(p).rejects.toHaveProperty("code");
    vi.runAllTimers();
    expect(e.reload).not.toHaveBeenCalled();
    expect(calls.filter(c => c.init.method === "POST")).toHaveLength(0);
    expect(e.ownPublish!.active).toBe(0);
    expect(e.ownPublish!.settled).not.toHaveBeenCalled();
    await expect(h.call("publish", [DOC])).rejects.toHaveProperty("code");
  });

  it("a timer scheduled before dispose is cleared by it", async () => {
    vi.useFakeTimers();
    const e = env();
    stub(() => new Response(JSON.stringify({ version: { n: 4 } }), { status: 201 }));
    const h = artifactHandler(e, null as never);
    await h.call("publish", [DOC]);
    h.dispose!();
    vi.runAllTimers();
    expect(e.reload).not.toHaveBeenCalled();
  });

  it("allows one publish per 2 s and 10 per minute, then rejects rate_limited with no request", async () => {
    vi.useFakeTimers({ toFake: ["Date", "setTimeout", "clearTimeout"] });
    vi.setSystemTime(1_000_000);
    const calls = stub(ok);
    const h = artifactHandler(env(), null as never);
    await h.call("publish", [DOC]);
    const before = calls.length;
    await expect(h.call("publish", [DOC])).rejects.toMatchObject({ code: "rate_limited", message: expect.stringMatching(/2 s/) });
    expect(calls.length).toBe(before);
    for (let i = 1; i < 10; i++) {
      vi.setSystemTime(1_000_000 + i * 2_000);
      await h.call("publish", [DOC]);
    }
    expect(posts(calls)).toBe(10);
    vi.setSystemTime(1_000_000 + 10 * 2_000);
    const n = calls.length;
    await expect(h.call("publish", [DOC])).rejects.toMatchObject({ code: "rate_limited", message: expect.stringMatching(/40 s/) });
    expect(calls.length).toBe(n);
    vi.setSystemTime(1_000_000 + 60_000);
    await expect(h.call("publish", [DOC])).resolves.toEqual({ version: "4" });
  });

  it("keeps the budget across a reload of the view", async () => {
    const calls = stub(ok);
    await artifactHandler(env(), null as never).call("publish", [DOC]);
    forgetBudgets(); // a reload loses memory, not sessionStorage
    const n = calls.length;
    await expect(artifactHandler(env(), null as never).call("publish", [DOC])).rejects.toMatchObject({ code: "rate_limited" });
    expect(calls.length).toBe(n);
  });

  it("refuses a publish without the strict gesture: none (on load, on a timer), or shell input within the quiet time", async () => {
    const calls = stub(ok);
    gesture("no_gesture");
    await expect(artifactHandler(env(), null as never).call("publish", [DOC])).rejects.toMatchObject({ code: "rate_limited", message: NO_GESTURE });
    gesture("shell_input_recent");
    await expect(artifactHandler(env(), null as never).call("publish", [DOC])).rejects.toMatchObject({ code: "shell_input_recent", message: SHELL_RECENT });
    expect(calls).toHaveLength(0);
    gesture("ok");
    await expect(artifactHandler(env(), null as never).call("publish", [DOC])).resolves.toEqual({ version: "4" });
  });

  it("refuses a page whose stored content type is not text/html, with no request", async () => {
    const calls = stub(ok);
    const files = { "index.html": { content_type: "text/html", size: 1 }, "data.xml": { content_type: "application/xml", size: 1 }, "b.html": { content_type: "Text/HTML; charset=utf-8", size: 1 } };
    await expect(artifactHandler(env({ page: () => "data.xml", files }), null as never).call("publish", [DOC])).rejects.toMatchObject({ code: "invalid_content" });
    await expect(artifactHandler(env({ page: () => "gone.html", files }), null as never).call("publish", [DOC])).rejects.toMatchObject({ code: "invalid_content" });
    expect(calls).toHaveLength(0);
    await expect(artifactHandler(env({ page: () => "index.html", files: { "index.html": files["b.html"] } }), null as never).call("publish", [DOC])).resolves.toEqual({ version: "4" });
  });

  it("counts overlapping publishes, so one settling cannot release the other", async () => {
    vi.useFakeTimers({ toFake: ["Date", "setTimeout", "clearTimeout"] });
    vi.setSystemTime(2_000_000);
    const e = env();
    let answerFirst!: (r: Response) => void;
    const answers = [new Promise<Response>(r => { answerFirst = r; }), Promise.resolve(new Response(JSON.stringify({ error: { code: "internal", message: "x" } }), { status: 500 }))];
    stub(() => new Response("{}"));
    const inner = globalThis.fetch;
    vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit = {}) => (init.method === "POST" ? answers.shift()! : inner(url, init))));
    const h = artifactHandler(e, null as never);
    const first = h.call("publish", [DOC]);
    await vi.waitFor(() => expect(e.ownPublish!.active).toBe(1));
    vi.setSystemTime(2_003_000);
    await expect(h.call("publish", [DOC])).rejects.toMatchObject({ code: "upstream_error" });
    expect(e.ownPublish!.active).toBe(1);
    expect(e.ownPublish!.settled).not.toHaveBeenCalled();
    answerFirst(new Response(JSON.stringify({ error: { code: "internal", message: "y" } }), { status: 500 }));
    await expect(first).rejects.toMatchObject({ code: "upstream_error" });
    expect(e.ownPublish!.active).toBe(0);
    expect(e.ownPublish!.settled).toHaveBeenCalledTimes(1);
  });
});
