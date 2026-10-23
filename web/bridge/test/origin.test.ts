import { afterEach, describe, expect, it, vi } from "vitest";
import { MAX_SUBSCRIPTIONS, POLL_MS, RETRY_MS, SNAPSHOT_DEBOUNCE_MS, UNWATCHED_POLL_MS, type Whoami, dbTransport, originUse, rootWrite, siteOf, userLocals } from "../src/caps/origin";
import type { OriginClient, WireDoc } from "../src/caps/origin-client";
import { CapabilityError } from "../src/rpc";

const doc = (path: string, version: number): WireDoc => ({ path, id: path.split("/").at(-1)!, data: { v: version }, version });
const ME = "u_00000000000000000000aa";
const OTHER = "u_00000000000000000000bb";

/** A site whose db answers `answer(method, args)`; every call is listed. */
function site(answer: (method: string, args: unknown[]) => unknown) {
  const calls: [string, string, unknown[]][] = [];
  const client: OriginClient = {
    call: async (cap, method, a) => {
      const args = a as unknown[];
      calls.push([cap, method, args]);
      const v = answer(method, args);
      if (v instanceof Error) throw v;
      return v;
    },
    socket: () => { throw new Error("no socket"); },
  };
  return { client, calls };
}

/** A framing shell that grants what `granted` names and records what it is asked. */
function shell(granted: string[] = ["db", "user"], answers: Record<string, (args: unknown[]) => unknown> = {}) {
  const listeners = new Map<string, (d: unknown) => void>();
  const asked: string[] = [];
  return {
    use: vi.fn(async (name: string) => (granted.includes(name) ? { config: {} } : null)),
    call: vi.fn(async (ns: string, method: string, args: unknown[]) => { asked.push(`${ns}.${method}`); return answers[`${ns}.${method}`]?.(args) ?? null; }),
    on: vi.fn((ns: string, topic: string, fn: (d: unknown) => void) => { listeners.set(`${ns}/${topic}`, fn); return () => {}; }),
    push: (topic: string, data: unknown) => listeners.get(`db/${topic}`)?.(data),
    asked,
  };
}

function pushes(t: ReturnType<typeof dbTransport>) {
  const got: { topic: string; data: Record<string, unknown> }[] = [];
  t.on("db", "snapshot", d => got.push({ topic: "snapshot", data: d as Record<string, unknown> }));
  t.on("db", "snapshot-error", d => got.push({ topic: "snapshot-error", data: d as Record<string, unknown> }));
  return got;
}
const gets = (calls: [string, string, unknown[]][]) => calls.filter(c => c[1] === "get" || c[1] === "query").length;
const sub = (path: string) => ({ kind: "doc", path });
const tasks = { kind: "query", collection: "tasks", where: [], orderBy: null, desc: false, limit: null };

