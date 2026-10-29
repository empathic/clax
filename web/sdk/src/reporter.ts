// A reporter's client (spec 2026-10-27 §5.3): a deployer's page files
// reports into a queue and reads the replies, with a reporter token its own
// backend mints for a key that never leaves the page.
import type { Badge, DiagLine, Report, ReportContext, ReportPage, ReporterToken } from "./types";
import { ClaxError, failure, multipart, queuePath, readJson, signedFetch, type Fetch, type Part } from "./client";
import { encodeDiagnostics } from "./diagnostics";
import { wellFormed } from "./scrub";
import { indexedDbStore, type Kept, type ReporterStore } from "./keystore";
import { base64url, generateKeyPair, keyedSigner, type KeyedSigner } from "./sign";

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
  /** The reporter's key. Default: the one kept for `user` (below), else a
   * new non-extractable one, made on first use; a browser without WebCrypto
   * Ed25519 fails every call with `ClaxError` code `unsupported_browser`. */
  signer?: KeyedSigner;
  /** A stable ID of the signed-in user, in the host's own terms (it is
   * hashed before it is stored). With it, the reporter's key (a
   * non-extractable CryptoKey) and its token are kept in IndexedDB for
   * this instance, queue and user, and reused across page loads until
   * shortly before the token expires, so `auth` is called once per token
   * lifetime rather than on every page load. Without it, or where
   * IndexedDB fails, they last as long as the page. Another user on the
   * same browser gets a key and token of their own. The key cannot be
   * exported, but any script of the page's origin can sign with it: the
   * host's origin is the trust boundary, and the host forgets the key on
   * sign-out (`forget`, `forgetReporter`). */
  user?: string;
  /** Where the key and token are kept. Default: IndexedDB when `user` is
   * given; `null` keeps them for the page alone. */
  store?: ReporterStore | null;
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
  /** What the reporter's browser says at capture (spec
   * 2026-10-29-feedback-loop-design §5.1). */
  context?: ReportContext;
  /** The page's recent diagnostics (`installDiagnostics`); the newest that
   * fit in 200 lines and 256 KiB are sent, none when empty. */
  diagnostics?: DiagLine[];
}

/** A verdict that a fix works. */
export interface Verification {
  /** The reporter's words, posted with the verdict. */
  note?: string;
  /** The reporter's browser context at verification. */
  context?: ReportContext;
}

/** A verdict that a report is still broken: a new capture of it. */
export interface Reopening {
  /** The reporter's comment, required. */
  body: string;
  /** A PNG of what is still broken. */
  clip?: Blob | Uint8Array;
  /** The page's recent diagnostics, sent as `NewReport.diagnostics` is. */
  diagnostics?: DiagLine[];
  context?: ReportContext;
}

/** `v` with every string and object key well-formed (`wellFormed`). */
function wellFormedDeep(v: unknown): unknown {
  if (typeof v === "string") return wellFormed(v);
  if (Array.isArray(v)) return v.map(wellFormedDeep);
  if (v !== null && typeof v === "object") {
    const toJSON = (v as { toJSON?: unknown }).toJSON;
    if (typeof toJSON === "function") return wellFormedDeep(toJSON.call(v));
    return Object.fromEntries(Object.entries(v).map(([k, x]) => [wellFormed(k), wellFormedDeep(x)]));
  }
  return v;
}

/** `ctx` as the JSON a report sends: every string well-formed, and a flag
 * whose value is a number that is not finite left out (JSON has no such
 * number, and Clax refuses the `null` it would become). */
export function contextJson(ctx: ReportContext): string {
  const c = wellFormedDeep(ctx) as ReportContext;
  if (c.flags) {
    c.flags = Object.fromEntries(Object.entries(c.flags).filter(([, f]) => typeof f !== "number" || Number.isFinite(f)));
  }
  return JSON.stringify(c);
}

/** The multipart parts a capture's `clip`, `context` and `diagnostics` make. */
async function captureParts(c: { clip?: Blob | Uint8Array; context?: ReportContext; diagnostics?: DiagLine[] }): Promise<[string, Part][]> {
  const parts: [string, Part][] = [];
  if (c.clip !== undefined) {
    const bytes = c.clip instanceof Uint8Array ? c.clip : new Uint8Array(await c.clip.arrayBuffer());
    parts.push(["clip", { bytes, filename: "clip.png", type: "image/png" }]);
  }
  if (c.context !== undefined) parts.push(["context", contextJson(c.context)]);
  if (c.diagnostics !== undefined && c.diagnostics.length > 0) {
    parts.push(["diagnostics", { bytes: encodeDiagnostics(c.diagnostics), filename: "diagnostics.jsonl", type: "application/jsonl" }]);
  }
  return parts;
}

/** Files and reads reports as one reporter.
 *
 * Each call carries a reporter token: the client asks `auth` for one on
 * first use and again from [`RENEW_BEFORE_S`] seconds before it expires,
 * and on a 401 asks once more and sends the call again once. Calls made
 * while a token is being fetched share that one `auth` call. With a `user`,
 * the key and the token are kept across page loads (`ReporterOptions.user`). */
