import { describe, expect, it } from "vitest";
import { indexedDbStore, type Kept, type ReporterStore } from "./keystore";
import { ReporterClient, RENEW_BEFORE_S } from "./reporter";
import type { ReporterToken } from "./types";

const T0 = Date.parse("2026-10-29T12:00:00Z");
const TOKEN_S = 900;

/** A store kept in memory, as IndexedDB keeps one across page loads; it
 * records what was saved, and fails every call once `broken` is set. */
function memoryStore() {
  let kept: Kept | null = null;
  const s = {
    broken: false,
    saves: 0,
    get kept() { return kept; },
    store: {
      load: async () => { if (s.broken) throw new Error("no IndexedDB"); return kept; },
      save: async (k: Kept) => { if (s.broken) throw new Error("no IndexedDB"); s.saves += 1; kept = k; },
      forgetToken: async () => { if (s.broken) throw new Error("no IndexedDB"); if (kept) kept = { privateKey: kept.privateKey, publicKey: kept.publicKey }; },
    } satisfies ReporterStore,
  };
  return s;
}

/** A page load's client on `store`, at `now()`, answering 200 (or `status`). */
function pageLoad(store: ReporterStore | null, now: () => number, auths: string[], status: () => number = () => 200) {
  let n = auths.length;
  const auth = async (pk: string): Promise<ReporterToken> => {
    auths.push(pk);
    n += 1;
    return { token: `cxr_${n}`, expires_at: new Date(now() + TOKEN_S * 1000).toISOString() };
  };
  const sent: string[] = [];
  const fetch = async (_: string, init: RequestInit) => {
    sent.push((init.headers as Record<string, string>).authorization);
    const st = status();
    return new Response(JSON.stringify(st === 200 ? { waiting: 0, report_ids: [] } : { error: { code: "token_expired", message: "expired" } }), { status: st, headers: { "content-type": "application/json" } });
  };
  return { sent, client: new ReporterClient({ instance: "https://clax.example", queue: "q1", auth, fetch, now, store }) };
}

describe("a reporter's key and token across page loads", () => {
  it("mints once per token lifetime, not per page load, with one key", async () => {
    const m = memoryStore();
    let now = T0;
    const auths: string[] = [];
    const first = pageLoad(m.store, () => now, auths);
    await first.client.badge();
    now += 60_000;
    const second = pageLoad(m.store, () => now, auths);
    await second.client.badge();
    expect(auths).toHaveLength(1);
    expect(second.sent).toEqual(["Clax-Reporter cxr_1"]);
    expect(await second.client.publicKey()).toBe(await first.client.publicKey());
    // The key is kept as a CryptoKey that cannot be exported.
    expect(m.kept?.privateKey.extractable).toBe(false);
    await expect(crypto.subtle.exportKey("pkcs8", m.kept!.privateKey)).rejects.toThrow();
    // Shortly before it expires, a page load mints again, for the same key.
    now = T0 + (TOKEN_S - RENEW_BEFORE_S) * 1000;
    const third = pageLoad(m.store, () => now, auths);
    await third.client.badge();
    expect(auths).toHaveLength(2);
    expect(auths[1]).toBe(auths[0]);
    expect(m.kept?.token?.token).toBe("cxr_2");
  });

  it("forgets a refused token and keeps the key", async () => {
    const m = memoryStore();
    const auths: string[] = [];
    let refuse = 1;
    const load = pageLoad(m.store, () => T0, auths, () => (refuse-- > 0 ? 401 : 200));
    await load.client.badge();
    expect(load.sent).toEqual(["Clax-Reporter cxr_1", "Clax-Reporter cxr_2"]);
    expect(m.kept?.token?.token).toBe("cxr_2");
    const next = pageLoad(m.store, () => T0, auths);
    await next.client.badge();
    expect(auths).toHaveLength(2);
  });

  it("keeps the key and token for the page alone when the store fails", async () => {
    const m = memoryStore();
    m.broken = true;
    const auths: string[] = [];
    const load = pageLoad(m.store, () => T0, auths);
    await load.client.badge();
    await load.client.badge();
    expect(auths).toHaveLength(1);
    expect(load.sent).toEqual(["Clax-Reporter cxr_1", "Clax-Reporter cxr_1"]);
    // A page load after it mints again: nothing was kept.
    await pageLoad(m.store, () => T0, auths).client.badge();
    expect(auths).toHaveLength(2);
  });

  it("without a store, every page load mints", async () => {
    const auths: string[] = [];
    await pageLoad(null, () => T0, auths).client.badge();
    await pageLoad(null, () => T0, auths).client.badge();
    expect(auths).toHaveLength(2);
    expect(auths[0]).not.toBe(auths[1]);
  });

  it("uses IndexedDB only for a named user, and none where there is none", () => {
    expect(indexedDbStore("https://clax.example", "q1", "u1", undefined)).toBeNull();
    const c = new ReporterClient({ instance: "https://clax.example", queue: "q1", auth: async () => ({ token: "x", expires_at: "2099-01-01T00:00:00Z" }) });
    expect((c as unknown as { store: unknown }).store).toBeNull();
  });
});