describe("db on the page's own origin", () => {
  afterEach(() => { vi.useRealTimers(); });

  it("reads and writes through the site, and refetches the matching subscriptions right after a write", async () => {
    vi.useFakeTimers();
    let v = 1;
    const { client, calls } = site(m => (m === "query" ? [doc("tasks/a", v)] : m === "get" ? doc("locks/l", v) : m === "acquire" ? { acquired: true } : (v++, null)));
    const t = dbTransport(client, false);
    const got = pushes(t);
    await t.call("db", "subscribe", ["q", tasks]);
    await t.call("db", "subscribe", ["l", sub("locks/l")]);
    await t.call("db", "subscribe", ["o", sub("other/x")]);
    const n = got.length;
    for (const [method, args] of [["set", ["tasks/a", { v: 1 }]], ["update", ["tasks/a", { v: 2 }]], ["delete", ["tasks/a"]], ["acquire", ["locks/l", { holder: "h" }]]] as const) {
      await t.call("db", method, [...args]);
      await vi.advanceTimersByTimeAsync(0);
    }
    expect(got.slice(n).map(g => g.data.sub)).toEqual(["q", "q", "q", "l"]);
    expect(calls.filter(c => !["get", "query"].includes(c[1])).map(c => c[1])).toEqual(["set", "update", "delete", "acquire"]);
  });

  it("polls while no shell says what changed, slower while nothing does, and stops with the last unsubscribe", async () => {
    vi.useFakeTimers();
    let v = 1;
    const { client, calls } = site(m => (m === "get" ? doc("tasks/t1", v) : (v++, null)));
    const t = dbTransport(client, false);
    const got = pushes(t);
    await t.call("db", "subscribe", ["s1", sub("tasks/t1")]);
    expect(gets(calls)).toBe(1);
    // Nothing changes: 5, then 10, 20 and 30 s apart, and never slower.
    const at: number[] = [];
    for (const ms of [5_000, 10_000, 20_000, 30_000, 30_000]) {
      await vi.advanceTimersByTimeAsync(ms - 1);
      at.push(gets(calls));
      await vi.advanceTimersByTimeAsync(1);
    }
    expect(at).toEqual([1, 2, 3, 4, 5]);
    expect(gets(calls)).toBe(6);
    expect(UNWATCHED_POLL_MS.at(-1)).toBe(POLL_MS);
    // The page's own write hurries it back to 5 s.
    await t.call("db", "set", ["tasks/t1", {}]);
    await vi.advanceTimersByTimeAsync(0);
    const n = gets(calls);
    await vi.advanceTimersByTimeAsync(UNWATCHED_POLL_MS[0]);
    expect(gets(calls)).toBe(n + 1);
    // A poll that finds something new keeps it at 5 s.
    v = 10;
    await vi.advanceTimersByTimeAsync(UNWATCHED_POLL_MS[1]);
    expect(gets(calls)).toBe(n + 2);
    await vi.advanceTimersByTimeAsync(UNWATCHED_POLL_MS[0]);
    expect(gets(calls)).toBe(n + 3);
    // A poll that found nothing new delivered nothing.
    expect(got.map(g => (g.data.docs as WireDoc[])[0].version)).toEqual([1, 2, 10]);
    await t.call("db", "unsubscribe", ["s1"]);
    await vi.advanceTimersByTimeAsync(POLL_MS * 4);
    expect(gets(calls)).toBe(n + 3);
  });

  it("polls nothing while the page is hidden, catches up as it comes back, and hurries on focus", async () => {
    vi.useFakeTimers();
    const state = { v: "visible" };
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => state.v });
    try {
      const { client, calls } = site(() => doc("tasks/t1", 1));
      const t = dbTransport(client, false);
      await t.call("db", "subscribe", ["s1", sub("tasks/t1")]);
      await vi.advanceTimersByTimeAsync(5_000 + 10_000 + 20_000);
      expect(gets(calls)).toBe(4);
      state.v = "hidden";
      document.dispatchEvent(new Event("visibilitychange"));
      await vi.advanceTimersByTimeAsync(POLL_MS * 10);
      expect(gets(calls)).toBe(4);
      state.v = "visible";
      document.dispatchEvent(new Event("visibilitychange"));
      await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS);
      expect(gets(calls)).toBe(5);
      await vi.advanceTimersByTimeAsync(UNWATCHED_POLL_MS[0]);
      expect(gets(calls)).toBe(6);
      // Slowed down again, a focus brings the next poll back to 5 s.
      await vi.advanceTimersByTimeAsync(10_000 + 20_000);
      expect(gets(calls)).toBe(8);
      dispatchEvent(new Event("focus"));
      await vi.advanceTimersByTimeAsync(UNWATCHED_POLL_MS[0]);
      expect(gets(calls)).toBe(9);
    } finally {
      delete (document as unknown as { visibilityState?: string }).visibilityState;
    }
  });

  it("follows a watching shell's reports instead of polling, and polls only while its stream is down", async () => {
    vi.useFakeTimers();
    let v = 1;
    const { client, calls } = site(m => (m === "query" ? [doc("tasks/a", v)] : doc("tasks/t1", v)));
    const sh = shell();
    const t = dbTransport(client, sh);
    const got = pushes(t);
    await vi.advanceTimersByTimeAsync(0);
    expect(sh.asked).toEqual(["db.watch"]);
    await t.call("db", "subscribe", ["q", tasks]);
    await t.call("db", "subscribe", ["d", sub("tasks/t1")]);
    await vi.advanceTimersByTimeAsync(POLL_MS * 3);
    expect(gets(calls)).toBe(2);
    v = 2;
    // A change elsewhere refetches nothing; one under the collection, the query; `{}`, everything.
    sh.push("changed", { path: "other/x" });
    sh.push("changed", { path: "tasks/b" });
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    expect(got.slice(2).map(g => g.data.sub)).toEqual(["q"]);
    sh.push("changed", {});
    await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    expect(gets(calls)).toBe(5);
    sh.push("stream", { up: false });
    await vi.advanceTimersByTimeAsync(POLL_MS + SNAPSHOT_DEBOUNCE_MS);
    expect(gets(calls)).toBe(7);
    sh.push("stream", { up: true });
    await vi.advanceTimersByTimeAsync(POLL_MS * 3);
    expect(gets(calls)).toBe(7);
  });

  it("polls when the frame's shell does not serve db", async () => {
    vi.useFakeTimers();
    const { client, calls } = site(() => doc("tasks/t1", 1));
    const sh = shell([]);
    const t = dbTransport(client, sh);
    await t.call("db", "subscribe", ["s1", sub("tasks/t1")]);
    await vi.advanceTimersByTimeAsync(UNWATCHED_POLL_MS[0]);
    expect(gets(calls)).toBe(2);
    expect(sh.asked).toEqual([]);
  });

  it("retries an unavailable refetch after 5 s, then every 30 s, and ends a refused one once", async () => {
    vi.useFakeTimers();
    let up = false;
    const { client, calls } = site(() => (up ? doc("tasks/t1", 1) : new CapabilityError("unavailable", "down")));
    const sh = shell();
    const t = dbTransport(client, sh);
    const got = pushes(t);
    await vi.advanceTimersByTimeAsync(0);
    await t.call("db", "subscribe", ["s1", sub("tasks/t1")]);
    await vi.advanceTimersByTimeAsync(RETRY_MS[0] - 1);
    expect(gets(calls)).toBe(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(gets(calls)).toBe(2);
    await vi.advanceTimersByTimeAsync(RETRY_MS[1]);
    expect(gets(calls)).toBe(3);
    up = true;
    await vi.advanceTimersByTimeAsync(RETRY_MS[1]);
    expect(got.map(g => g.topic)).toEqual(["snapshot"]);
    await vi.advanceTimersByTimeAsync(RETRY_MS[1] * 3);
    expect(gets(calls)).toBe(4);
    const refused = site(() => new CapabilityError("resource_exhausted", "over the budget"));
    const r = dbTransport(refused.client, sh);
    const ended = pushes(r);
    await r.call("db", "subscribe", ["s1", tasks]);
    expect(ended).toEqual([{ topic: "snapshot-error", data: { sub: "s1", code: "resource_exhausted", message: "over the budget" } }]);
    await vi.advanceTimersByTimeAsync(RETRY_MS[1] * 3);
    expect(gets(refused.calls)).toBe(1);
  });

  it("never delivers an older version of a document after a newer one, nor a slower earlier fetch", async () => {
    vi.useFakeTimers();
    const answers: unknown[] = [doc("tasks/t1", 3), doc("tasks/t1", 2), null, doc("tasks/t1", 5)];
    const { client } = site(() => answers.shift());
    const sh = shell();
    const t = dbTransport(client, sh);
    const got = pushes(t);
    await vi.advanceTimersByTimeAsync(0);
    await t.call("db", "subscribe", ["s1", sub("tasks/t1")]);
    for (let i = 0; i < 3; i++) {
      sh.push("changed", { path: "tasks/t1" });
      await vi.advanceTimersByTimeAsync(SNAPSHOT_DEBOUNCE_MS + 1);
    }
    expect(got.map(g => (g.data.docs as WireDoc[]).map(d => d.version))).toEqual([[3], [], [5]]);
  });

  it(`refuses the ${MAX_SUBSCRIPTIONS + 1}th subscription`, async () => {
    const { client } = site(() => doc("tasks/t1", 1));
    const t = dbTransport(client, false);
    for (let i = 0; i < MAX_SUBSCRIPTIONS; i++) await t.call("db", "subscribe", [`s${i}`, sub("tasks/t1")]);
    await expect(t.call("db", "subscribe", ["over", sub("tasks/t1")])).rejects.toMatchObject({ code: "resource_exhausted" });
    await t.call("db", "subscribe", ["s0", sub("tasks/t1")]);
    for (let i = 0; i < MAX_SUBSCRIPTIONS; i++) await t.call("db", "unsubscribe", [`s${i}`]);
  });
});

