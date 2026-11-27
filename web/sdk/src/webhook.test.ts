import { describe, expect, it } from "vitest";
import { verifyWebhook, webhookChallengeAnswer, WebhookError, webhookSignature, WEBHOOK_TOLERANCE_S } from "./webhook";
import type { ReportWebhookEvent } from "./types";

const SECRET = "whsec_test";
const T = 1_700_000_000;
const EVENT: ReportWebhookEvent = {
  id: "evt_01JDX",
  type: "report.fix_proposed",
  created_at: "2026-11-24T12:00:00.000Z",
  queue: { id: "q1", name: "App reports" },
  report: {
    id: "t1", state: "fix_proposed", external_id: "invoice:9", artifact_id: "abcdefghjkmn", title: "Invoice 9",
    page_url: "https://app.example.com/invoices/9", link: "https://clax.example.com/queues/q1/t1",
    reporter_id: "emp-1", reporter: { name: "Ana", email: "ana@example.com" }, reopen_count: 0, created_at: "2026-11-24T11:00:00.000Z",
  },
  fix: { n: 1, summary: "Totals include tax", url: "https://github.com/acme/app/pull/7", verify_url: "https://app-pr-7.example.com/", outcome: "pending", proposer_name: "claude" },
};

async function signed(body: string, t = T) {
  return { "Clax-Timestamp": String(t), "Clax-Signature": await webhookSignature(SECRET, t, body), "Content-Type": "application/json" };
}

async function refusal(p: Promise<unknown>): Promise<string> {
  try {
    await p;
  } catch (e) {
    expect(e).toBeInstanceOf(WebhookError);
    return (e as WebhookError).reason;
  }
  throw new Error("verified");
}

describe("verifyWebhook", () => {
  // The vector crates/clax-core/src/webhook.rs checks too: HMAC-SHA256 of
  // `1700000000.{"a":1}` under `whsec_test`.
  it("signs as Clax does", async () => {
    expect(await webhookSignature(SECRET, T, '{"a":1}')).toBe("v1=38877139021993b830af32feea6e18a8da83eb2f6e49ee50bd9e4cf4ca4d3789");
    expect(await webhookSignature(SECRET, T, new TextEncoder().encode('{"a":1}'))).toBe(await webhookSignature(SECRET, T, '{"a":1}'));
  });

  it("answers the typed event of a fresh delivery signed with the secret", async () => {
    const body = JSON.stringify(EVENT);
    const ev = await verifyWebhook({ secret: SECRET, headers: await signed(body), body, now: T * 1000 });
    if (ev.type === "webhook.verify") throw new Error("a report event");
    expect(ev.type).toBe("report.fix_proposed");
    expect(ev.fix?.verify_url).toBe("https://app-pr-7.example.com/");
    expect(ev.report.reporter?.email).toBe("ana@example.com");
  });

  it("reads Headers and Node's lowercase header objects alike, and takes the body's bytes", async () => {
    const body = JSON.stringify(EVENT);
    const h = await signed(body);
    const node = Object.fromEntries(Object.entries(h).map(([k, v]) => [k.toLowerCase(), v]));
    const bytes = new TextEncoder().encode(body);
    for (const headers of [new Headers(h), node]) {
      expect((await verifyWebhook({ secret: SECRET, headers, body: bytes, now: T * 1000 })).id).toBe("evt_01JDX");
    }
  });

  it("takes any matching v1 signature of several", async () => {
    const body = "{}";
    const h = await signed(body);
    h["Clax-Signature"] = `v1=00, ${h["Clax-Signature"]}`;
    expect(await verifyWebhook({ secret: SECRET, headers: h, body, now: T * 1000 })).toEqual({});
  });

  it("refuses another secret, another body, a stale or missing timestamp, and a body that is not JSON", async () => {
    const body = JSON.stringify(EVENT);
    const h = await signed(body);
    expect(await refusal(verifyWebhook({ secret: "whsec_other", headers: h, body, now: T * 1000 }))).toBe("bad_signature");
    expect(await refusal(verifyWebhook({ secret: SECRET, headers: h, body: body.replace("Ana", "Eve"), now: T * 1000 }))).toBe("bad_signature");
    expect(await refusal(verifyWebhook({ secret: SECRET, headers: { ...h, "Clax-Timestamp": String(T + 1) }, body, now: T * 1000 }))).toBe("bad_signature");
    expect(await refusal(verifyWebhook({ secret: SECRET, headers: h, body, now: (T + WEBHOOK_TOLERANCE_S + 1) * 1000 }))).toBe("stale");
    expect((await verifyWebhook({ secret: SECRET, headers: h, body, now: (T + WEBHOOK_TOLERANCE_S) * 1000 })).id).toBe("evt_01JDX");
    expect(await refusal(verifyWebhook({ secret: SECRET, headers: { "Clax-Signature": h["Clax-Signature"] }, body, now: T * 1000 }))).toBe("bad_timestamp");
    expect(await refusal(verifyWebhook({ secret: SECRET, headers: await signed("not json"), body: "not json", now: T * 1000 }))).toBe("bad_body");
  });
});

describe("a verification", () => {
  const VERIFY = {
    id: "evt_01JDY",
    type: "webhook.verify",
    created_at: "2026-11-24T12:00:00.000Z",
    queue: { id: "q1", name: "App reports" },
    webhook: { id: "wh1", url: "https://app.example.com/clax/webhook" },
    challenge: "abc",
  };

  // The vector crates/clax-core/src/webhook.rs checks too: HMAC-SHA256 of
  // `webhook.verify.abc` under `whsec_test`.
  it("is answered with the challenge's HMAC under the secret", async () => {
    expect(await webhookChallengeAnswer(SECRET, "abc")).toBe("c9fd1999ad0c5c7fac4ddf087306ac90eb210ef229afe14db35b7b6e4e938344");
  });

  it("comes back from verifyWebhook with the reply to send, in one call", async () => {
    const body = JSON.stringify(VERIFY);
    const ev = await verifyWebhook({ secret: SECRET, headers: await signed(body), body, now: T * 1000 });
    if (ev.type !== "webhook.verify") throw new Error("a verification");
    expect(ev.webhook.id).toBe("wh1");
    expect(ev.reply).toEqual({ answer: await webhookChallengeAnswer(SECRET, "abc") });
    // Another secret's reply differs: only the secret's holder answers rightly.
    expect(ev.reply.answer).not.toBe(await webhookChallengeAnswer("whsec_other", "abc"));
  });

  it("is refused unsigned, under another secret, stale, or without a challenge", async () => {
    const body = JSON.stringify(VERIFY);
    const h = await signed(body);
    expect(await refusal(verifyWebhook({ secret: SECRET, headers: { "Clax-Timestamp": String(T) }, body, now: T * 1000 }))).toBe("bad_signature");
    expect(await refusal(verifyWebhook({ secret: SECRET, headers: {}, body, now: T * 1000 }))).toBe("bad_timestamp");
    expect(await refusal(verifyWebhook({ secret: "whsec_other", headers: h, body, now: T * 1000 }))).toBe("bad_signature");
    expect(await refusal(verifyWebhook({ secret: SECRET, headers: h, body, now: (T + WEBHOOK_TOLERANCE_S + 1) * 1000 }))).toBe("stale");
    const none = JSON.stringify({ ...VERIFY, challenge: undefined });
    expect(await refusal(verifyWebhook({ secret: SECRET, headers: await signed(none), body: none, now: T * 1000 }))).toBe("bad_body");
  });
});
