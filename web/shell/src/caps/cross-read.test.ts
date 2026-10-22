import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShellToBridge } from "../../../bridge/src/protocol";
import type { StreamEvent } from "../stream";
import { crossRead } from "./cross-read";
import { MAX_SUBSCRIPTIONS, SNAPSHOT_DEBOUNCE_MS, dbHandler } from "./db";
import type { CapEnv } from "./host";
import type { Prompt, PromptAnswer } from "./grants";

const A = "7q3k9mzx2b4t";
const B = "8r4m0nay3c5v";
const C = "9s5n1pbz4d6w";
const doc = (path: string, version: number) => ({ path, collection: path.split("/")[0], id: path.split("/").at(-1), data: { v: version }, version, updated_at: "x" });
const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });
const refused = () => json({ error: { code: "not_granted", message: `${A} may not read ${B}'s documents` } }, 403);

type Req = { method: string; url: string; reader: string | null; auth: string | null };

/** A daemon where A may read B once `granted` (a grant or B's share). */
function setup(opts: { token?: string | null; declared?: Record<string, unknown>; answer?: PromptAnswer; granted?: boolean; version?: () => number; sourceDb?: boolean } = {}) {
  const state = { granted: opts.granted ?? false, docVersion: opts.version ?? (() => 1), sourceDb: opts.sourceDb ?? true, gate: null as Promise<void> | null };
  const requests: Req[] = [];
  const posted: ShellToBridge[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit = {}) => {
    const h = (init.headers ?? {}) as Record<string, string>;
    const method = init.method ?? "GET";
    requests.push({ method, url, reader: h["x-clax-reader"] ?? null, auth: h.authorization ?? null });
    if (method === "PUT" && url === `/api/artifacts/${A}/db-grants/${B}`) {
      state.granted = true;
      return json({ grant: { source_id: B, granted_at: "now" } });
    }
    if (url === `/api/artifacts/${A}`) return json({ artifact: { id: A, title: "Dashboard" } });
    if (url === `/api/artifacts/${B}`) return json({ artifact: { id: B, title: "Runs", capabilities: state.sourceDb ? { db: {} } : {} } });
    if (url.startsWith(`/api/artifacts/${B}/docs`)) {
      if (state.gate) await state.gate;
      if (!state.granted) return refused();
      if (!state.sourceDb) return json({ error: { code: "not_declared", message: "this artifact does not declare the db capability" } }, 403);
      if (url.includes("?collection=")) return json({ docs: [doc("runs/r1", state.docVersion())], next_cursor: null });
      return json({ doc: doc(url.split("/docs/")[1], state.docVersion()) });
    }
    return json({ error: { code: "not_found", message: "no" } }, 404);
  }));
  const prompts: Prompt[] = [];
  const prompt = vi.fn(async (p: Prompt) => { prompts.push(p); return opts.answer ?? "allow"; });
  const env = {
    aid: A, version: 1, pinned: false, token: opts.token === undefined ? "tok" : opts.token,
    declared: opts.declared ?? { db: { read: [B] } }, prompt, post: (m: ShellToBridge) => posted.push(m),
  } as unknown as CapEnv;
  const watchers: { topics: string[]; on: (e: StreamEvent) => void; off: boolean }[] = [];
  const watch = (topics: string[], on: (e: StreamEvent) => void) => {
    const w = { topics, on, off: false };
    watchers.push(w);
    return () => { w.off = true; };
  };
  return { cr: crossRead(env, watch), env, state, requests, posted, prompt, prompts, watchers };
}

const get = (path: string, artifact: string = B) => ({ op: "get", artifact, path });