describe("db.artifact(id) on the page's own origin", () => {
  afterEach(() => { vi.useRealTimers(); });
  const B = "8r4m0nay3c5v";

  it("reads another app's documents through this site, never its origin", async () => {
    const { client, calls } = site(m => (m === "read_get" ? doc("runs/r1", 1) : m === "read_query" ? [doc("runs/r1", 1)] : null));
    const t = dbTransport(client, false);
    expect(await t.call("db", "read", [{ op: "get", artifact: B, path: "runs/r1" }])).toEqual(doc("runs/r1", 1));
    expect(await t.call("db", "read", [{ op: "query", artifact: B, spec: { ...tasks, collection: "runs" } }])).toEqual([doc("runs/r1", 1)]);
    expect(calls.map(c => [c[1], c[2][0]])).toEqual([["read_get", B], ["read_query", B]]);
    await expect(t.call("db", "read", [{ op: "set", artifact: B }])).rejects.toMatchObject({ code: "invalid_argument" });
  });

  it("asks the shell once on not_granted, and reads again when it grants", async () => {
    let granted = false;
    const { client } = site(() => (granted ? doc("runs/r1", 1) : new CapabilityError("not_granted", "no")));
    const sh = shell(["db"], { "db.grant": () => { granted = true; return true; } });
    const t = dbTransport(client, sh);
    expect(await t.call("db", "read", [{ op: "get", artifact: B, path: "runs/r1" }])).toEqual(doc("runs/r1", 1));
    expect(sh.asked.filter(a => a === "db.grant")).toHaveLength(1);
    // Refused without a shell: the refusal is the page's.
    granted = false;
    await expect(dbTransport(client, false).call("db", "read", [{ op: "get", artifact: B, path: "runs/r1" }])).rejects.toMatchObject({ code: "not_granted" });
  });

  it("polls a subscription to another app's documents even while the shell watches, and ends it when refused", async () => {
    vi.useFakeTimers();
    let v = 1;
    let refused = false;
    const { client } = site(m => (refused ? new CapabilityError("not_granted", "revoked") : m === "read_get" ? doc("runs/r1", v) : null));
    const sh = shell();
    const t = dbTransport(client, sh);
    const got = pushes(t);
    await vi.advanceTimersByTimeAsync(0);
    await t.call("db", "read", [{ op: "subscribe", artifact: B, sub: "s1", spec: sub("runs/r1") }]);
    v = 2;
    await vi.advanceTimersByTimeAsync(UNWATCHED_POLL_MS[0]);
    expect(got.map(g => g.topic === "snapshot" ? (g.data.docs as WireDoc[])[0].version : g.topic)).toEqual([1, 2]);
    refused = true;
    await vi.advanceTimersByTimeAsync(UNWATCHED_POLL_MS[0]);
    expect(got.at(-1)).toMatchObject({ topic: "snapshot-error", data: { sub: "s1", code: "not_granted" } });
  });
});

