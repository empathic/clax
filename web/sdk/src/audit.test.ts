import { describe, expect, it } from "vitest";
import { AuditClient, auditQuery } from "./audit";
import type { AuditEvent, AuditPage } from "./contract.gen";

const ev = (seq: number): AuditEvent => ({
  seq, at: "2026-11-02T00:00:00Z", actor: { id: "*owner", name: null }, on_behalf_of: null,
  kind: "artifact.publish", summary: `event ${seq}`, artifact_id: null, run: null,
});
const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

/** A fake instance: its events (newest first in pages of `limit`) and the
 * URLs and headers asked. */
function instance(events: () => AuditEvent[], limit = 3) {
  const asked: { url: string; auth: string | null }[] = [];
  const fetch = async (input: string, init: RequestInit) => {
    const url = new URL(input, "http://clax.test");
    asked.push({ url: url.pathname + url.search, auth: (init.headers as Record<string, string>).authorization ?? null });
    if (url.pathname.startsWith("/api/audit/")) return json(404, { error: { code: "not_found", message: "no such event" } });
    const cursor = Number(url.searchParams.get("cursor") ?? Infinity);
    const all = events().filter(e => e.seq < cursor).sort((a, b) => b.seq - a.seq);
    const page: AuditPage = { events: all.slice(0, limit), next_cursor: all.length > limit ? all[limit - 1].seq : null };
    return json(200, page);
  };
  return { asked, fetch };
}

describe("AuditClient", () => {
  it("asks the filters given, with the token", async () => {
    const i = instance(() => [ev(1)]);
    const c = new AuditClient({ instance: "http://clax.test", token: "tok", fetch: i.fetch });
    await c.list({ kind: "bot.*", from: "2026-11-01", limit: 5, actor: undefined });
    expect(i.asked).toEqual([{ url: "/api/audit?kind=bot.*&from=2026-11-01&limit=5", auth: "Bearer tok" }]);
    expect(auditQuery({})).toBe("");
  });

  it("sends no token without one, and turns a refusal into a ClaxError", async () => {
    const i = instance(() => []);
    const c = new AuditClient({ fetch: i.fetch });
    await expect(c.event(9)).rejects.toMatchObject({ code: "not_found", status: 404 });
    expect(i.asked[0]).toEqual({ url: "/api/audit/9", auth: null });
  });

  it("follows new events oldest first, asking one query, paging back only for a whole new page", async () => {
    let events = [ev(1), ev(2)];
    const i = instance(() => events);
    const c = new AuditClient({ instance: "http://clax.test", fetch: i.fetch });
    const stop = new AbortController();
    const got: number[] = [];
    const run = (async () => {
      for await (const e of c.follow({ kind: "artifact.publish" }, { intervalMs: 1, signal: stop.signal })) {
        got.push(e.seq);
        if (got.length === 1) events = [...events, ...[4, 5, 6, 7].map(ev)];
        if (got.length === 5) stop.abort();
      }
    })();
    // The first poll after the start finds 3.
    events = [...events, ev(3)];
    await run;
    expect(got).toEqual([3, 4, 5, 6, 7]);
    const polls = i.asked.filter(a => !a.url.includes("cursor"));
    expect(new Set(polls.map(a => a.url))).toEqual(new Set(["/api/audit?kind=artifact.publish"]));
    // 4 to 7 came in one poll, a whole page and more: one page back.
    expect(i.asked.filter(a => a.url.includes("cursor"))).toHaveLength(1);
  });
});
