import { describe, expect, it } from "vitest";
import { BACKOFF_S, checkInstance, ClaxError, failure, MAX_OFFSET_S, sendSigned, Timing } from "./client";
import { generateSigner } from "./sign";

const T0 = Date.parse("2026-10-27T12:00:00Z");

const json = (status: number, v: unknown, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify(v), { status, headers: { "content-type": "application/json", ...headers } });
const refusal = (status: number, code: string, headers: Record<string, string> = {}) => json(status, { error: { code, message: code } }, headers);

/** A fake instance answering `answers` in turn, recording each request's
 * signed time, and a timing whose sleeps are recorded, not waited. */
async function setup(answers: Response[], random = 0) {
  const ts: number[] = [];
  const slept: number[] = [];
  let now = T0;
  const timing = new Timing(() => now, async ms => { slept.push(ms); now += ms; }, () => random);
  const signer = await generateSigner();
  const send = () => sendSigned({
    method: "GET", instance: "https://clax.example", path: "/api/queues/q1/reporter/badge",
    credential: { scheme: "Clax-Reporter", token: "cxr_1", signer },
    fetch: async (_url, init) => {
      ts.push(Number(/^t=(\d+),/.exec((init.headers as Record<string, string>)["clax-signature"])![1]));
      const next = answers.shift();
      if (!next) throw new Error("no more answers");
      return next;
    },
  }, timing);
  return { ts, slept, timing, send };
}

describe("checkInstance", () => {
  it("takes an origin, with or without its trailing slash", () => {
    for (const ok of ["https://clax.example.com", "https://clax.example.com/", "http://localhost:7481", "https://[::1]:8443"]) {
      expect(() => checkInstance(ok, "Test"), ok).not.toThrow();
    }
  });

  it("refuses a path, a query, a fragment, a user, another scheme or no URL, naming the caller", () => {
    for (const bad of ["https://apps.example.com/clax", "https://apps.example.com/clax/", "https://clax.example.com//", "https://clax.example.com?q=1", "https://clax.example.com/?", "https://clax.example.com#x", "https://u:p@clax.example.com", "ftp://clax.example.com", "clax.example.com", ""]) {
      expect(() => checkInstance(bad, "Test"), bad).toThrow(TypeError);
      expect(() => checkInstance(bad, "Test"), bad).toThrow(`Test: instance must be an origin, such as https://clax.example.com, not ${JSON.stringify(bad)}`);
    }
  });
});

describe("sendSigned", () => {
  it("retries limit_reached and busy three times after 1, 2 and 4 seconds, each signed anew", async () => {
    const s = await setup([refusal(429, "limit_reached"), refusal(503, "busy"), refusal(429, "limit_reached"), json(200, { waiting: 0 })]);
    const res = await s.send();
    expect(res.status).toBe(200);
    expect(s.slept).toEqual(BACKOFF_S.map(b => b * 1000));
    // Each attempt is signed at its own time.
    expect(s.ts).toEqual([T0 / 1000, T0 / 1000 + 1, T0 / 1000 + 3, T0 / 1000 + 7]);
  });

  it("adds up to a quarter of each backoff as jitter", async () => {
    const s = await setup([refusal(429, "limit_reached"), json(200, {})], 0.5);
    await s.send();
    expect(s.slept).toEqual([1125]);
  });

  it("gives up after the third retry with the last answer", async () => {
    const s = await setup([1, 2, 3, 4].map(() => refusal(503, "busy")));
    const res = await s.send();
    expect(res.status).toBe(503);
    expect(s.ts).toHaveLength(4);
    expect(s.slept).toHaveLength(3);
  });

  it("does not retry another 429 or 503", async () => {
    for (const r of [refusal(429, "rate_limited"), refusal(503, "feedback_unavailable"), new Response("down", { status: 503 })]) {
      const s = await setup([r]);
      expect((await s.send()).status).toBe(r.status);
      expect(s.slept).toEqual([]);
    }
  });

  it("re-signs once on stale_request at the instance's time, and keeps that offset", async () => {
    const ahead = new Date(T0 + 300_000).toUTCString();
    const s = await setup([refusal(401, "stale_request", { date: ahead }), json(200, {}), json(200, {})]);
    expect((await s.send()).status).toBe(200);
    expect(s.ts).toEqual([T0 / 1000, T0 / 1000 + 300]);
    expect(s.timing.offsetMs).toBe(300_000);
    await s.send();
    expect(s.ts[2]).toBe(T0 / 1000 + 300);
  });

  it("answers stale_request when the instance's clock is too far off, unreadable, or stale again", async () => {
    const far = new Date(T0 + (MAX_OFFSET_S + 1) * 1000).toUTCString();
    for (const answers of [
      [refusal(401, "stale_request", { date: far })],
      [refusal(401, "stale_request")],
      [refusal(401, "stale_request", { date: new Date(T0 + 200_000).toUTCString() }), refusal(401, "stale_request", { date: new Date(T0 + 400_000).toUTCString() })],
    ]) {
      const n = answers.length;
      const s = await setup(answers);
      const res = await s.send();
      expect(res.status).toBe(401);
      expect(s.ts).toHaveLength(n);
    }
  });

  it("leaves the last answer's body unread", async () => {
    const s = await setup([refusal(401, "invalid_signature")]);
    const res = await s.send();
    expect(await res.json()).toEqual({ error: { code: "invalid_signature", message: "invalid_signature" } });
  });
});

describe("failure", () => {
  it("carries the code, message, status and Retry-After in seconds", async () => {
    const e = await failure(refusal(429, "limit_reached", { "retry-after": "30" }));
    expect(e).toBeInstanceOf(ClaxError);
    expect(e).toMatchObject({ code: "limit_reached", message: "limit_reached", status: 429, retryAfterS: 30 });
  });

  it("takes the error's retry_after_s without a Retry-After, and has none when neither names a wait", async () => {
    expect((await failure(json(429, { error: { code: "rate_limited", message: "m", retry_after_s: 12 } }))).retryAfterS).toBe(12);
    expect(await failure(refusal(400, "invalid_args"))).not.toHaveProperty("retryAfterS");
    expect(await failure(refusal(429, "limit_reached", { "retry-after": "soon" }))).not.toHaveProperty("retryAfterS");
  });

  it("keeps the status of an answer without the error shape", async () => {
    expect(await failure(new Response("no", { status: 502, statusText: "Bad Gateway" }))).toMatchObject({ code: "http_502", message: "Bad Gateway", status: 502 });
  });
});