describe("db in the Clax app's migration view", () => {
  it("hands the shell each document's value before its first write, and writes once the journal has it", async () => {
    const order: string[] = [];
    const { client } = site((m, args) => { order.push(`${m} ${String(args[0])}`); return m === "get" ? (args[0] === "tasks/a" ? doc("tasks/a", 3) : null) : null; });
    const sh = shell(["db"], { "db.journal": args => { order.push(`journal ${JSON.stringify(args[0])}`); return null; } });
    const t = dbTransport(client, sh, true);
    await t.call("db", "set", ["tasks/a", { v: 9 }]);
    await t.call("db", "update", ["tasks/a", { v: 10 }]);
    await t.call("db", "delete", ["tasks/b"]);
    expect(order.filter(o => !o.startsWith("query"))).toEqual([
      "get tasks/a", `journal ${JSON.stringify({ path: "tasks/a", before: { v: 3 } })}`, "set tasks/a",
      "update tasks/a",
      "get tasks/b", `journal ${JSON.stringify({ path: "tasks/b", before: null })}`, "delete tasks/b",
    ]);
    // Outside the migration view nothing is journaled.
    const plain = site(() => null);
    const sh2 = shell(["db"]);
    await dbTransport(plain.client, sh2).call("db", "set", ["tasks/a", { v: 1 }]);
    expect(sh2.asked.filter(a => a === "db.journal")).toEqual([]);
  });
});

