import { describe, expect, it, vi } from "vitest";
import { MAX_SUBSCRIPTIONS, checkCollectionPath, checkDocPath, makeDb, querySnapshot, type WireDoc } from "../src/caps/db";
import { CapabilityError } from "../src/rpc";

type Listener = (d: unknown) => void;
function fakeRpc(answer: (method: string, args: unknown[]) => unknown = () => null) {
  const listeners = new Map<string, Set<Listener>>();
  const calls: { method: string; args: unknown[] }[] = [];
  return {
    calls,
    emit(topic: string, data: unknown) { for (const f of listeners.get(topic) ?? []) f(data); },
    rpc: {
      call: vi.fn(async (_ns: string, method: string, args: unknown[]) => { calls.push({ method, args }); return answer(method, args); }),
      on: (_ns: string, topic: string, f: Listener) => { if (!listeners.has(topic)) listeners.set(topic, new Set()); listeners.get(topic)!.add(f); return () => listeners.get(topic)!.delete(f); },
    },
  };
}
const w = (id: string, version: number, data: Record<string, unknown> = { v: version }): WireDoc => ({ path: `tasks/${id}`, id, data, version });

describe("db paths", () => {
  it("throws TypeError synchronously for paths that break the grammar", () => {
    expect(checkDocPath("tasks/t1")).toBe("tasks/t1");
    expect(checkCollectionPath("boards/b1/columns")).toBe("boards/b1/columns");
    for (const bad of ["tasks", "", "a/../b/c", "a/b c", "a/é", `a/${"x".repeat(201)}`]) expect(() => checkDocPath(bad), bad).toThrow(TypeError);
    expect(() => checkCollectionPath("tasks/t1")).toThrow(/2 segments/);
    const { rpc } = fakeRpc();
    const db = makeDb(rpc as never);
    expect(() => db.doc("tasks")).toThrow(TypeError);
    expect(() => db.collection("tasks").doc("a/b")).toThrow(TypeError);
    expect(db.collection("tasks").doc().id).toMatch(/^[A-Za-z0-9]{20}$/);
    expect(db.doc("tasks/t1").collection("subs").path).toBe("tasks/t1/subs");
  });
});

