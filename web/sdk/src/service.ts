// A deployer's backend (spec 2026-10-27 §5.3): it holds a queue service
// grant in its secrets and mints reporter tokens for its own users. Runs
// in Node 20 or later, a Worker or any runtime with WebCrypto Ed25519; it
// is never shipped to a page, which would hand out the grant.
import type { ReporterProfile, ReporterToken, ReporterTokenRequest } from "./types";
import { queuePath, readJson, signedFetch, type Fetch } from "./client";
import { fromBase64url, importPkcs8Signer, type Signer } from "./sign";

export interface ServiceOptions {
  /** The instance's base URL. */
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
}

/** Calls a queue's service routes with its service grant. */
export class ServiceClient {
  private signer: Promise<Signer> | undefined;

  constructor(private readonly opts: ServiceOptions) {}

  /** Mints a reporter token for `r.reporter_id`, bound to the key
   * `r.public_key` and the page origin `r.origin` (one of the queue's
   * origins): `POST /api/queues/<queue>/reporter-tokens`. `profile` (else
   * `r.profile`) is who this backend says the reporter is: each report
   * filed under the token keeps it, shown to triage members and never to
   * reporters; 400 `invalid_profile` for a field over 256 bytes or an
   * `avatar_url` that is not http(s). */
  async mintReporterToken(r: ReporterTokenRequest, profile: ReporterProfile | undefined = r.profile): Promise<ReporterToken> {
    const req: ReporterTokenRequest = { reporter_id: r.reporter_id, origin: r.origin, public_key: r.public_key };
    if (profile !== undefined) req.profile = profile;
    const body = new TextEncoder().encode(JSON.stringify(req));
    this.signer ??= importPkcs8Signer(fromBase64url(this.opts.privateKey));
    const res = await signedFetch({
      method: "POST",
      instance: this.opts.instance,
      path: queuePath(this.opts.queue, "reporter-tokens"),
      body,
      contentType: "application/json",
      credential: { scheme: "Clax-Queue", token: this.opts.token, signer: await this.signer },
      now: this.opts.now ?? Date.now,
      fetch: this.opts.fetch ?? ((input, init) => globalThis.fetch(input, init)),
    });
    return readJson<ReporterToken>(res);
  }
}