describe("db.artifact(id) in the shell", () => {
  afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });

  it("refuses not_declared, with no request, unless the page's declaration lists the source", async () => {
    for (const declared of [{}, { db: {} }, { db: { read: [C] } }, { db: { read: B } }]) {
      const { cr, requests, prompt } = setup({ declared });
      await expect(cr.call(get("runs/r1")), JSON.stringify(declared)).rejects.toMatchObject({ code: "not_declared" });
      await expect(cr.call({ op: "query", artifact: B, spec: { kind: "query", collection: "runs", where: [], orderBy: null, desc: false, limit: null } })).rejects.toMatchObject({ code: "not_declared" });
      await expect(cr.call({ op: "subscribe", artifact: B, sub: "s1", spec: { kind: "doc", path: "runs/r1" } })).rejects.toMatchObject({ code: "not_declared" });
      expect(requests).toEqual([]);
      expect(prompt).not.toHaveBeenCalled();
    }
    // The page's own artifact is never a source.
    const own = setup({ declared: { db: { read: [A] } } });
    await expect(own.cr.call(get("runs/r1", A))).rejects.toMatchObject({ code: "not_declared" });
  });

  it("refuses bad IDs, paths and ops before any request", async () => {
    const { cr, requests } = setup();
    for (const bad of ["../x", "8R4M0NAY3C5V", `${B}/docs`, 7, undefined]) {
      await expect(cr.call({ op: "get", artifact: bad, path: "runs/r1" }), String(bad)).rejects.toMatchObject({ code: "invalid_argument" });
    }
    for (const bad of ["../../x", "runs/..", "runs/./r", "runs", "a/b%2F"]) {
      await expect(cr.call(get(bad)), bad).rejects.toMatchObject({ code: "invalid_argument" });
    }
    await expect(cr.call({ op: "set", artifact: B, path: "runs/r1" })).rejects.toMatchObject({ code: "capability_removed" });
    await expect(cr.call(null)).rejects.toMatchObject({ code: "capability_removed" });
    expect(requests).toEqual([]);
  });

  it("asks once; Allow makes the grant, then reads as the page's own artifact", async () => {
    const { cr, requests, prompts } = setup();
    // Two reads at once share one dialog.
    const [one, two] = await Promise.all([cr.call(get("runs/r1")), cr.call(get("runs/r2"))]);
    expect(one).toEqual({ path: "runs/r1", id: "r1", data: { v: 1 }, version: 1 });
    expect(two).toMatchObject({ path: "runs/r2" });
    expect(prompts).toEqual([{ kind: "crossRead", title: "Let “Dashboard” read “Runs”'s data?", body: expect.stringContaining("cannot change"), allow: "Allow", deny: "Don't allow", always: "Always for this app" }]);
    expect(requests.filter(r => r.method === "PUT")).toEqual([{ method: "PUT", url: `/api/artifacts/${A}/db-grants/${B}`, reader: null, auth: "Bearer tok" }]);
    const reads = requests.filter(r => r.url.includes("/docs"));
    expect(reads.every(r => r.reader === A && r.method === "GET")).toBe(true);
    expect(reads.map(r => r.url)).toContain(`/api/artifacts/${B}/docs/runs/r1`);
    await cr.call(get("runs/r3"));
    expect(prompts).toHaveLength(1);
  });

  it("sets the reader from the view, never from what the page sends", async () => {
    const { cr, requests } = setup({ granted: true });
    await cr.call({ ...get("runs/r1"), reader: C, headers: { "x-clax-reader": C } });
    expect(requests.map(r => [r.url, r.reader])).toEqual([[`/api/artifacts/${B}/docs/runs/r1`, A]]);
  });

  it("Don't allow rejects not_granted and is not asked again in the view", async () => {
    const { cr, requests, prompt } = setup({ answer: "deny" });
    await expect(cr.call(get("runs/r1"))).rejects.toMatchObject({ code: "not_granted" });
    await expect(cr.call(get("runs/r1"))).rejects.toMatchObject({ code: "not_granted" });
    expect(prompt).toHaveBeenCalledTimes(1);
    expect(requests.some(r => r.method === "PUT")).toBe(false);
  });

  it("a dismissed prompt rejects that read and keeps no refusal: the next read asks again", async () => {
    const { cr, requests, prompt } = setup({ answer: "dismiss" });
    await expect(cr.call(get("runs/r1"))).rejects.toMatchObject({ code: "not_granted" });
    expect(requests.some(r => r.method === "PUT")).toBe(false);
    prompt.mockImplementation(async () => "allow");
    await expect(cr.call(get("runs/r1"))).resolves.toMatchObject({ path: "runs/r1" });
    expect(prompt).toHaveBeenCalledTimes(2);
    expect(requests.filter(r => r.method === "PUT")).toHaveLength(1);
  });

  it("in the app's inspect view, rejects not_granted without asking or granting", async () => {
    history.replaceState(null, "", `/a/${A}?app=1&inspect=1`);
    try {
      const { cr, requests, prompt } = setup();
      await expect(cr.call(get("runs/r1"))).rejects.toMatchObject({ code: "not_granted" });
      await expect(cr.call({ op: "subscribe", artifact: B, sub: "s1", spec: { kind: "doc", path: "runs/r1" } })).rejects.toMatchObject({ code: "not_granted" });
      expect(prompt).not.toHaveBeenCalled();
      expect(requests.filter(r => r.method !== "GET" || !r.url.includes("/docs"))).toEqual([]);
      // A grant the owner made before is still served.
      const granted = setup({ granted: true });
      await expect(granted.cr.call(get("runs/r1"))).resolves.toMatchObject({ path: "runs/r1" });
    } finally {
      history.replaceState(null, "", "/");
    }
  });

  it("reads a source that shares, or was granted before, without asking", async () => {
    const { cr, prompt, requests } = setup({ granted: true });
    expect(await cr.call(get("runs/r1"))).toMatchObject({ id: "r1" });
    const docs = await cr.call({ op: "query", artifact: B, spec: { kind: "query", collection: "runs", where: [], orderBy: null, desc: false, limit: null } });
    expect(docs).toEqual([{ path: "runs/r1", id: "r1", data: { v: 1 }, version: 1 }]);
    expect(prompt).not.toHaveBeenCalled();
    expect(requests.every(r => r.reader === A)).toBe(true);
  });

  it("never asks a viewer who is not the owner, and never grants for them", async () => {
    const { cr, prompt, requests } = setup({ token: null });
    await expect(cr.call(get("runs/r1"))).rejects.toMatchObject({ code: "not_granted" });
    expect(prompt).not.toHaveBeenCalled();
    expect(requests.map(r => [r.method, r.auth, r.reader])).toEqual([["GET", null, A]]);
  });

  it("follows only the source's doc events on its reader topic, and ends on a revoke", async () => {
    vi.useFakeTimers();
    let version = 1;
    const { cr, state, posted, watchers, prompt } = setup({ granted: true, version: () => version });
    await cr.call({ op: "subscribe", artifact: B, sub: "s1", spec: { kind: "query", collection: "runs", where: [], orderBy: null, desc: false, limit: null } });
    expect(watchers.map(w => w.topics)).toEqual([[`docs:${B}?reader=${A}`]]);
    expect(posted).toEqual([{ type: "clax:event", ns: "db", topic: "snapshot", data: { sub: "s1", docs: [{ path: "runs/r1", id: "r1", data: { v: 1 }, version: 1 }] } }]);
    const on = watchers[0].on;
    const topic = `docs:${B}?reader=${A}`;
    version = 2;
    // Not forwarded, and no refetch: another topic, another collection, other kinds of event.
    on({ type: "doc", topic: `docs:${B}`, artifact_id: B, path: "runs/r9", version: 2 });
    on({ type: "doc", topic, artifact_id: B, path: "notes/n1", version: 2 });
    on({ type: "doc", topic, artifact_id: C, path: "runs/r9", version: 2 });
    on({ type: "thread", topic, artifact_id: B, thread: { id: "t" } });
    on({ type: "version", topic, artifact_id: B, n: 2 });
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    expect(posted).toHaveLength(1);
    on({ type: "doc", topic, artifact_id: B, path: "runs/r9", version: 2 });
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    expect(posted).toHaveLength(2);
    expect(posted[1]).toMatchObject({ topic: "snapshot", data: { sub: "s1", docs: [{ version: 2 }] } });
    expect(posted.every(m => m.type === "clax:event" && m.ns === "db")).toBe(true);
    // Revoked: the next refetch is refused, the subscription ends once, its topic is let go.
    state.granted = false;
    on({ type: "ready" });
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    expect(posted[2]).toEqual({ type: "clax:event", ns: "db", topic: "snapshot-error", data: { sub: "s1", code: "not_granted", message: expect.any(String) } });
    expect(watchers[0].off).toBe(true);
    expect(prompt).not.toHaveBeenCalled();
    on({ type: "doc", topic, artifact_id: B, path: "runs/r9", version: 3 });
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    expect(posted).toHaveLength(3);
  });

  it("asks before subscribing, watches only after consent, and lets go on unsubscribe and reset", async () => {
    const denied = setup({ answer: "deny" });
    await expect(denied.cr.call({ op: "subscribe", artifact: B, sub: "s1", spec: { kind: "doc", path: "runs/r1" } })).rejects.toMatchObject({ code: "not_granted" });
    expect(denied.watchers).toEqual([]);
    vi.unstubAllGlobals();
    const { cr, watchers, prompt } = setup();
    await cr.call({ op: "subscribe", artifact: B, sub: "s1", spec: { kind: "doc", path: "runs/r1" } });
    await cr.call({ op: "subscribe", artifact: B, sub: "s2", spec: { kind: "doc", path: "runs/r2" } });
    expect(prompt).toHaveBeenCalledTimes(1);
    expect(watchers).toHaveLength(1);
    await cr.call({ op: "unsubscribe", sub: "s1" });
    expect(watchers[0].off).toBe(false);
    await cr.call({ op: "unsubscribe", sub: "s2" });
    expect(watchers[0].off).toBe(true);
    await cr.call({ op: "subscribe", artifact: B, sub: "s3", spec: { kind: "doc", path: "runs/r1" } });
    cr.reset();
    expect(watchers[1].off).toBe(true);
  });

  it("is served by the db handler's read method, and ends with it", async () => {
    const { env, requests, watchers, posted } = setup({ granted: true });
    const h = dbHandler(env, null as never);
    expect(await h.call("read", [get("runs/r1")])).toMatchObject({ id: "r1" });
    expect(requests.map(r => r.reader)).toEqual([A]);
    // The page's own subscriptions are untouched by the source's reads.
    expect(posted).toEqual([]);
    expect(watchers).toEqual([]);
    h.dispose!();
    await expect(h.call("read", [get("runs/r1")])).rejects.toMatchObject({ code: "unavailable" });
  });

  it("holds at most MAX_SUBSCRIPTIONS subscriptions of other apps, however many are asked for at once", async () => {
    const { cr, state, watchers } = setup({ granted: true });
    let open!: () => void;
    state.gate = new Promise<void>(r => { open = r; });
    const all = Array.from({ length: 200 }, (_, i) => cr.call({ op: "subscribe", artifact: B, sub: `s${i}`, spec: { kind: "doc", path: "runs/r1" } }));
    open();
    const results = await Promise.allSettled(all);
    expect(results.filter(r => r.status === "fulfilled")).toHaveLength(MAX_SUBSCRIPTIONS);
    const refusals = results.filter((r): r is PromiseRejectedResult => r.status === "rejected");
    expect(refusals).toHaveLength(200 - MAX_SUBSCRIPTIONS);
    expect(refusals.every(r => r.reason.code === "resource_exhausted")).toBe(true);
    expect(watchers).toHaveLength(1);
  });

  it("drops a subscribe still waiting on its first read when the frame resets or leaves", async () => {
    for (const end of ["reset", "leave"] as const) {
      const { cr, state, posted, watchers } = setup({ granted: true });
      let open!: () => void;
      state.gate = new Promise<void>(r => { open = r; });
      const pending = cr.call({ op: "subscribe", artifact: B, sub: "s1", spec: { kind: "doc", path: "runs/r1" } });
      cr[end]();
      open();
      await expect(pending, end).rejects.toMatchObject({ code: "unavailable" });
      expect(watchers, end).toEqual([]);
      expect(posted, end).toEqual([]);
      // The slot is free again for the next document.
      state.gate = null;
      await cr.call({ op: "subscribe", artifact: B, sub: "s1", spec: { kind: "doc", path: "runs/r1" } });
      expect(watchers, end).toHaveLength(1);
    }
  });

  it("keeps not_declared from a source without db, and never asks for a grant that cannot help", async () => {
    for (const granted of [true, false]) {
      const { cr, prompt, requests } = setup({ granted, sourceDb: false });
      await expect(cr.call(get("runs/r1")), String(granted)).rejects.toMatchObject({ code: "not_declared" });
      expect(prompt).not.toHaveBeenCalled();
      expect(requests.some(r => r.method === "PUT")).toBe(false);
    }
  });
});
