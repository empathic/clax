import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import { MAX_DOC_BYTES, MAX_SUBSCRIPTIONS, POLL_MS, RETRY_MS, SNAPSHOT_DEBOUNCE_MS, dbError, dbHandler } from "./db";
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
    expect(dbError(429, {}, false).code).toBe("resource_exhausted");
    // A daemon timeout is transient: subscriptions retry it.
    expect(dbError(408, {}, false).code).toBe("unavailable");
    // A republish that dropped db withdraws it from this view.
    expect(dbError(403, { code: "not_declared" }, false).code).toBe("revoked");
    expect(dbError(403, { code: "not_declared" }, true).code).toBe("revoked");
  });

  it("rejects revoked, not null, when a read finds the artifact no longer declares db", async () => {
    const { h } = setup(null, () => json({ error: { code: "not_declared", message: "this artifact does not declare the db capability" } }, 403));
    await expect(h.call("get", ["tasks/t1"])).rejects.toMatchObject({ code: "revoked" });
  });

  it("keeps a subscription through a daemon timeout and retries it", async () => {
    vi.useFakeTimers();
    let timeout = true;
    const { h, posted } = setup(null, () => (timeout ? json({ error: { code: "request_timeout", message: "timed out" } }, 408) : json({ doc: doc("tasks/a", 1) })));
    await h.call("subscribe", ["s1", { kind: "doc", path: "tasks/a" }]);
    expect(posted).toEqual([]);
    timeout = false;
    await vi.advanceTimersByTimeAsync(RETRY_MS[0] + 1);
    expect(posted).toHaveLength(1);
    expect(posted[0]).toMatchObject({ topic: "snapshot", data: { sub: "s1", docs: [{ path: "tasks/a" }] } });
  });

  it("ends an ordered subscription over the daemon's byte budget once, without retrying it", async () => {
    vi.useFakeTimers();
    const { h, posted, requests } = setup(null, () => json({ error: { code: "resource_exhausted", message: "over the budget" } }, 400));
    await h.call("subscribe", ["s1", { kind: "query", collection: "log", where: [], orderBy: "at", desc: false, limit: null }]);
    expect(posted).toEqual([{ type: "clax:event", ns: "db", topic: "snapshot-error", data: { sub: "s1", code: "resource_exhausted", message: "over the budget" } }]);
    h.onEvent!({ type: "doc", artifact_id: "7q3k9mzx2b4t", path: "log/a", version: 2 });
    await vi.advanceTimersByTimeAsync(RETRY_MS[1] * 3);
    expect(requests).toHaveLength(1);
    expect(posted).toHaveLength(1);
  });

  it("asks for every match of an ordered query without a limit, and the limit when there is one", async () => {
    const { h, requests } = setup(null, () => json({ docs: [doc("log/a", 1)], next_cursor: null }));
    await h.call("query", [{ kind: "query", collection: "log", where: [], orderBy: "at", desc: false, limit: null }]);
    await h.call("query", [{ kind: "query", collection: "log", where: [], orderBy: "at", desc: true, limit: 5 }]);
    const params = requests.map(r => new URLSearchParams(r.url.split("?")[1]));
    expect(params.map(p => [p.get("order_by"), p.get("direction"), p.get("limit")])).toEqual([["at", null, null], ["at", "desc", "5"]]);
  });

  it("counts a lease holder's length in characters, as the daemon does", async () => {
    const { h, requests } = setup(null, () => json({ acquired: true, version: 1, expires_at: "x", holder: "h" }));
    await h.call("acquire", ["locks/l", { holder: "\u{1F600}".repeat(200) }]);
    expect(requests).toHaveLength(1);
    await expect(h.call("acquire", ["locks/l", { holder: "\u{1F600}".repeat(201) }])).rejects.toMatchObject({ code: "invalid_argument" });
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
    expect(posted[0]).toMatchObject({ type: "clax:event", ns: "db", topic: "snapshot", data: { sub: "s1" } });
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

  it("maps oversized requests to invalid_argument", () => {
    for (const s of [413, 414, 431]) expect(dbError(s, {}, true).code).toBe("invalid_argument");
  });

  describe("refuses every malformed argument with invalid_argument and no request", () => {
    const Q = { kind: "query", collection: "tasks", where: [], orderBy: null, desc: false, limit: null };
    const cases: [string, unknown[]][] = [
      ["set array body", ["tasks/t1", [1]]],
      ["set null body", ["tasks/t1", null]],
      ["set string body", ["tasks/t1", "x"]],
      ["set oversized body", ["tasks/t1", { s: "x".repeat(MAX_DOC_BYTES) }]],
      ["update BigInt body", ["tasks/t1", { n: 1n }]],
      ["update undefined body", ["tasks/t1", undefined]],
      ["query where not a list", [{ ...Q, where: "n>1" }]],
      ["query where entry not a triple", [{ ...Q, where: [["n", "=="]] }]],
      ["query empty field", [{ ...Q, where: [["", "==", 1]] }]],
      ["query long field", [{ ...Q, where: [["f".repeat(201), "==", 1]] }]],
      ["query non-string field", [{ ...Q, where: [[1, "==", 1]] }]],
      ["query unknown op", [{ ...Q, where: [["n", "~", 1]] }]],
      ["query undefined value", [{ ...Q, where: [["n", "==", undefined]] }]],
      ["query BigInt value", [{ ...Q, where: [["n", "==", 1n]] }]],
      ["query 11 filters", [{ ...Q, where: Array(11).fill(["n", "==", 1]) }]],
      ["query in over 30", [{ ...Q, where: [["n", "in", Array(31).fill(1)]] }]],
      ["query not-in scalar", [{ ...Q, where: [["n", "not-in", 1]] }]],
      ["query empty orderBy", [{ ...Q, orderBy: "" }]],
      ["query numeric orderBy", [{ ...Q, orderBy: 3 }]],
      ["query desc not boolean", [{ ...Q, desc: "yes" }]],
      ["query limit 0", [{ ...Q, limit: 0 }]],
      ["query limit 1001", [{ ...Q, limit: 1001 }]],
      ["query limit fractional", [{ ...Q, limit: 1.5 }]],
      ["query no spec", [null]],
      ["acquire no holder", ["locks/l", {}]],
      ["acquire long holder", ["locks/l", { holder: "h".repeat(201) }]],
      ["acquire negative ttl", ["locks/l", { holder: "h", ttlMs: -1 }]],
      ["acquire infinite ttl", ["locks/l", { holder: "h", ttlMs: Infinity }]],
      ["acquire string ttl", ["locks/l", { holder: "h", ttlMs: "5" }]],
      ["acquire array data", ["locks/l", { holder: "h", data: [1] }]],
      ["subscribe empty ID", ["", { kind: "doc", path: "tasks/t1" }]],
      ["subscribe long ID", ["s".repeat(65), { kind: "doc", path: "tasks/t1" }]],
      ["subscribe numeric ID", [5, { kind: "doc", path: "tasks/t1" }]],
      ["subscribe bad spec", ["s1", { kind: "other" }]],
      ["unsubscribe numeric ID", [5]],
    ];
    for (const [name, args] of cases) {
      it(name, async () => {
        const { h, requests } = setup("tok", () => json({ doc: doc("tasks/t1", 1), acquired: true, docs: [], next_cursor: null }));
        const method = name.split(" ")[0];
        await expect(h.call(method, args)).rejects.toMatchObject({ code: "invalid_argument" });
        expect(requests).toEqual([]);
      });
    }
  });

  it("floors and clamps ttlMs, and treats 0 as the daemon's default", async () => {
    const { h, requests } = setup(null, () => json({ acquired: false, version: null, expires_at: null, holder: null }));
    for (const ttlMs of [0, 1.9, 10.5, 5000.7, 9e9]) await h.call("acquire", ["locks/l", { holder: "h", ttlMs }]);
    expect(requests.map(r => (r.body as { ttl_ms?: number }).ttl_ms)).toEqual([undefined, 1000, 1000, 5000, 600000]);
  });

  it(`refuses the ${MAX_SUBSCRIPTIONS + 1}th subscription, and reset drops them all`, async () => {
    vi.useFakeTimers();
    const { h, posted, requests } = setup(null, () => json({ doc: doc("tasks/t1", 1) }));
    for (let i = 0; i < MAX_SUBSCRIPTIONS; i++) await h.call("subscribe", [`s${i}`, { kind: "doc", path: "tasks/t1" }]);
    await expect(h.call("subscribe", ["over", { kind: "doc", path: "tasks/t1" }])).rejects.toMatchObject({ code: "resource_exhausted" });
    await h.call("subscribe", ["s0", { kind: "doc", path: "tasks/t1" }]);
    h.onEvent!({ type: "doc", artifact_id: "7q3k9mzx2b4t", path: "tasks/t1", version: 2 });
    h.reset!();
    const before = [posted.length, requests.length];
    await vi.advanceTimersByTimeAsync(POLL_MS * 2);
    h.onEvent!({ type: "doc", artifact_id: "7q3k9mzx2b4t", path: "tasks/t1", version: 3 });
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    expect([posted.length, requests.length]).toEqual(before);
    await h.call("subscribe", ["new", { kind: "doc", path: "tasks/t1" }]);
    expect(posted.at(-1)).toMatchObject({ topic: "snapshot", data: { sub: "new" } });
  });

  it("retries an unavailable refetch after 5 s, then every 30 s, until it succeeds", async () => {
    vi.useFakeTimers();
    let up = false;
    const { h, posted, requests } = setup(null, () => (up ? json({ doc: doc("tasks/t1", 1) }) : json({}, 503)));
    await h.call("subscribe", ["s1", { kind: "doc", path: "tasks/t1" }]);
    expect([posted.length, requests.length]).toEqual([0, 1]);
    await vi.advanceTimersByTimeAsync(RETRY_MS[0] - 1);
    expect(requests).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(requests).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(RETRY_MS[1] - 1);
    expect(requests).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(requests).toHaveLength(3);
    up = true;
    await vi.advanceTimersByTimeAsync(RETRY_MS[1]);
    expect(requests).toHaveLength(4);
    expect(posted).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(RETRY_MS[1] * 3);
    expect(requests).toHaveLength(4);
  });

  it("stops retrying on unsubscribe", async () => {
    vi.useFakeTimers();
    const { h, requests } = setup(null, () => json({}, 503));
    await h.call("subscribe", ["s1", { kind: "doc", path: "tasks/t1" }]);
    await h.call("unsubscribe", ["s1"]);
    await vi.advanceTimersByTimeAsync(RETRY_MS[1] * 3);
    expect(requests).toHaveLength(1);
  });

  it("polls every subscription every 30 s while the stream is down, until ready, unsubscribe or reset", async () => {
    vi.useFakeTimers();
    const { h, requests } = setup(null, () => json({ doc: doc("tasks/t1", 1) }));
    await h.call("subscribe", ["s1", { kind: "doc", path: "tasks/t1" }]);
    h.onEvent!({ type: "stream_down" });
    await vi.advanceTimersByTimeAsync(POLL_MS + SNAPSHOT_DEBOUNCE_MS);
    await vi.advanceTimersByTimeAsync(POLL_MS);
    expect(requests).toHaveLength(3);
    h.onEvent!({ type: "ready" });
    await vi.advanceTimersByTimeAsync(POLL_MS * 3);
    expect(requests).toHaveLength(4);
    h.onEvent!({ type: "stream_down" });
    await h.call("unsubscribe", ["s1"]);
    await vi.advanceTimersByTimeAsync(POLL_MS * 3);
    expect(requests).toHaveLength(4);
    await h.call("subscribe", ["s2", { kind: "doc", path: "tasks/t1" }]);
    await vi.advanceTimersByTimeAsync(POLL_MS + SNAPSHOT_DEBOUNCE_MS);
    expect(requests).toHaveLength(6);
    h.reset!();
    await vi.advanceTimersByTimeAsync(POLL_MS * 3);
    expect(requests).toHaveLength(6);
  });

  it("echoes this shell's own writes to matching subscriptions at once", async () => {
    vi.useFakeTimers();
    let version = 1;
    const { h, posted } = setup(null, (m, url) => {
      if (m !== "GET") { version++; return json({ doc: doc("tasks/a", version), acquired: true, version, expires_at: "t", holder: "h", deleted: true }); }
      return url.includes("?") ? json({ docs: [doc("tasks/a", version)], next_cursor: null }) : json({ doc: doc("locks/l", version) });
    });
    await h.call("subscribe", ["q", { kind: "query", collection: "tasks", where: [], orderBy: null, desc: false, limit: null }]);
    await h.call("subscribe", ["l", { kind: "doc", path: "locks/l" }]);
    await h.call("subscribe", ["o", { kind: "doc", path: "other/x" }]);
    const n = posted.length;
    const writes: [string, unknown[]][] = [["set", ["tasks/a", { v: 1 }]], ["update", ["tasks/a", { v: 2 }]], ["delete", ["tasks/a"]], ["acquire", ["locks/l", { holder: "h", data: { by: "h" } }]]];
    for (const [i, [method, args]] of writes.entries()) {
      await h.call(method, args);
      await vi.advanceTimersByTimeAsync(0);
      expect(posted.length).toBe(n + i + 1);
    }
    expect(posted.slice(n).map(m => (m as { data: { sub: string } }).data.sub)).toEqual(["q", "q", "q", "l"]);
  });

  it("refetches on a republish, and unsubscribe cancels a pending refetch", async () => {
    vi.useFakeTimers();
    const { h, requests } = setup(null, () => json({ doc: doc("tasks/t1", 1) }));
    await h.call("subscribe", ["s1", { kind: "doc", path: "tasks/t1" }]);
    h.onEvent!({ type: "version", artifact_id: "7q3k9mzx2b4t", n: 2 });
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    expect(requests).toHaveLength(2);
    h.onEvent!({ type: "doc", artifact_id: "7q3k9mzx2b4t", path: "tasks/t1", version: 2 });
    await h.call("unsubscribe", ["s1"]);
    await h.call("subscribe", ["s1", { kind: "doc", path: "other/x" }]);
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    expect(requests.map(r => r.url.split("/docs/")[1])).toEqual(["tasks/t1", "tasks/t1", "other/x"]);
  });

  it("refuses bodies holding anything but plain objects, arrays and JSON scalars", async () => {
    class Thing { a = 1; }
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    const bad: unknown[] = [cyclic, new Map(), new Set(), new Date(0), /x/, new Thing(), { nested: { m: new Map() } }, { list: [new Set()] }, { d: new Date(0) }];
    for (const body of bad) {
      const { h, requests } = setup(null, () => json({ doc: doc("tasks/t1", 1) }));
      await expect(h.call("set", ["tasks/t1", body]), String(body)).rejects.toMatchObject({ code: "invalid_argument" });
      await expect(h.call("acquire", ["locks/l", { holder: "h", data: body }])).rejects.toMatchObject({ code: "invalid_argument" });
      expect(requests).toEqual([]);
    }
    const { h, requests } = setup(null, () => json({ doc: doc("tasks/t1", 1) }));
    const bare = Object.assign(Object.create(null), { x: 1 });
    await h.call("set", ["tasks/t1", { a: [1, { b: [null, "s"] }], bare }]);
    expect(requests[0].body).toEqual({ data: { a: [1, { b: [null, "s"] }], bare: { x: 1 } }, lww: true });
  });

  it("a successful refetch clears the pending retry", async () => {
    vi.useFakeTimers();
    let up = false;
    const { h, requests, posted } = setup(null, () => (up ? json({ doc: doc("tasks/t1", 1) }) : json({}, 503)));
    await h.call("subscribe", ["s1", { kind: "doc", path: "tasks/t1" }]);
    up = true;
    h.onEvent!({ type: "doc", artifact_id: "7q3k9mzx2b4t", path: "tasks/t1", version: 1 });
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    expect([requests.length, posted.length]).toEqual([2, 1]);
    await vi.advanceTimersByTimeAsync(RETRY_MS[1] * 3);
    expect(requests).toHaveLength(2);
  });

  it("never delivers an older version of a document after a newer one", async () => {
    vi.useFakeTimers();
    let answer: unknown = { doc: doc("tasks/t1", 3) };
    let status = 200;
    const { h, posted } = setup(null, () => json(answer, status));
    await h.call("subscribe", ["s1", { kind: "doc", path: "tasks/t1" }]);
    answer = { doc: doc("tasks/t1", 2) };
    h.onEvent!({ type: "resync" });
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    answer = { error: { code: "not_found", message: "gone" } };
    status = 404;
    h.onEvent!({ type: "doc", artifact_id: "7q3k9mzx2b4t", path: "tasks/t1", version: null });
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    answer = { doc: doc("tasks/t1", 5) };
    status = 200;
    h.onEvent!({ type: "doc", artifact_id: "7q3k9mzx2b4t", path: "tasks/t1", version: 5 });
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    expect(posted.map(m => (m as { data: { docs: { version: number }[] } }).data.docs.map(d => d.version))).toEqual([[3], [], [5]]);
  });

  it("dispose drops every subscription and posts nothing afterwards", async () => {
    vi.useFakeTimers();
    let release: (r: Response) => void = () => {};
    let n = 0;
    const posted: ShellToBridge[] = [];
    vi.stubGlobal("fetch", vi.fn(() => (++n === 1 ? Promise.resolve(json({}, 503)) : new Promise<Response>(r => { release = r; }))));
    const h = dbHandler({ aid: "7q3k9mzx2b4t", token: null, post: (m: ShellToBridge) => posted.push(m) } as unknown as CapEnv, null as never);
    await h.call("subscribe", ["s1", { kind: "doc", path: "tasks/t1" }]);
    h.onEvent!({ type: "stream_down" });
    void h.call("subscribe", ["s2", { kind: "doc", path: "tasks/t2" }]);
    h.dispose!();
    release(json({ doc: doc("tasks/t2", 1) }));
    await vi.advanceTimersByTimeAsync(POLL_MS * 3);
    expect([n, posted.length]).toEqual([2, 0]);
    await expect(h.call("get", ["tasks/t1"])).rejects.toMatchObject({ code: "unavailable" });
    h.onEvent!({ type: "doc", artifact_id: "7q3k9mzx2b4t", path: "tasks/t1", version: 2 });
    await vi.advanceTimersByTimeAsync(POLL_MS);
    expect(n).toBe(2);
  });
});
