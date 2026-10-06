import { describe, expect, it } from "vitest";
import { Api, ApiFailure } from "./api";
import { PairError, type Pairing } from "./pairing";

const A: Pairing = { daemon: "http://localhost:7480", credential: `cxe_${"A".repeat(43)}`, claxVersion: "0.9.0" };
const B: Pairing = { daemon: "http://localhost:7481", credential: `cxe_${"B".repeat(43)}`, claxVersion: "0.9.0" };
const AID = "7q3k9mzx2b4t";
const TID = "01J9ZQ3V7K8M2N4P6R8T0V2X4Y";

function setup(results: (Response | Error)[], pairer?: { current(): Promise<Pairing>; pair(): Promise<Pairing> }) {
  const calls: { url: string; init: RequestInit }[] = [];
  let pairs = 0;
  const p = pairer ?? { current: async () => A, pair: async () => { pairs++; return B; } };
  const fetchFn = (async (url: string, init: RequestInit) => {
    calls.push({ url, init });
    const r = results.shift()!;
    if (r instanceof Error) throw r;
    return r;
  }) as unknown as typeof fetch;
  return { api: new Api(p, fetchFn), calls, pairs: () => pairs };
}
const ok = (body: unknown = { page: null, route: null }) => new Response(JSON.stringify(body), { status: 200 });
const auth = (init: RequestInit) => new Headers(init.headers).get("authorization");

