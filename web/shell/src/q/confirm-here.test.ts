import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api";
import { browserCall, Refused } from "../held";
import { answerQuestion } from "./api";
import { passkeySettings } from "./passkey";

const ID = "cf_0123456789abcdef0123456789abcdef";
const bytes = (...b: number[]) => new Uint8Array(b).buffer;
const held = [202, { status: "confirmation_pending", confirmation_id: ID, confirm_url: `http://localhost:7480/inbox?confirm=${ID}` }];
const settings = (required: boolean) => ({
  available: true, unavailable_reason: null, rp_id: "localhost", origin: "http://localhost:7480",
  require_passkey: required, required, offer_dismissed: false, passkeys: required ? [{ credential_id: "AQ", alg: -7, created_at: "2026-10-30T10:00:00.000Z" }] : [],
});
const challenge = { challenge: "AQID", rp_id: "localhost", user_id: "CQ", timeout_ms: 600000, allow: ["AQ"] };
const question = { id: "q1", status: "answered" };

type Call = { method: string; url: string; body: unknown; confirmation: string | null };
/** Answers each call by `routes` (`[status, body]` or a body), keeping its confirmation header. */
function stub(routes: Record<string, (c: Call) => unknown>): Call[] {
  const calls: Call[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit = {}) => {
    const h = (init.headers ?? {}) as Record<string, string>;
    const c = { method: init.method ?? "GET", url, body: init.body ? JSON.parse(String(init.body)) : undefined, confirmation: h["x-clax-confirmation"] ?? null };
    calls.push(c);
    const r = routes[`${c.method} ${url}`]?.(c);
    if (r === undefined) return new Response(JSON.stringify({ error: { code: "not_found", message: "not found" } }), { status: 404 });
    if (Array.isArray(r) && typeof r[0] === "number") return new Response(JSON.stringify(r[1]), { status: r[0] });
    return new Response(JSON.stringify(r));
  }));
  return calls;
}

let get: ReturnType<typeof vi.fn>;
beforeEach(() => {
  vi.stubGlobal("isSecureContext", true);
  vi.stubGlobal("PublicKeyCredential", {});
  get = vi.fn(async () => ({ rawId: bytes(1), response: { clientDataJSON: bytes(5), authenticatorData: bytes(7), signature: bytes(8) } }));
  Object.defineProperty(navigator, "credentials", { configurable: true, value: { get } });
});
afterEach(() => { vi.unstubAllGlobals(); });

describe("a decision this page made, held for confirmation", () => {
  it("is confirmed here with the passkey and made again, finishing it with no card", async () => {
    const calls = stub({
      "POST /api/questions/q1/answer": c => c.confirmation === ID ? { question } : held,
      "GET /api/passkeys": () => settings(true),
      "POST /api/passkeys/challenge": () => challenge,
      [`POST /api/confirmations/${ID}`]: () => ({ confirmation: { id: ID, status: "approved" } }),
    });
    await passkeySettings(true);
    expect(await answerQuestion("q1", { answers: [] } as never)).toEqual({ question });
    expect(get).toHaveBeenCalledTimes(1);
    expect(calls.filter(c => c.method === "POST").map(c => [c.url, c.body, c.confirmation])).toEqual([
      ["/api/questions/q1/answer", { answers: [] }, null],
      ["/api/passkeys/challenge", { purpose: "confirm", confirmation_id: ID }, null],
      [`/api/confirmations/${ID}`, { approve: true, assertion: { credential_id: "AQ", authenticator_data: "Bw", client_data_json: "BQ", signature: "CA" } }, null],
      ["/api/questions/q1/answer", { answers: [] }, ID],
    ]);
  });

  it("a passkey not used declines it and says so", async () => {
    get.mockResolvedValue(null);
    const calls = stub({
      "PUT /api/people/p1": () => held,
      "GET /api/passkeys": () => settings(true),
      "POST /api/passkeys/challenge": () => challenge,
      [`POST /api/confirmations/${ID}`]: () => ({ confirmation: { id: ID, status: "declined" } }),
    });
    await expect(browserCall("/api/people/p1", "PUT", { rights: {} })).rejects.toThrow("Not done: confirming it takes your passkey, and none was used.");
    expect(calls.filter(c => c.url === `/api/confirmations/${ID}`).map(c => c.body)).toEqual([{ approve: false }]);
    expect(calls.filter(c => c.url === "/api/people/p1")).toHaveLength(1);
  });

  it("without a required passkey, it says where to confirm, as before", async () => {
    stub({ "POST /api/questions/q1/answer": () => held, "GET /api/passkeys": () => settings(false) });
    await passkeySettings(true);
    const e = await answerQuestion("q1", { answers: [] } as never).catch(x => x);
    expect(e).toBeInstanceOf(ApiError);
    expect(e.code).toBe("confirmation_pending");
    expect(get).not.toHaveBeenCalled();
  });

  it("a call made with its confirmation, or asked to stay held, is not confirmed again", async () => {
    stub({ "POST /api/x": () => held, "GET /api/passkeys": () => settings(true) });
    for (const [confirm, hold] of [[ID, false], [undefined, true]] as const) {
      const e = await browserCall("/api/x", "POST", {}, confirm, hold).catch(x => x);
      expect(e).toBeInstanceOf(Refused);
      expect((e as Refused).held?.id).toBe(ID);
    }
    expect(get).not.toHaveBeenCalled();
  });
});
