import { describe, expect, it } from "vitest";
import { QuietGate, RELOAD_GUARD_MS, type Source, Updater, buildOf, readOwn } from "./update";

/** A source the test makes busy and idle. */
function source() {
  let busy = false;
  const idlers: (() => void)[] = [];
  const s: Source & { set(b: boolean): void; waiting: () => number } = {
    busy: () => busy,
    whenIdle: fn => { idlers.push(fn); if (!busy) for (const f of idlers.splice(0)) f(); },
    set: b => { busy = b; if (!b) for (const f of idlers.splice(0)) f(); },
    waiting: () => idlers.length,
  };
  return s;
}

describe("QuietGate", () => {
  it("runs at once when nothing is busy", () => {
    let ran = 0;
    new QuietGate([source(), source()]).when(() => ran++);
    expect(ran).toBe(1);
  });

  it("waits until every source is idle at the same moment, checking all again when one goes idle", () => {
    const a = source(), b = source();
    const g = new QuietGate([a, b]);
    a.set(true); b.set(true);
    let ran = 0;
    g.when(() => ran++);
    a.set(false);
    expect(ran).toBe(0);
    // a is busy again before b goes idle: still not quiet.
    a.set(true);
    b.set(false);
    expect(ran).toBe(0);
    a.set(false);
    expect(ran).toBe(1);
  });

  it("keeps one function waiting, the latest, and arms each source once", () => {
    const a = source();
    const g = new QuietGate([a]);
    a.set(true);
    const ran: string[] = [];
    g.when(() => ran.push("first"));
    g.when(() => ran.push("second"));
    g.when(() => ran.push("third"));
    expect(a.waiting()).toBe(1);
    a.set(false);
    expect(ran).toEqual(["third"]);
    expect(g.quiet()).toBe(true);
  });
});

const LOADED = { version: "0.6.0", version_name: "0.6.0 (aaaaaaaaaaaa)" };
const manifest = (version_name?: string, extra: Record<string, unknown> = {}) =>
  JSON.stringify({ manifest_version: 3, version: "0.6.0", ...(version_name === undefined ? {} : { version_name }), background: { service_worker: "sw.js", type: "module" }, ...extra });

function updater(files: Record<string, string | null>, opts: { busy?: Source; now?: number } = {}) {
  const store: Record<string, unknown> = {};
  const reloads: number[] = [];
  let now = opts.now ?? 1_000_000;
  const gate = new QuietGate(opts.busy ? [opts.busy] : []);
  let reads = 0;
  const u = new Updater({
    loaded: LOADED,
    read: async p => { reads++; return files[p] ?? null; },
    local: { get: async k => ({ [k]: store[k] }), set: async v => { Object.assign(store, v); } },
    gate,
    reload: () => reloads.push(now),
    now: () => now,
  });
  return { u, reloads, store, files, gate, reads: () => reads, tick: (ms: number) => { now += ms; } };
}
const settle = async () => { for (let i = 0; i < 10; i++) await Promise.resolve(); };

describe("Updater", () => {
  it("does nothing while the files on disk are the build running", async () => {
    const t = updater({ "manifest.json": manifest(LOADED.version_name), "sw.js": "x" });
    expect(await t.u.check()).toBeNull();
    await settle();
    expect(t.reloads).toEqual([]);
    expect(t.u.newer).toBeNull();
  });

  it("reloads for another build on disk, once, and not again for it within the guard", async () => {
    const t = updater({ "manifest.json": manifest("0.6.0 (bbbbbbbbbbbb)"), "sw.js": "x" });
    expect(await t.u.check()).toBe("0.6.0 (bbbbbbbbbbbb)");
    await settle();
    expect(t.reloads).toHaveLength(1);
    expect(t.store.reloadedForBuild).toMatchObject({ build: "0.6.0 (bbbbbbbbbbbb)" });
    // The reload did not take (the same worker still runs): no loop.
    expect(await t.u.check()).toBeNull();
    await settle();
    expect(t.reloads).toHaveLength(1);
    // Past the guard it is asked again.
    t.tick(RELOAD_GUARD_MS);
    expect(await t.u.check()).toBe("0.6.0 (bbbbbbbbbbbb)");
    await settle();
    expect(t.reloads).toHaveLength(2);
  });

  it("reloads for a build from before stamps (a rollback): its version is its build", async () => {
    const t = updater({ "manifest.json": manifest(), "sw.js": "x" });
    expect(await t.u.check()).toBe("0.6.0");
    await settle();
    expect(t.reloads).toHaveLength(1);
  });

  it("does not reload for files that do not look whole: no manifest, one that does not parse, another manifest version, or no service worker", async () => {
    for (const files of [
      {},
      { "manifest.json": "{\"manifest_version\": 3,", "sw.js": "x" },
      { "manifest.json": manifest("0.6.0 (bbbbbbbbbbbb)", { manifest_version: 2 }), "sw.js": "x" },
      { "manifest.json": manifest("0.6.0 (bbbbbbbbbbbb)"), "sw.js": null },
      { "manifest.json": manifest("0.6.0 (bbbbbbbbbbbb)"), "sw.js": "" },
      { "manifest.json": manifest("0.6.0 (bbbbbbbbbbbb)", { background: {} }), "sw.js": "x" },
      { "manifest.json": "null", "sw.js": "x" },
    ] as Record<string, string | null>[]) {
      const t = updater(files);
      expect(await t.u.check()).toBeNull();
      await settle();
      expect(t.reloads).toEqual([]);
    }
  });

  it("waits for a quiet moment, checking again after the guard is written", async () => {
    const busy = source();
    busy.set(true);
    const t = updater({ "manifest.json": manifest("0.6.0 (bbbbbbbbbbbb)"), "sw.js": "x" }, { busy });
    await t.u.check();
    await settle();
    expect(t.reloads).toEqual([]);
    busy.set(false);
    // The guard is being written; something opens meanwhile.
    busy.set(true);
    await settle();
    expect(t.reloads).toEqual([]);
    busy.set(false);
    expect(t.reloads).toHaveLength(1);
  });

  it("shares one look among concurrent checks", async () => {
    const t = updater({});
    await Promise.all([t.u.check(), t.u.check(), t.u.check()]);
    expect(t.reads()).toBe(1);
    await t.u.check();
    expect(t.reads()).toBe(2);
  });
});

describe("buildOf and readOwn", () => {
  it("names a manifest's stamp, else its version", () => {
    expect(buildOf(LOADED)).toBe("0.6.0 (aaaaaaaaaaaa)");
    expect(buildOf({ version: "0.6.0" })).toBe("0.6.0");
  });

  it("reads a file without the cache, and answers null for a failure or an error status", async () => {
    const asked: [string, RequestInit | undefined][] = [];
    const ok = (async (u: string, init?: RequestInit) => { asked.push([u, init]); return new Response("body"); }) as typeof fetch;
    expect(await readOwn("chrome-extension://x/manifest.json", ok)).toBe("body");
    expect(asked).toEqual([["chrome-extension://x/manifest.json", { cache: "no-store" }]]);
    expect(await readOwn("u", (async () => new Response("", { status: 404 })) as typeof fetch)).toBeNull();
    expect(await readOwn("u", (async () => { throw new TypeError("gone"); }) as typeof fetch)).toBeNull();
  });
});
