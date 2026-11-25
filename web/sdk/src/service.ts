// A deployer's backend (spec 2026-10-27 §5.3): it holds a queue service
// grant in its secrets and mints reporter tokens for its own users; a
// preview deployment's backend also attaches its origin to the fix it
// carries, with that fix's ticket. Runs
// in Node 20 or later, a Worker or any runtime with WebCrypto Ed25519, as
// an ES module or, through the package's `require` condition, CommonJS; it
// is never shipped to a page, which would hand out the grant.
import type { PreviewAttached, PreviewAttachment, ReporterProfile, ReporterToken, ReporterTokenRequest } from "./types";
import { BACKOFF_S, checkInstance, ClaxError, errorCode, queuePath, readJson, sendSigned, Timing, type Fetch } from "./client";
import { fromBase64url, importPkcs8Signer, type Signer } from "./sign";

export { ClaxError } from "./client";
export { verifyWebhook, webhookChallengeAnswer, webhookSignature, WebhookError, WEBHOOK_TOLERANCE_S, type VerifyWebhookOptions, type WebhookErrorReason, type WebhookHeaders } from "./webhook";
export type { ClaxErrorCode, FixTicket, PreviewAttached, PreviewAttachment, QueueErrorCode, ReporterProfile, ReporterToken, ReporterTokenRequest, SdkErrorCode, ReportWebhookEvent, WebhookComment, WebhookEvent, WebhookEventType, WebhookFix, WebhookReport, WebhookVerifyEvent, WebhookVerifyReply } from "./types";

/** A fix ticket's parts: `cxf_<queue ID>.<report ID>.<fix number>.<secret>`. */
const TICKET = /^cxf_([^./]+)\.([^./]+)\.([1-9]\d*)\.[\w-]+$/;

export interface ServiceOptions {
  /** The instance's origin, such as `https://clax.example.com`: Clax is
   * served at the root of its own host, so a URL with a path, query or
   * fragment is refused with a `TypeError`. */
  instance: string;
  /** The queue's ID. */
  queue: string;
  /** The service grant's token, `cxq_…`. */
  token: string;
  /** The service grant's `private_key`: PKCS#8, unpadded base64url. */
  privateKey: string;
  /** Default: the global `fetch`. */
  fetch?: Fetch;
  /** Milliseconds since the Unix epoch. Default: `Date.now`. */
  now?: () => number;
  /** Waits out a retry's backoff (429 `limit_reached`, 503 `busy`).
   * Default: `setTimeout`. */
  sleep?: (ms: number) => Promise<void>;
  /** A number in [0, 1) that scales a backoff's jitter. Default: `Math.random`. */
  random?: () => number;
}

/** Calls a queue's service routes with its service grant. Each call is
 * retried as `docs/contract.md` ("Signed requests") says: once on 401
 * `stale_request` with the instance's clock, and up to three times on 429
 * `limit_reached` and 503 `busy`, after 1, 2 and 4 seconds. */
export class ServiceClient {
  private signer: Promise<Signer> | undefined;
  private readonly timing: Timing;

  constructor(private readonly opts: ServiceOptions) {
    checkInstance(opts.instance, "ServiceClient");
    this.timing = new Timing(opts.now ?? Date.now, opts.sleep, opts.random);
  }

  /** Mints a reporter token for `r.reporter_id`, bound to the key
   * `r.public_key` and the page's exact origin `r.origin` (one the queue
   * lists, or one of its patterns matches):
   * `POST /api/queues/<queue>/reporter-tokens`. `profile` (else
   * `r.profile`) is who this backend says the reporter is: each report
   * filed under the token keeps it, shown to triage members and never to
   * reporters; 400 `invalid_profile` for a field over 256 bytes or an
   * `avatar_url` that is not http(s). */
  async mintReporterToken(r: ReporterTokenRequest, profile: ReporterProfile | undefined = r.profile): Promise<ReporterToken> {
    const req: ReporterTokenRequest = { reporter_id: r.reporter_id, origin: r.origin, public_key: r.public_key };
    if (profile !== undefined) req.profile = profile;
    const body = new TextEncoder().encode(JSON.stringify(req));
    this.signer ??= importPkcs8Signer(fromBase64url(this.opts.privateKey));
    const res = await sendSigned({
      method: "POST",
      instance: this.opts.instance,
      path: queuePath(this.opts.queue, "reporter-tokens"),
      body,
      contentType: "application/json",
      credential: { scheme: "Clax-Queue", token: this.opts.token, signer: await this.signer },
      fetch: this.opts.fetch ?? ((input, init) => globalThis.fetch(input, init)),
    }, this.timing);
    return readJson<ReporterToken>(res);
  }

  /** Attaches this preview deployment to the fix its ticket was issued
   * for (`a.ticket`, the `CLAX_FIX_TICKET` the fixing agent set): the
   * fix's `verify_url` becomes `a.origin` and `a.path` (default `/`), and
   * the reporter is told a preview of the fix is ready:
   * `POST /api/queues/<queue>/reports/<report>/fixes/<n>/preview`. Take
   * `a.origin` from the deployment's own configuration, never from a
   * request. The ticket is the call's only credential (this client's
   * grant is not used), and its only right: 403 `wrong_origin` for an
   * origin none of the queue's patterns matches, 401 `token_expired`
   * once the fix has its verdict or 14 days have passed. Attaching the
   * URL the fix names already changes nothing (`changed: false`), so a
   * redeploy may call it again. Retried on 429 `limit_reached` and 503
   * `busy` as the signed calls are; a `TypeError` for a ticket of
   * another queue, or that is no ticket. */
  async attachPreview(a: PreviewAttachment): Promise<PreviewAttached> {
    const m = TICKET.exec(a.ticket.trim());
    if (!m) throw new TypeError("attachPreview: ticket is not a fix ticket (cxf_<queue>.<report>.<n>.<secret>)");
    const [, queue, report, n] = m;
    if (queue !== this.opts.queue) throw new TypeError(`attachPreview: the ticket is of queue ${queue}, not ${this.opts.queue}`);
    const body: { origin: string; path?: string } = { origin: a.origin };
    if (a.path !== undefined) body.path = a.path;
    const url = new URL(queuePath(queue, "reports", report, "fixes", n, "preview"), this.opts.instance).href;
    const fetch = this.opts.fetch ?? ((input, init) => globalThis.fetch(input, init));
    const init: RequestInit = {
      method: "POST",
      headers: { authorization: `Clax-Fix ${a.ticket.trim()}`, "content-type": "application/json" },
      body: JSON.stringify(body),
      credentials: "omit",
    };
    for (let backoffs = 0; ; backoffs++) {
      let res: Response;
      try {
        res = await fetch(url, init);
      } catch (e) {
        throw new ClaxError("network", e instanceof Error ? e.message : String(e), 0);
      }
      const code = res.status === 429 || res.status === 503 ? await errorCode(res) : "";
      const retry = (res.status === 429 && code === "limit_reached") || (res.status === 503 && code === "busy");
      if (!retry || backoffs >= BACKOFF_S.length) return readJson<PreviewAttached>(res);
      const base = BACKOFF_S[backoffs] * 1000;
      await this.timing.sleep(base + base * Math.min(Math.max(this.timing.random(), 0), 1) / 4);
    }
  }
}
