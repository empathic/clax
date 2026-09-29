import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import { SNAPSHOT_DEBOUNCE_MS, dbError, dbHandler } from "./db";
import type { CapEnv } from "./host";

const doc = (path: string, version: number) => ({ path, collection: path.split("/")[0], id: path.split("/").at(-1), data: { v: version }, version, updated_at: "x" });

function setup(token: string | null, routes: (method: string, url: string, body: unknown) => Response) {
  const posted: ShellToBridge[] = [];
  const requests: { method: string; url: string; body: unknown; auth: string | null }[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit = {}) => {
    const method = init.method ?? "GET";
    const body = init.body ? JSON.parse(String(init.body)) : undefined;
    requests.push({ method, url, body, auth: (init.headers as Record<string, string>)?.authorization ?? null });
    return routes(method, url, body);
  }));
  const env = { aid: "7q3k9mzx2b4t", token, post: (m: ShellToBridge) => posted.push(m) } as unknown as CapEnv;
  return { h: dbHandler(env, null as never), posted, requests };
}
const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });

describe("db handler", () => {
  afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });

  it("reads, and writes last-writer-wins with the token only in the owner shell", async () => {
    const { h, requests } = setup("tok", (m, url) => (m === "GET" && url.endsWith("/missing") ? json({ error: { code: "not_found", message: "not found" } }, 404) : json({ doc: doc("tasks/t1", 1), created: true, deleted: true })));
    expect(await h.call("get", ["tasks/t1"])).toEqual({ path: "tasks/t1", id: "t1", data: { v: 1 }, version: 1 });
    expect(await h.call("get", ["tasks/missing"])).toBeNull();
    await h.call("set", ["tasks/t1", { a: 1 }]);
    await h.call("delete", ["tasks/t1"]);
    expect(requests.slice(2).map(r => [r.method, r.url, r.body, r.auth])).toEqual([
      ["PUT", "/api/artifacts/7q3k9mzx2b4t/docs/tasks/t1", { data: { a: 1 }, lww: true }, "Bearer tok"],
      ["DELETE", "/api/artifacts/7q3k9mzx2b4t/docs/tasks/t1?lww=true", undefined, "Bearer tok"],
    ]);
    const lan = setup(null, () => json({ doc: doc("tasks/t1", 1) }));
    await lan.h.call("update", ["tasks/t1", { a: 2 }]);
    expect(lan.requests[0].auth).toBeNull();
  });

  it("maps daemon errors to the contract's codes", () => {
    expect(dbError(404, { code: "not_found" }, true).code).toBe("invalid_argument");
    expect(dbError(400, { code: "quota_exceeded" }, true).code).toBe("quota_exceeded");
    expect(dbError(400, { code: "invalid_argument" }, false).code).toBe("invalid_argument");
    expect(dbError(500, {}, false).code).toBe("unavailable");
    expect(dbError(400, { code: "resource_exhausted" }, true).code).toBe("resource_exhausted");
  });

  it("refuses paths and specs that break the grammar before any request", async () => {
    const { h, requests } = setup("tok", () => json({ doc: doc("tasks/t1", 1) }));
    for (const bad of ["../../x", "tasks/..", "tasks/./t", "tasks", "a/b%2F", 7]) {
      await expect(h.call("delete", [bad]), String(bad)).rejects.toMatchObject({ code: "invalid_argument" });
    }
    await expect(h.call("subscribe", ["s1", { kind: "doc", path: "../x/y" }])).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(h.call("query", [{ kind: "doc", path: "tasks/t1" }])).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(h.call("query", [{ kind: "query", collection: "tasks/t1", where: [], orderBy: null, desc: false, limit: null }])).rejects.toMatchObject({ code: "invalid_argument" });
    expect(requests).toEqual([]);
  });

  it("refetches on ready, and a slower earlier fetch never overwrites a newer one", async () => {
    vi.useFakeTimers();
    const gates: ((r: Response) => void)[] = [];
    let n = 0;
    const posted: ShellToBridge[] = [];
    vi.stubGlobal("fetch", vi.fn(() => {
      n++;
      return n === 1 ? Promise.resolve(json({ doc: doc("tasks/a", 1) })) : new Promise<Response>(r => gates.push(r));
    }));
    const h = dbHandler({ aid: "7q3k9mzx2b4t", token: null, post: (m: ShellToBridge) => posted.push(m) } as unknown as CapEnv, null as never);
    await h.call("subscribe", ["s1", { kind: "doc", path: "tasks/a" }]);
    h.onEvent!({ type: "ready" });
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    h.onEvent!({ type: "doc", artifact_id: "7q3k9mzx2b4t", path: "tasks/a", version: null });
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    expect(gates).toHaveLength(2);
    gates[1](json({ error: { code: "not_found", message: "gone", path: "tasks/a" } }, 404));
    await vi.advanceTimersByTimeAsync(0);
    gates[0](json({ doc: doc("tasks/a", 2) }));
    await vi.advanceTimersByTimeAsync(0);
    expect(posted.map(m => (m as { data: { docs: { version: number }[] } }).data.docs.map(d => d.version))).toEqual([[1], []]);
  });

  it("pages an unordered, unlimited query through the whole collection", async () => {
    const { h, requests } = setup(null, (_m, url) => (url.includes("cursor=") ? json({ docs: [doc("tasks/c", 1)], next_cursor: null }) : json({ docs: [doc("tasks/a", 1), doc("tasks/b", 1)], next_cursor: "b" })));
    const docs = await h.call("query", [{ kind: "query", collection: "tasks", where: [["n", ">", 1]], orderBy: null, desc: false, limit: null }]);
    expect((docs as { id: string }[]).map(d => d.id)).toEqual(["a", "b", "c"]);
    expect(decodeURIComponent(requests[0].url)).toContain('where=[["n",">",1]]');
  });

  it("pushes a snapshot on subscribe and again, debounced, for each doc event that touches it", async () => {
    vi.useFakeTimers();
    let version = 1;
    const { h, posted } = setup(null, () => json({ docs: [doc("tasks/a", version)], next_cursor: null }));
    await h.call("subscribe", ["s1", { kind: "query", collection: "tasks", where: [], orderBy: null, desc: false, limit: null }]);
    expect(posted).toHaveLength(1);
    expect(posted[0]).toMatchObject({ type: "artifax:event", ns: "db", topic: "snapshot", data: { sub: "s1" } });
    version = 2;
    h.onEvent!({ type: "doc", artifact_id: "7q3k9mzx2b4t", path: "tasks/a", version: 2 });
    h.onEvent!({ type: "doc", artifact_id: "7q3k9mzx2b4t", path: "tasks/b", version: 1 });
    h.onEvent!({ type: "doc", artifact_id: "7q3k9mzx2b4t", path: "other/x", version: 1 });
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    expect(posted).toHaveLength(2);
    await h.call("unsubscribe", ["s1"]);
    h.onEvent!({ type: "doc", artifact_id: "7q3k9mzx2b4t", path: "tasks/a", version: 3 });
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    expect(posted).toHaveLength(2);
  });

  it("reports a subscription the daemon refuses as snapshot-error", async () => {
    const { h, posted } = setup(null, () => json({ error: { code: "invalid_argument", message: "bad where" } }, 400));
    await h.call("subscribe", ["s1", { kind: "query", collection: "tasks", where: [], orderBy: null, desc: false, limit: null }]);
    expect(posted[0]).toMatchObject({ topic: "snapshot-error", data: { sub: "s1", code: "invalid_argument" } });
  });

  it("maps acquire's result to the contract's camelCase", async () => {
    const { h, requests } = setup("t", () => json({ acquired: true, version: 1, expires_at: "2026-09-29T10:00:30.000Z", holder: "tab" }));
    expect(await h.call("acquire", ["locks/l", { holder: "tab", ttlMs: 5000 }])).toEqual({ acquired: true, version: 1, expiresAt: "2026-09-29T10:00:30.000Z", holder: "tab" });
    expect(requests[0].body).toEqual({ path: "locks/l", holder: "tab", ttl_ms: 5000 });
  });
});
