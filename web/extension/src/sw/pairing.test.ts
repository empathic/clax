import { describe, expect, it } from "vitest";
import { PAIR_SLOW_MS, PairError, Pairer, REPAIR_MS, type PairEnv } from "./pairing";

const CRED = `cxe_${"A".repeat(43)}`;
function env(replies: unknown[], version = "0.9.0") {
  const store: Record<string, unknown> = {};
  const local: Record<string, unknown> = {};
  let now = 0;
  const e = {
    sent: [] as unknown[], reloads: 0, store,
    tick(ms: number) { now += ms; },
    sendNative: async (_h: string, m: object) => { e.sent.push(m); return replies.shift(); },
    session: { get: async (k: string) => (k in store ? { [k]: store[k] } : {}), set: async (v: Record<string, unknown>) => { Object.assign(store, v); }, remove: async (k: string) => { delete store[k]; } },
    local: { get: async (k: string) => (k in local ? { [k]: local[k] } : {}), set: async (v: Record<string, unknown>) => { Object.assign(local, v); }, remove: async (k: string) => { delete local[k]; } },
    manifestVersion: version,
    reload: () => { e.reloads++; },
    now: () => now,
  };
  return e;
}
const paired = (daemon = "http://localhost:7480", v = "0.9.0") => ({ type: "paired", v: 1, daemon, credential: CRED, clax_version: v });