export class ReporterClient {
  private readonly fetch: Fetch;
  private readonly now: () => number;
  private signer: Promise<KeyedSigner> | undefined;
  private token: { token: string; expiresAt: number } | undefined;
  private pending: Promise<{ token: string; expiresAt: number }> | undefined;
  /** Where the key and token are kept, until it first fails. */
  private store: ReporterStore | null;
  /** The key as kept, for saving it again with a new token. */
  private kept: Kept | undefined;
  /** A refused key was replaced since the last successful mint. */
  private rekeyed = false;

  constructor(private readonly opts: ReporterOptions) {
    this.fetch = opts.fetch ?? ((input, init) => globalThis.fetch(input, init));
    this.now = opts.now ?? Date.now;
    if (opts.signer) this.signer = Promise.resolve(opts.signer);
    this.store = opts.signer ? null
      : opts.store !== undefined ? opts.store
      : opts.user !== undefined ? indexedDbStore(opts.instance, opts.queue, opts.user)
      : null;
  }

  /** Files `r` into the queue: `POST …/reporter/reports`, with `context`
   * as a JSON part and `diagnostics` as `diagnostics.jsonl`. */
  async fileReport(r: NewReport): Promise<Report> {
    const parts: [string, Part][] = [];
    if (r.externalId !== undefined) parts.push(["external_id", wellFormed(r.externalId)]);
    parts.push(["url", wellFormed(r.url)], ["title", wellFormed(r.title)], ["anchor", JSON.stringify(wellFormedDeep(r.anchor ?? null))], ["body", wellFormed(r.body)]);
    if (r.metadata !== undefined) parts.push(["metadata", JSON.stringify(wellFormedDeep(r.metadata))]);
    if (r.revision !== undefined) parts.push(["revision", wellFormed(r.revision)]);
    parts.push(...await captureParts(r));
    return this.sendMultipart(queuePath(this.opts.queue, "reporter", "reports"), parts);
  }

  /** Says the fix waiting on report `reportId`, which this reporter filed,
   * works: `POST …/verify`. The report is `verified`; 409
   * `not_fix_proposed` when no fix waits. */
  async verify(reportId: string, v: Verification = {}): Promise<Report> {
    const fields: string[] = [];
    if (v.note !== undefined) fields.push(`"note":${JSON.stringify(wellFormed(v.note))}`);
    if (v.context !== undefined) fields.push(`"context":${contextJson(v.context)}`);
    const bytes = new TextEncoder().encode(`{${fields.join(",")}}`);
    const { report } = await readJson<{ report: Report }>(
      await this.send("POST", queuePath(this.opts.queue, "reporter", "reports", reportId, "verify"), bytes, "application/json"));
    return report;
  }

  /** Says report `reportId`, which this reporter filed, is still broken:
   * `POST …/reopen`, a new capture whose comment is `r.body`. A fix
   * waiting is `rejected` and the report is `open` again; 409
   * `not_fix_proposed` for an `open` or `claimed` report. */
  async reopen(reportId: string, r: Reopening): Promise<Report> {
    const parts: [string, Part][] = [["body", wellFormed(r.body)], ...await captureParts(r)];
    return this.sendMultipart(queuePath(this.opts.queue, "reporter", "reports", reportId, "reopen"), parts);
  }

  /** The diagnostics of report `reportId`, which this reporter filed:
   * capture `capture`'s (0 the filing's, `n` the `n`th reopen's), or
   * without it the latest capture's that has any; `[]` for none. */
  async diagnostics(reportId: string, capture?: number): Promise<DiagLine[]> {
    const q = capture === undefined ? "" : `?capture=${encodeURIComponent(String(capture))}`;
    const { lines } = await readJson<{ lines: DiagLine[] }>(
      await this.send("GET", queuePath(this.opts.queue, "reporter", "reports", reportId, "diagnostics") + q));
    return lines;
  }

  /** What waits for this reporter: its reports with a fix waiting for its
   * verdict, or a comment not its own since it last read them (`seen`). */
  async badge(): Promise<Badge> {
    return readJson<Badge>(await this.send("GET", queuePath(this.opts.queue, "reporter", "badge")));
  }

  /** Marks report `reportId` read: its comments until now leave the badge. */
  async seen(reportId: string): Promise<void> {
    const res = await this.send("POST", queuePath(this.opts.queue, "reporter", "reports", reportId, "seen"));
    if (!res.ok) throw await failure(res);
  }

  /** The reports this reporter may see (its own, or all under `reporters_see: "all"`), a page at a time. */
  async myReports(cursor?: string): Promise<ReportPage> {
    const q = cursor === undefined ? "" : `?cursor=${encodeURIComponent(cursor)}`;
    return readJson<ReportPage>(await this.send("GET", queuePath(this.opts.queue, "reporter", "reports") + q));
  }

