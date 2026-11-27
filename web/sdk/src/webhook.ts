// Checking a report queue's webhook delivery (docs/contract.md, "Queue
// webhooks"; crates/clax-core/src/webhook.rs): `Clax-Signature: v1=<hex>`
// is the HMAC-SHA256, under the endpoint's secret, of `<timestamp>.<body>`
// with `Clax-Timestamp` the send's Unix time in seconds. An endpoint
// answers its verification (`webhook.verify`) with `{"answer": <hex
// HMAC-SHA256 of "webhook.verify.<challenge>">}`. On WebCrypto alone, so
// it runs in Node 20, a Worker or a browser.
import type { WebhookEvent, WebhookVerifyReply } from "./types";

/** How far a delivery's timestamp may be from `now`, in seconds. */
export const WEBHOOK_TOLERANCE_S = 300;

/** Why `verifyWebhook` refused a delivery. */
export type WebhookErrorReason = "bad_timestamp" | "stale" | "bad_signature" | "bad_body";

/** A delivery that is not Clax's, or not now. */
export class WebhookError extends Error {
  constructor(public reason: WebhookErrorReason, message: string) {
    super(message);
    this.name = "WebhookError";
  }
}

/** Request headers as a `Request` (`Headers`) or Node's `req.headers` gives them. */
export type WebhookHeaders = Headers | Record<string, string | string[] | undefined>;

export interface VerifyWebhookOptions {
  /** The endpoint's secret, `whsec_…`, shown once when it was made. */
  secret: string;
  headers: WebhookHeaders;
  /** The request body exactly as received: its bytes, or their UTF-8 text. */
  body: string | Uint8Array;
  /** Milliseconds since the Unix epoch. Default: `Date.now()`. */
  now?: number;
  /** Default: {@link WEBHOOK_TOLERANCE_S}. */
  toleranceS?: number;
}

const utf8 = new TextEncoder();

function header(h: WebhookHeaders, name: string): string | undefined {
  if (typeof (h as Headers).get === "function") return (h as Headers).get(name) ?? undefined;
  const r = h as Record<string, string | string[] | undefined>;
  const key = Object.keys(r).find(k => k.toLowerCase() === name.toLowerCase());
  const v = key === undefined ? undefined : r[key];
  return Array.isArray(v) ? v.join(", ") : v;
}

function hex(bytes: ArrayBuffer): string {
  return [...new Uint8Array(bytes)].map(b => b.toString(16).padStart(2, "0")).join("");
}

/** Equal strings, compared without stopping at the first difference. */
function same(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let d = 0;
  for (let i = 0; i < a.length; i++) d |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return d === 0;
}

async function hmac(secret: string, msg: BufferSource): Promise<string> {
  const key = await crypto.subtle.importKey("raw", utf8.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return hex(await crypto.subtle.sign("HMAC", key, msg));
}

/** `Clax-Signature`'s value for `body` sent at Unix time `timestamp` under `secret`. */
export async function webhookSignature(secret: string, timestamp: number, body: string | Uint8Array): Promise<string> {
  const bytes = typeof body === "string" ? utf8.encode(body) : body;
  const prefix = utf8.encode(`${timestamp}.`);
  const msg = new Uint8Array(prefix.length + bytes.length);
  msg.set(prefix);
  msg.set(bytes, prefix.length);
  return `v1=${await hmac(secret, msg)}`;
}

/** The answer to a verification's `challenge` under `secret`: the hex
 * HMAC-SHA256 of `webhook.verify.<challenge>`. An HMAC, not the challenge,
 * so that a URL echoing its request back verifies nothing. */
export async function webhookChallengeAnswer(secret: string, challenge: string): Promise<string> {
  return hmac(secret, utf8.encode(`webhook.verify.${challenge}`));
}

/** Checks a webhook delivery and answers its event: `Clax-Timestamp`
 * within `toleranceS` of `now`, and a `v1=` value of `Clax-Signature`
 * equal to {@link webhookSignature} of the body as received. Throws
 * {@link WebhookError} otherwise. Verify the raw body before parsing it:
 * a body parsed and serialised again no longer matches.
 *
 * A `webhook.verify` event comes with its `reply` set: answer it 2xx with
 * that as the JSON body (`res.json(event.reply)`), and the endpoint is
 * verified; Clax sends it no report event before. */
export async function verifyWebhook(o: VerifyWebhookOptions): Promise<WebhookEvent> {
  const ts = header(o.headers, "clax-timestamp")?.trim();
  if (!ts || !/^-?\d+$/.test(ts)) throw new WebhookError("bad_timestamp", "no Clax-Timestamp, or one that is no integer");
  const t = Number(ts);
  const now = Math.floor((o.now ?? Date.now()) / 1000);
  if (Math.abs(now - t) > (o.toleranceS ?? WEBHOOK_TOLERANCE_S)) throw new WebhookError("stale", `Clax-Timestamp ${t} is too far from now (${now})`);
  const want = await webhookSignature(o.secret, t, o.body);
  const given = (header(o.headers, "clax-signature") ?? "").split(",").map(v => v.trim()).filter(v => v.startsWith("v1="));
  if (!given.some(v => same(v, want))) throw new WebhookError("bad_signature", "no Clax-Signature matches the body under this secret");
  const text = typeof o.body === "string" ? o.body : new TextDecoder().decode(o.body);
  let event: WebhookEvent;
  try {
    event = JSON.parse(text) as WebhookEvent;
  } catch {
    throw new WebhookError("bad_body", "the signed body is not JSON");
  }
  if (event?.type === "webhook.verify") {
    if (typeof event.challenge !== "string") throw new WebhookError("bad_body", "a webhook.verify event without a challenge");
    const reply: WebhookVerifyReply = { answer: await webhookChallengeAnswer(o.secret, event.challenge) };
    event.reply = reply;
  }
  return event;
}