describe("Pairer", () => {
  it("says when the native host is slow to answer, keeps waiting for it, and pairs on its late answer, with one host for a Retry too", async () => {
    let answer!: (v: unknown) => void;
    const timers: { at: number; fn: () => void; off: boolean }[] = [];
    let clock = 0;
    const advance = (ms: number) => { clock += ms; for (const t of timers) if (!t.off && t.at <= clock) { t.off = true; t.fn(); } };
    const e = env([]);
    const sent: unknown[] = [];
    const slowEnv = {
      ...e,
      sendNative: (_h: string, m: object) => { sent.push(m); return new Promise(r => { answer = r; }); },
      after: (ms: number, fn: () => void) => { const t = { at: clock + ms, fn, off: false }; timers.push(t); return () => { t.off = true; }; },
    };
    const p = new Pairer(slowEnv as unknown as PairEnv);
    const told: boolean[] = [];
    p.onSlow = s => told.push(s);
    const first = p.pair();
    advance(PAIR_SLOW_MS - 1);
    expect(told).toEqual([]);
    advance(1);
    expect(told).toEqual([true]);
    // The person's Retry waits for the same pairing: no second native host.
    const retry = p.pair(true);
    expect(retry).toBe(first);
    expect(sent).toHaveLength(1);
    // The host answers late: the pairing is kept, and the slowness is over.
    answer(paired());
    expect((await first).daemon).toBe("http://localhost:7480");
    expect(e.store.pairing).toMatchObject({ daemon: "http://localhost:7480" });
    expect(told).toEqual([true, false]);
  });

  it("says nothing of slowness for a pairing answered in time, and stops its timer", async () => {
    const timers: { fn: () => void; off: boolean }[] = [];
    const e = env([paired()]);
    const p = new Pairer({ ...e, after: (_ms: number, fn: () => void) => { const t = { fn, off: false }; timers.push(t); return () => { t.off = true; }; } } as unknown as PairEnv);
    const told: boolean[] = [];
    p.onSlow = s => told.push(s);
    await p.pair();
    expect(timers.every(t => t.off)).toBe(true);
    expect(told).toEqual([]);
  });

  it("pairs once and keeps the pairing in session storage", async () => {
    const e = env([paired()]);
    const p = new Pairer(e as unknown as PairEnv);
    expect((await p.current()).daemon).toBe("http://localhost:7480");
    expect((await p.current()).credential).toBe(CRED);
    expect(e.sent).toEqual([{ type: "pair", v: 1, extension_version: "0.9.0" }]);
  });

  it("shares one pairing between concurrent callers and refuses another within REPAIR_MS", async () => {
    const e = env([paired(), paired("http://localhost:7481")]);
    const p = new Pairer(e as unknown as PairEnv);
    const [a, b] = await Promise.all([p.pair(), p.pair()]);
    expect(a).toEqual(b);
    await expect(p.pair()).rejects.toMatchObject({ code: "paired_recently" });
    e.tick(REPAIR_MS);
    expect((await p.pair()).daemon).toBe("http://localhost:7481");
  });

  it("reports the host's errors and refuses replies that are not a pairing", async () => {
    const e = env([{ type: "error", v: 1, code: "daemon_unavailable", message: "see the log" }, { ...paired(), daemon: "http://evil.example:80" }]);
    const p = new Pairer(e as unknown as PairEnv);
    await expect(p.pair()).rejects.toEqual(new PairError("daemon_unavailable", "see the log"));
    e.tick(REPAIR_MS);
    await expect(p.pair()).rejects.toMatchObject({ code: "bad_reply" });
  });

  it("takes only a daemon on localhost or 127.0.0.1 over http, with a port", async () => {
    const good = ["http://localhost:7480", "http://127.0.0.1:1", "http://localhost:65535"];
    const bad = ["http://192.168.1.4:7480", "http://0.0.0.0:7480", "https://localhost:7480", "http://localhost", "http://localhost:7480/",
      "http://localhost:0", "http://localhost:65536", "http://localhost:7480@evil.example", "http://localhost.evil.example:7480", "http://[::1]:7480"];
    for (const daemon of good) {
      const e = env([paired(daemon)]);
      expect((await new Pairer(e as unknown as PairEnv).pair()).daemon).toBe(daemon);
    }
    for (const daemon of bad) {
      const e = env([paired(daemon)]);
      await expect(new Pairer(e as unknown as PairEnv).pair(), daemon).rejects.toMatchObject({ code: "bad_reply" });
      expect(e.store.pairing).toBeUndefined();
    }
  });

  it("refuses a malformed credential, a missing version and another protocol version", async () => {
    for (const reply of [{ ...paired(), credential: "cxe_short" }, { ...paired(), clax_version: 9 }, { ...paired(), v: 2 }, null, "paired"]) {
      const e = env([reply]);
      await expect(new Pairer(e as unknown as PairEnv).pair()).rejects.toMatchObject({ code: "bad_reply" });
    }
  });

  it("forgets the pairing, so the next caller pairs again", async () => {
    const e = env([paired(), paired("http://localhost:7481")]);
    const p = new Pairer(e as unknown as PairEnv);
    await p.current();
    await p.forget();
    e.tick(REPAIR_MS);
    expect((await p.current()).daemon).toBe("http://localhost:7481");
  });

  it("pairs again at once when asked to retry, inside REPAIR_MS", async () => {
    const e = env([{ type: "error", v: 1, code: "daemon_unavailable", message: "see the log" }, paired()]);
    const p = new Pairer(e as unknown as PairEnv);
    await expect(p.pair()).rejects.toMatchObject({ code: "daemon_unavailable" });
    await expect(p.pair()).rejects.toMatchObject({ code: "paired_recently" });
    expect((await p.pair(true)).daemon).toBe("http://localhost:7480");
  });

  it("reloads the extension once for a daemon of another version", async () => {
    const e = env([paired(undefined, "1.0.0"), paired(undefined, "1.0.0")]);
    const p = new Pairer(e as unknown as PairEnv);
    await p.pair();
    e.tick(REPAIR_MS);
    await p.pair();
    expect(e.reloads).toBe(1);
  });

  it("does not reload for a daemon of the same numeric version", async () => {
    const e = env([paired(undefined, "0.9.0-dev+abc")]);
    await new Pairer(e as unknown as PairEnv).pair();
    expect(e.reloads).toBe(0);
  });
});