  /** Comments `body` (made well-formed, `wellFormed`) on report `reportId` as this reporter. */
  async comment(reportId: string, body: string): Promise<Report> {
    const bytes = new TextEncoder().encode(JSON.stringify({ body: wellFormed(body) }));
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

  /** The clip of comment `commentId` (a reopen's) on report `reportId`. */
  async commentClip(reportId: string, commentId: string): Promise<Blob> {
    const res = await this.send("GET", queuePath(this.opts.queue, "reporter", "reports", reportId, "comments", commentId, "clip"));
    if (!res.ok) throw await failure(res);
    return res.blob();
  }

  /** An object URL of `commentClip`, which the caller revokes. */
  async commentClipUrl(reportId: string, commentId: string): Promise<string> {
    return URL.createObjectURL(await this.commentClip(reportId, commentId));
  }

  /** Posts `parts` as signed multipart to `path`; the answer's report. */
  private async sendMultipart(path: string, parts: [string, Part][]): Promise<Report> {
    const boundary = `clax-${base64url(crypto.getRandomValues(new Uint8Array(18)))}`;
    const { body, contentType } = multipart(parts, boundary);
    const { report } = await readJson<{ report: Report }>(await this.send("POST", path, body, contentType));
    return report;
  }

  /** The reporter's public key in unpadded base64url. */
  async publicKey(): Promise<string> {
    return (await this.key()).publicKey;
  }

  /** Forgets this reporter, as a host does on sign-out: deletes the key
   * and token kept for its user and drops them here, and keeps nothing
   * from now on. A call after it makes a key for the page alone. */
  async forget(): Promise<void> {
    const store = this.store;
    this.store = null;
    this.signer = undefined;
    this.kept = undefined;
    this.token = undefined;
    if (store) await store.forget().catch(() => undefined);
  }

  private key(): Promise<KeyedSigner> {
    this.signer ??= (async () => {
      const kept = await this.loadKept();
      if (kept) {
        this.kept = kept;
        if (kept.token) this.token ??= kept.token;
        return keyedSigner(kept.privateKey, kept.publicKey);
      }
      const made = await generateKeyPair().catch((e: unknown) => {
        if ((e as { name?: unknown } | null)?.name === "NotSupportedError") {
          throw new ClaxError("unsupported_browser", "this browser lacks WebCrypto Ed25519 signing", 0);
        }
        throw e;
      });
      this.kept = made;
      await this.keep(made);
      return keyedSigner(made.privateKey, made.publicKey);
    })();
    return this.signer;
  }

  /** The key and token the store keeps, or none (the store failing is
   * given up on). */
  private async loadKept(): Promise<Kept | null> {
    if (!this.store) return null;
    try {
      const k = await this.store.load();
      return k && k.privateKey && typeof k.publicKey === "string" ? k : null;
    } catch {
      this.store = null;
      return null;
    }
  }

  /** Forgets the kept key and token, here and in the store, so the next
   * `key()` makes a new key (and keeps it). */
  private async dropKey(): Promise<void> {
    this.signer = undefined;
    this.kept = undefined;
    this.token = undefined;
    if (this.store) await this.store.forget().catch(() => { this.store = null; });
  }

  /** Keeps `k` in the store, if it still works. */
  private async keep(k: Kept): Promise<void> {
    if (!this.store) return;
    try {
      await this.store.save(k);
    } catch {
      this.store = null;
    }
  }

  private async send(method: string, path: string, body?: Uint8Array, contentType?: string): Promise<Response> {
    const once = async (token: string) => signedFetch({
      method, instance: this.opts.instance, path, body, contentType, now: this.now, fetch: this.fetch,
      credential: { scheme: "Clax-Reporter", token, signer: await this.key() },
    });
    const first = await once((await this.current()).token);
    if (first.status !== 401) return first;
    this.token = undefined;
    if (this.store) await this.store.forgetToken().catch(() => { this.store = null; });
    return once((await this.current()).token);
  }

  /** A token valid for at least [`RENEW_BEFORE_S`] more seconds. */
  private async current(): Promise<{ token: string; expiresAt: number }> {
    const key = await this.key();
    const t = this.token;
    if (t && this.now() < t.expiresAt - RENEW_BEFORE_S * 1000) return t;
    this.pending ??= (async () => {
      try {
        let got: ReporterToken;
        try {
          got = await this.opts.auth(key.publicKey);
        } catch (e) {
          // The backend refused this key for good: forget it, and ask
          // once with a fresh one.
          if (this.rekeyed || (e as { code?: unknown } | null)?.code !== "invalid_key") throw e;
          this.rekeyed = true;
          await this.dropKey();
          got = await this.opts.auth((await this.key()).publicKey);
        }
        const expiresAt = Date.parse(got.expires_at);
        if (!got.token || Number.isNaN(expiresAt)) throw new Error("auth gave no reporter token");
        this.token = { token: got.token, expiresAt };
        // A key that worked: a later refusal of it may be replaced again.
        this.rekeyed = false;
        if (this.kept) await this.keep({ privateKey: this.kept.privateKey, publicKey: this.kept.publicKey, token: this.token });
        return this.token;
      } finally {
        this.pending = undefined;
      }
    })();
    return this.pending;
  }
}
