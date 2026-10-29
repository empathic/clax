// A reporter's client (spec 2026-10-27 §5.3): a deployer's page files
// reports into a queue and reads the replies, with a reporter token its own
// backend mints for a key that never leaves the page.
import type { Report, ReportPage, ReporterToken } from "./types";
import { ClaxError, failure, multipart, queuePath, readJson, signedFetch, type Fetch, type Part } from "./client";
import { base64url, generateSigner, type KeyedSigner } from "./sign";

/** Seconds before a token's `expires_at` that the client asks for a new one. */
export const RENEW_BEFORE_S = 60;

/** Gets a reporter token for the key `publicKey` (unpadded base64url): the
 * deployer's own call to its backend, which knows the reporter from its
 * own session and calls `POST /api/queues/<queue>/reporter-tokens` with the
 * queue's service grant (`ServiceClient.mintReporterToken`). */
export type ReporterAuth = (publicKey: string) => Promise<ReporterToken>;

export interface ReporterOptions {
  /** The instance's base URL, such as `https://clax.corp.example`. */
  instance: string;
  /** The queue's ID. */
  queue: string;
  auth: ReporterAuth;
  /** Default: the global `fetch`. */
  fetch?: Fetch;
  /** Milliseconds since the Unix epoch. Default: `Date.now`. */
  now?: () => number;
  /** The reporter's key. Default: a new non-extractable one, made on first
   * use; a browser without WebCrypto Ed25519 fails every call with
   * `ClaxError` code `unsupported_browser`. */
  signer?: KeyedSigner;
}

/** A report to file. The reporter is the token's, never the body's. */
export interface NewReport {
  /** The page's artifact; without one the instance uses the page URL without its route. */
  externalId?: string;
  url: string;
  title: string;
  /** Where on the page the report points (the bridge's anchor). */
  anchor: unknown;
  body: string;
  metadata?: unknown;
  revision?: string;
  /** A PNG of what the report points at. */
  clip?: Blob | Uint8Array;
}

/** Files and reads reports as one reporter.
 *
 * Each call carries a reporter token: the client asks `auth` for one on
 * first use and again from [`RENEW_BEFORE_S`] seconds before it expires,
 * and on a 401 asks once more and sends the call again once. Calls made
 * while a token is being fetched share that one `auth` call. */
export class ReporterClient {
  private readonly fetch: Fetch;
  private readonly now: () => number;
  private signer: Promise<KeyedSigner> | undefined;
  private token: { token: string; expiresAt: number } | undefined;
  private pending: Promise<{ token: string; expiresAt: number }> | undefined;

  constructor(private readonly opts: ReporterOptions) {
    this.fetch = opts.fetch ?? ((input, init) => globalThis.fetch(input, init));
    this.now = opts.now ?? Date.now;
    if (opts.signer) this.signer = Promise.resolve(opts.signer);
  }

  /** Files `r` into the queue: `POST …/reporter/reports`. */
  async fileReport(r: NewReport): Promise<Report> {
    const parts: [string, Part][] = [];
    if (r.externalId !== undefined) parts.push(["external_id", r.externalId]);
    parts.push(["url", r.url], ["title", r.title], ["anchor", JSON.stringify(r.anchor ?? null)], ["body", r.body]);
    if (r.metadata !== undefined) parts.push(["metadata", JSON.stringify(r.metadata)]);
    if (r.revision !== undefined) parts.push(["revision", r.revision]);
    if (r.clip !== undefined) {
      const bytes = r.clip instanceof Uint8Array ? r.clip : new Uint8Array(await r.clip.arrayBuffer());
      parts.push(["clip", { bytes, filename: "clip.png", type: "image/png" }]);
    }
    const boundary = `clax-${base64url(crypto.getRandomValues(new Uint8Array(18)))}`;
    const { body, contentType } = multipart(parts, boundary);
    const { report } = await readJson<{ report: Report }>(await this.send("POST", queuePath(this.opts.queue, "reporter", "reports"), body, contentType));
    return report;
  }

  /** The reports this reporter may see (its own, or all under `reporters_see: "all"`), a page at a time. */
  async myReports(cursor?: string): Promise<ReportPage> {
    const q = cursor === undefined ? "" : `?cursor=${encodeURIComponent(cursor)}`;
    return readJson<ReportPage>(await this.send("GET", queuePath(this.opts.queue, "reporter", "reports") + q));
  }

  /** Comments `body` on report `reportId` as this reporter. */
  async comment(reportId: string, body: string): Promise<Report> {
    const bytes = new TextEncoder().encode(JSON.stringify({ body }));
    const { report } = await readJson<{ report: Report }>(
      await this.send("POST", queuePath(this.opts.queue, "reporter", "reports", reportId, "comments"), bytes, "application/json"));
    return report;
  }

  /** Report `reportId`'s clip. */
  async clip(reportId: string): Promise<Blob> {
    const res = await this.send("GET", queuePath(this.opts.queue, "reporter", "reports", reportId, "clip"));
    if (!res.ok) throw await failure(res);
    return res.blob();
  }

  /** An object URL of report `reportId`'s clip, for an `<img>`; the caller
   * revokes it (`URL.revokeObjectURL`). The clip route is signed, so a
   * plain URL to it does not load. */
  async clipUrl(reportId: string): Promise<string> {
    return URL.createObjectURL(await this.clip(reportId));
  }

  /** The reporter's public key in unpadded base64url. */
  async publicKey(): Promise<string> {
    return (await this.key()).publicKey;
  }

  private key(): Promise<KeyedSigner> {
    this.signer ??= generateSigner().catch((e: unknown) => {
      if ((e as { name?: unknown } | null)?.name === "NotSupportedError") {
        throw new ClaxError("unsupported_browser", "this browser lacks WebCrypto Ed25519 signing", 0);
      }
      throw e;
    });
    return this.signer;
  }

  private async send(method: string, path: string, body?: Uint8Array, contentType?: string): Promise<Response> {
    const once = async (token: string) => signedFetch({
      method, instance: this.opts.instance, path, body, contentType, now: this.now, fetch: this.fetch,
      credential: { scheme: "Clax-Reporter", token, signer: await this.key() },
    });
    const first = await once((await this.current()).token);
    if (first.status !== 401) return first;
    this.token = undefined;
    return once((await this.current()).token);
  }

  /** A token valid for at least [`RENEW_BEFORE_S`] more seconds. */
  private current(): Promise<{ token: string; expiresAt: number }> {
    const t = this.token;
    if (t && this.now() < t.expiresAt - RENEW_BEFORE_S * 1000) return Promise.resolve(t);
    this.pending ??= (async () => {
      try {
        const got = await this.opts.auth((await this.key()).publicKey);
        const expiresAt = Date.parse(got.expires_at);
        if (!got.token || Number.isNaN(expiresAt)) throw new Error("auth gave no reporter token");
        this.token = { token: got.token, expiresAt };
        return this.token;
      } finally {
        this.pending = undefined;
      }
    })();
    return this.pending;
  }
}