describe("db refs", () => {
  it("get delivers frozen snapshots; absence is exists:false", async () => {
    const { rpc } = fakeRpc((m, a) => (m === "get" && a[0] === "tasks/t1" ? w("t1", 1, { title: "Ship" }) : null));
    const db = makeDb(rpc as never);
    const s = await db.doc("tasks/t1").get();
    expect([s.id, s.exists, s.data()]).toEqual(["t1", true, { title: "Ship" }]);
    expect(Object.isFrozen(s) && Object.isFrozen(s.data())).toBe(true);
    const none = await db.doc("tasks/zz").get();
    expect([none.exists, none.data()]).toEqual([false, undefined]);
    expect(none.metadata).toEqual({ fromCache: false, hasPendingWrites: false });
  });

  it("writes reject non-object bodies and forward the rest", async () => {
    const f = fakeRpc();
    const db = makeDb(f.rpc as never);
    await expect(db.doc("tasks/t1").set([1] as never)).rejects.toMatchObject({ code: "invalid_argument" });
    await db.doc("tasks/t1").update({ a: 1 });
    await db.doc("tasks/t1").delete();
    const ref = await db.collection("tasks").add({ b: 2 });
    expect(f.calls.map(c => c.method)).toEqual(["update", "delete", "set"]);
    expect(f.calls[2].args).toEqual([ref.path, { b: 2 }]);
  });

  it("queries are immutable builders validated at the terminal call", async () => {
    const f = fakeRpc(() => [w("a", 1), w("b", 1)]);
    const db = makeDb(f.rpc as never);
    const base = db.collection("tasks");
    const q = base.where("n", ">", 1).orderBy("n", "desc").limit(5);
    expect(q).not.toBe(base);
    const snap = await q.get();
    expect([snap.size, snap.empty, snap.docs.map(d => d.id)]).toEqual([2, false, ["a", "b"]]);
    expect(f.calls[0].args[0]).toEqual({ kind: "query", collection: "tasks", where: [["n", ">", 1]], orderBy: "n", desc: true, limit: 5 });
    await expect(base.where("n", "~", 1).get()).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(base.limit(0).get()).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(base.orderBy("a").orderBy("b").get()).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(base.where("n", "in", Array(31).fill(1)).get()).rejects.toMatchObject({ code: "invalid_argument" });
  });

  it("onSnapshot reuses unchanged documents and reports changes", async () => {
    const f = fakeRpc();
    const db = makeDb(f.rpc as never);
    const seen: { ids: string[]; changes: string[] }[] = [];
    const firstA: unknown[] = [];
    const stop = db.collection("tasks").onSnapshot(s => {
      seen.push({ ids: s.docs.map(d => d.id), changes: s.docChanges().map(c => `${c.type}:${c.doc.id}:${c.oldIndex}:${c.newIndex}`) });
      const a = s.docs.find(x => x.id === "a");
      if (a) firstA.push(a);
    });
    await Promise.resolve();
    const sub = (f.calls[0].args as [string])[0];
    f.emit("snapshot", { sub, docs: [w("a", 1), w("b", 1)] });
    f.emit("snapshot", { sub, docs: [w("a", 1), w("b", 1)] });
    f.emit("snapshot", { sub, docs: [w("a", 1), w("b", 2), w("c", 1)] });
    f.emit("snapshot", { sub, docs: [w("b", 2), w("c", 1)] });
    expect(seen).toEqual([
      { ids: ["a", "b"], changes: ["added:a:-1:0", "added:b:-1:1"] },
      { ids: ["a", "b", "c"], changes: ["modified:b:1:1", "added:c:-1:2"] },
      { ids: ["b", "c"], changes: ["removed:a:0:-1", "modified:b:1:0", "modified:c:2:1"] },
    ]);
    stop();
    stop();
    f.emit("snapshot", { sub, docs: [] });
    expect(seen).toHaveLength(3);
    expect(firstA[0]).toBe(firstA[1]);
    expect(f.calls.at(-1)).toEqual({ method: "unsubscribe", args: [sub] });
    const again = querySnapshot([w("a", 1)], querySnapshot([w("a", 1)], null).cache);
    expect(again.changed).toBe(false);
  });

  it("a terminal error reaches the error callback once and ends the listener", async () => {
    const f = fakeRpc();
    const db = makeDb(f.rpc as never);
    const errors: string[] = [];
    const next = vi.fn();
    db.doc("tasks/t1").onSnapshot(next, e => errors.push(e.code));
    await Promise.resolve();
    const sub = (f.calls[0].args as [string])[0];
    f.emit("snapshot-error", { sub, code: "invalid_argument", message: "bad" });
    f.emit("snapshot-error", { sub, code: "invalid_argument", message: "bad" });
    f.emit("snapshot", { sub, docs: [w("t1", 1)] });
    expect(errors).toEqual(["invalid_argument"]);
    expect(next).not.toHaveBeenCalled();
  });

  it(`the ${MAX_SUBSCRIPTIONS + 1}th subscription fails with resource_exhausted`, async () => {
    const f = fakeRpc();
    const db = makeDb(f.rpc as never);
    for (let i = 0; i < MAX_SUBSCRIPTIONS; i++) db.doc(`tasks/t${i}`).onSnapshot(() => {});
    const err = await new Promise<CapabilityError>(r => db.doc("tasks/over").onSnapshot(() => {}, r));
    expect(err.code).toBe("resource_exhausted");
  });

  it("acquire needs a holder", async () => {
    const f = fakeRpc(() => ({ acquired: true, version: 1, expiresAt: "t", holder: "h" }));
    const db = makeDb(f.rpc as never);
    await expect(db.doc("locks/l").acquire({} as never)).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(db.doc("locks/l").acquire({ holder: "h" })).resolves.toMatchObject({ acquired: true });
  });
});