describe("Api", () => {
  it("sends the credential and never cookies", async () => {
    const s = setup([ok()]);
    await s.api.lookup("http://localhost:5173/");
    expect(s.calls[0].url).toBe("http://localhost:7480/api/live/pages?url=http%3A%2F%2Flocalhost%3A5173%2F");
    expect(auth(s.calls[0].init)).toBe(`Clax-Extension ${A.credential}`);
    expect(s.calls[0].init.credentials).toBe("omit");
  });

  it("re-pairs once on 401 and on a network error", async () => {
    let s = setup([new Response("{}", { status: 401 }), ok()]);
    await s.api.lookup("http://localhost:5173/");
    expect(s.pairs()).toBe(1);
    expect(s.calls[1].url.startsWith(B.daemon)).toBe(true);
    expect(auth(s.calls[1].init)).toBe(`Clax-Extension ${B.credential}`);
    s = setup([new TypeError("Failed to fetch"), ok()]);
    await s.api.lookup("http://localhost:5173/");
    expect(s.pairs()).toBe(1);
    s = setup([new Response("{}", { status: 401 }), new Response(JSON.stringify({ error: { code: "unknown_credential", message: "pair" } }), { status: 401 })]);
    await expect(s.api.lookup("http://x/")).rejects.toEqual(new ApiFailure("unknown_credential", "pair", 401));
    expect(s.pairs()).toBe(1);
  });

  it("never sends a comment or a send twice after a network error, but pairs again for the next request", async () => {
    const s = setup([new TypeError("Failed to fetch"), new TypeError("Failed to fetch"), ok({ thread: {} })]);
    await expect(s.api.comment(AID, TID, "hi")).rejects.toMatchObject({ code: "daemon_unreachable" });
    expect(s.calls).toHaveLength(1);
    expect(s.pairs()).toBe(1);
    // A new thread names its pick: it is sent again, and the daemon makes one thread of both.
    const f = new FormData();
    f.set("pick_id", "0".repeat(32));
    await s.api.postThread(f, []);
    expect(s.calls.slice(1).map(c => (c.init.body as FormData).get("pick_id"))).toEqual(["0".repeat(32), "0".repeat(32)]);
  });

  it("runs what waits for no request in flight once the last one is answered", async () => {
    let answer!: (r: Response) => void;
    const fetchFn = (() => new Promise<Response>(r => { answer = r; })) as unknown as typeof fetch;
    const api = new Api({ current: async () => A, pair: async () => B }, fetchFn);
    const ran: string[] = [];
    api.whenIdle(() => ran.push("idle before"));
    expect(ran).toEqual(["idle before"]);
    const req = api.lookup("http://localhost:5173/");
    await Promise.resolve();
    await Promise.resolve();
    api.whenIdle(() => ran.push("idle after"));
    expect(ran).toEqual(["idle before"]);
    answer(ok());
    await req;
    await new Promise(r => setTimeout(r, 0));
    expect(ran).toEqual(["idle before", "idle after"]);
  });

  it("tells its listener of a new pairing", async () => {
    const s = setup([new Response("{}", { status: 401 }), ok()]);
    const heard: Pairing[] = [];
    s.api.onRepair = p => heard.push(p);
    await s.api.lookup("http://localhost:5173/");
    expect(heard).toEqual([B]);
  });

  it("tells its listener once of a pairing that several requests share", async () => {
    let release!: (p: Pairing) => void;
    const shared = new Promise<Pairing>(r => { release = r; });
    let pairs = 0;
    const pairer = { current: async () => A, pair: () => { pairs++; return shared; } };
    const s = setup([new Response("{}", { status: 401 }), new Response("{}", { status: 401 }), ok(), ok()], pairer);
    const heard: Pairing[] = [];
    s.api.onRepair = p => heard.push(p);
    const both = Promise.all([s.api.lookup("http://localhost:5173/"), s.api.lookup("http://localhost:5173/b")]);
    await new Promise(r => setTimeout(r, 0));
    release(B);
    await both;
    expect(pairs).toBe(2);
    expect(heard).toEqual([B]);
  });

  it("retries with a pairing another request already renewed, without pairing again", async () => {
    let stored = A;
    let pairs = 0;
    const pairer = { current: async () => stored, pair: async () => { pairs++; throw new PairError("paired_recently", "wait"); } };
    const s = setup([new Response("{}", { status: 401 }), ok()], pairer);
    const p = s.api.lookup("http://localhost:5173/");
    stored = B;
    await p;
    expect(pairs).toBe(0);
    expect(auth(s.calls[1].init)).toBe(`Clax-Extension ${B.credential}`);
  });

  it("answers the daemon's 401 when pairing again is refused", async () => {
    const pairer = { current: async () => A, pair: async () => { throw new PairError("paired_recently", "wait"); } };
    const s = setup([new Response(JSON.stringify({ error: { code: "unknown_credential", message: "pair" } }), { status: 401 })], pairer);
    await expect(s.api.lookup("http://x/")).rejects.toEqual(new ApiFailure("unknown_credential", "pair", 401));
  });

  it("reports an unreachable daemon when pairing again is refused", async () => {
    const pairer = { current: async () => A, pair: async () => { throw new PairError("paired_recently", "wait"); } };
    const s = setup([new TypeError("Failed to fetch")], pairer);
    await expect(s.api.lookup("http://x/")).rejects.toMatchObject({ code: "daemon_unreachable" });
  });

  it("reports a pairing failure as the host's error", async () => {
    const pairer = { current: async () => { throw new PairError("host_missing", "Specified native messaging host not found."); }, pair: async () => A };
    const s = setup([], pairer);
    await expect(s.api.lookup("http://x/")).rejects.toMatchObject({ code: "host_missing" });
  });

  it("refuses artifact and thread IDs that are not IDs", async () => {
    const s = setup([]);
    await expect(s.api.threads("../token")).rejects.toMatchObject({ code: "invalid_id" });
    await expect(s.api.resolve("7q3k9mzx2b4t", "x/../y")).rejects.toMatchObject({ code: "invalid_id" });
    await expect(s.api.sendBatch(AID, [TID, "nope"], null, null)).rejects.toMatchObject({ code: "invalid_id" });
    expect(s.calls).toHaveLength(0);
  });

  it("reports the owner here for a window's panel, and never away", async () => {
    const s = setup([ok({})]);
    await s.api.presence(AID, 12);
    expect(s.calls[0].url).toBe(`${A.daemon}/api/viewers/me/presence`);
    expect(s.calls[0].init.method).toBe("PUT");
    expect(JSON.parse(String(s.calls[0].init.body))).toEqual({ artifact_id: AID, state: "here", tab: "clax-ext:12" });
  });

  it("reads who is on a live page", async () => {
    const s = setup([ok({ people: [] })]);
    expect(await s.api.presenceOf(AID)).toEqual({ people: [] });
    expect(s.calls[0].url).toBe(`${A.daemon}/api/artifacts/${AID}/presence`);
    await expect(s.api.presenceOf("../x")).rejects.toMatchObject({ code: "invalid_id" });
  });

  it("names the pending threads a snapshot covers, an empty list when none", async () => {
    const s = setup([ok({}), ok({}), new Response("{}", { status: 401 }), ok({})]);
    const form = () => { const f = new FormData(); f.set("url", "http://localhost:5173/"); return f; };
    await s.api.postThread(form(), []);
    await s.api.postSnapshot(form(), [TID]);
    expect(s.calls[0].url).toBe(`${A.daemon}/api/live/threads`);
    expect((s.calls[0].init.body as FormData).get("pending")).toBe("[]");
    expect(s.calls[1].url).toBe(`${A.daemon}/api/live/snapshots`);
    expect((s.calls[1].init.body as FormData).get("pending")).toBe(JSON.stringify([TID]));
    // A form survives the retry after a re-pair.
    await s.api.postThread(form(), [TID]);
    expect((s.calls[3].init.body as FormData).get("pending")).toBe(JSON.stringify([TID]));
    await expect(s.api.postSnapshot(form(), ["x"])).rejects.toMatchObject({ code: "invalid_id" });
  });

  it("reads the daemon's error, or the status when it has none", async () => {
    const s = setup([new Response("oops", { status: 502, statusText: "Bad Gateway" })]);
    await expect(s.api.me()).rejects.toEqual(new ApiFailure("http_502", "Bad Gateway", 502));
  });
});