describe("user on the page's own origin", () => {
  const who = (w: Partial<Whoami> | null) => {
    const { client } = site(() => null);
    const s = siteOf(client);
    return { ...s, whoami: async () => (w ? ({ level: "view", can_edit: false, kind: "person", ...w } as Whoami) : null) };
  };
  const ns = (w: Partial<Whoami> | null, declared: Record<string, unknown> = { user: { scopes: ["profile"] } }, sh: ReturnType<typeof shell> | false = false) =>
    userLocals(who(w), declared as never, sh) as unknown as Record<string, (...a: unknown[]) => Promise<any>>;

  it("answers who the viewer is from the site's whoami, never from a shell token", async () => {
    const owner = ns({ kind: "owner", id: ME, name: "Alex", level: "admin", can_edit: true });
    expect([await owner.isOwner(), await owner.canEdit(), await owner.id(), await owner.name(), await owner.email()]).toEqual([true, true, ME, "Alex", null]);
    expect(await owner.me()).toMatchObject({ id: ME, name: "Alex", email: null, isOwner: true, canEdit: true });
    // A person whose rights let them publish can edit and is not the owner;
    // a guest is neither, whatever their browser holds.
    const editor = ns({ kind: "person", id: OTHER, name: "Sam", level: "interact", can_edit: true });
    expect([await editor.isOwner(), await editor.canEdit()]).toEqual([false, true]);
    const guest = ns({ kind: "guest", id: OTHER, level: "public", can_edit: false });
    expect([await guest.isOwner(), await guest.canEdit(), await guest.can("data.write")]).toEqual([false, false, false]);
    // A world visitor is their browser's visitor ID, their data/users/<id>.
    const visitor = ns({ kind: "visitor", id: "v_00000000000000000000cc", level: "public" }, { user: {} });
    expect([await visitor.id(), await visitor.name(), await visitor.avatarUrl()]).toEqual(["v_00000000000000000000cc", "", null]);
  });

  it("without the declaration only the universal members answer, and nothing rejects when whoami fails", async () => {
    const h = ns({ kind: "owner", id: ME, name: "Alex", level: "admin", can_edit: true }, {});
    expect([await h.isOwner(), await h.canEdit(), await h.can("files.write"), await h.id(), await h.name(), await h.avatarUrl()]).toEqual([true, true, true, null, "", null]);
    const down = ns(null);
    expect(await down.me()).toMatchObject({ id: null, name: "", isOwner: false, canEdit: false });
    // The site said nothing of who this is: can() is null, not "cannot" (user.d.ts).
    expect([await down.can("data.write"), await down.can("files.write"), await down.search("sa")]).toEqual([null, null, []]);
  });

  it("can() follows the caller's level against the declared root write rule", async () => {
    const rules = { user: {}, db: { rules: [{ path: "", write: "admin" }] } };
    expect(await ns({ level: "admin" }, rules).can("data.write")).toBe(true);
    expect(await ns({ level: "interact" }, rules).can("data.write")).toBe(false);
    expect(await ns({ level: "interact" }, { user: {} }).can("data.write")).toBe(true);
    expect(await ns({ level: "view" }, { user: {} }).can("data.write")).toBe(false);
    expect(await ns({ level: "public" }, { db: { rules: [{ path: "", write: "public" }] } }).can("data.write")).toBe(true);
    expect(await ns({ kind: "owner" }).can("assets.write")).toBe(true);
    expect(await ns({ kind: "person", can_edit: true }).can("assets.write")).toBe(false);
    expect(await ns({ kind: "owner" }).can("launch.rockets")).toBe(false);
    expect(rootWrite({})).toBe("interact");
  });

  it("asks a framing shell for other people's names, and names only the viewer without one", async () => {
    const alone = ns({ id: ME, name: "Alex" });
    const p = (await alone.profiles([OTHER, ME, OTHER, 7])) as Record<string, Record<string, unknown>>;
    expect(Object.keys(p).sort()).toEqual([ME, OTHER]);
    expect(p[ME]).toMatchObject({ name: "Alex", isMe: true, guest: false, email: null });
    expect(p[OTHER]).toMatchObject({ name: "", isMe: false });
    expect(await alone.search("sa")).toEqual([]);
    const sh = shell(["user"], { "user.profiles": () => ({ [OTHER]: { id: OTHER, name: "Sam" } }), "user.search": () => [{ id: OTHER, name: "Sam" }] });
    const framed = ns({ id: ME, name: "Alex" }, undefined, sh);
    expect(await framed.profiles([OTHER])).toEqual({ [OTHER]: { id: OTHER, name: "Sam" } });
    expect(await framed.search("sa")).toEqual([{ id: OTHER, name: "Sam" }]);
    expect(sh.asked).toEqual(["user.profiles", "user.search"]);
    // The viewer's own entry is a guest exactly when the site says they are one.
    const guest = ns({ kind: "guest", id: ME, name: "Gus", level: "public" });
    expect(((await guest.profiles([ME, OTHER])) as Record<string, { guest: boolean }>)[ME].guest).toBe(true);
    expect(((await guest.profiles([OTHER])) as Record<string, { guest: boolean }>)[OTHER].guest).toBe(false);
    const viaShell = shell(["user"], { "user.profiles": () => ({ [ME]: { id: ME, name: "Gus", guest: false }, [OTHER]: { id: OTHER, guest: false } }) });
    const framedGuest = (await ns({ kind: "guest", id: ME, level: "public" }, undefined, viaShell).profiles([ME, OTHER])) as Record<string, { guest: boolean }>;
    expect([framedGuest[ME].guest, framedGuest[OTHER].guest]).toEqual([true, false]);
    expect(((await alone.profiles([ME])) as Record<string, { guest: boolean }>)[ME].guest).toBe(false);
    // A shell that does not serve user (a frame no Clax shell answers) is never asked.
    const foreign = shell([]);
    expect(await ns({ id: ME }, undefined, foreign).search("sa")).toEqual([]);
    expect(foreign.asked).toEqual([]);
  });
});

describe("originUse", () => {
  const room = async () => ({ roomNamespace: (t: { call: unknown; on: unknown }) => ({ room: typeof t.call === "function" && typeof t.on === "function" }) }) as never;
  const at = (declared: unknown, w: Partial<Whoami> | null = { kind: "visitor", level: "public", can_edit: false }) =>
    siteOf(site(m => (m === "get" ? { capabilities: declared } : m === "whoami" ? (w ?? new CapabilityError("unavailable", "x")) : null)).client);

  it("resolves db and room when declared, assets for the owner alone, user always, and undefined for the shell's", async () => {
    const declared = { db: {}, room: {}, assets: {} };
    expect(Object.keys((await originUse("db", false, room, at(declared))) as object).sort()).toEqual(["artifact", "collection", "doc"]);
    // The room's namespace over the origin's socket.
    expect(await originUse("room", false, room, at(declared))).toEqual({ room: true });
    expect(await originUse("assets", false, room, at(declared))).toBeNull();
    expect(Object.keys((await originUse("assets", false, room, at(declared, { kind: "owner", level: "admin", can_edit: true }))) as object).sort()).toEqual(["delete", "list", "upload"]);
    for (const n of ["db", "room", "assets"]) expect(await originUse(n, false, room, at({}, { kind: "owner", level: "admin", can_edit: true }))).toBeNull();
    const user = (await originUse("user", false, room, at({}))) as Record<string, unknown>;
    expect(Object.isFrozen(user)).toBe(true);
    expect(await (user.isOwner as () => Promise<boolean>)()).toBe(false);
    for (const n of ["permissions", "artifact", "comments", "downloads", "sample"]) expect(await originUse(n, false, room, at(declared))).toBeUndefined();
  });

  it("reports a refused session once, framed, only while the browser has not granted storage access", async () => {
    const refused = () => site(m => (m === "get" ? { capabilities: { db: {} } } : m === "whoami" ? new CapabilityError("unauthenticated", "no site session") : null)).client;
    const access = (granted: boolean | undefined) => Object.defineProperty(document, "hasStorageAccess", { value: granted === undefined ? undefined : async () => granted, configurable: true });
    try {
      for (const [granted, want] of [[false, 1], [undefined, 1], [true, 0]] as const) {
        access(granted);
        let told = 0;
        const s = siteOf(refused(), () => { told++; });
        // The first use() asks who the viewer is, whatever it names.
        await originUse("db", false, room, s);
        await s.whoami();
        expect(told, `hasStorageAccess ${granted}`).toBe(want);
      }
      // Any other failure is not a refused session.
      access(false);
      let told = 0;
      const down = siteOf(site(m => (m === "whoami" ? new CapabilityError("unavailable", "x") : null)).client, () => { told++; });
      expect(await down.whoami()).toBeNull();
      expect(told).toBe(0);
    } finally {
      delete (document as { hasStorageAccess?: unknown }).hasStorageAccess;
    }
  });

  it("resolves only user when the declaration cannot be read", async () => {
    const s = siteOf(site(() => new CapabilityError("unavailable", "down")).client);
    expect(await originUse("db", false, room, s)).toBeNull();
    expect(await originUse("user", false, room, s)).not.toBeNull();
  });
});
