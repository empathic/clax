// Pairing with the daemon through the native host (spec 2026-10-05 §9.1).
// The pairing (the daemon's URL and the credential) lives only in
// chrome.storage.session, in memory and out of content scripts' reach. A
// daemon of another Clax version reloads the extension once for that
// version: `clax extension install` has rewritten the unpacked files.
export const HOST = "dev.empathic.clax";
/** The least time between two pairings a failure asked for. */
export const REPAIR_MS = 10_000;

export type Pairing = { daemon: string; credential: string; claxVersion: string };

export class PairError extends Error {
  constructor(readonly code: string, message: string) { super(message); }
}

type Area = { get(k: string): Promise<Record<string, unknown>>; set(v: Record<string, unknown>): Promise<void>; remove(k: string): Promise<void> };
export interface PairEnv {
  sendNative(host: string, msg: object): Promise<unknown>;
  session: Area;
  local: Area;
  manifestVersion: string;
  reload(): void;
  now(): number;
}

const numeric = (v: string) => /^\d+\.\d+\.\d+/.exec(v)?.[0] ?? v;
const CREDENTIAL = /^cxe_[A-Za-z0-9_-]{43}$/;

/** `http://localhost:<port>` or `http://127.0.0.1:<port>`, nothing else
 * (spec §10.6: the daemon's `/api` host rule admits only these). */
function daemonUrl(v: unknown): v is string {
  const m = typeof v === "string" ? /^http:\/\/(?:localhost|127\.0\.0\.1):([1-9]\d{0,4})$/.exec(v) : null;
  return !!m && Number(m[1]) <= 65_535;
}

function isPairing(v: unknown): v is Pairing {
  const p = v as Partial<Pairing> | null;
  return typeof p === "object" && p !== null && daemonUrl(p.daemon) && typeof p.credential === "string" && CREDENTIAL.test(p.credential) && typeof p.claxVersion === "string";
}

function parse(reply: unknown): Pairing {
  const r = (typeof reply === "object" && reply !== null ? reply : {}) as Record<string, unknown>;
  if (r.type === "paired" && r.v === 1 && daemonUrl(r.daemon) && typeof r.credential === "string" && CREDENTIAL.test(r.credential) && typeof r.clax_version === "string") {
    return { daemon: r.daemon, credential: r.credential, claxVersion: r.clax_version };
  }
  if (r.type === "error" && typeof r.code === "string") throw new PairError(r.code, typeof r.message === "string" ? r.message : r.code);
  throw new PairError("bad_reply", "the native host answered something other than a pairing");
}

export class Pairer {
  private inflight: Promise<Pairing> | null = null;
  private last = Number.NEGATIVE_INFINITY;
  constructor(private readonly env: PairEnv) {}

  /** The stored pairing, else a new one. */
  async current(): Promise<Pairing> {
    const stored = (await this.env.session.get("pairing")).pairing;
    return isPairing(stored) ? stored : this.pair();
  }

  /** A new pairing; concurrent callers share it, and another within
   * `REPAIR_MS` of the last is refused (`paired_recently`). */
  pair(): Promise<Pairing> {
    if (this.inflight) return this.inflight;
    if (this.env.now() - this.last < REPAIR_MS) return Promise.reject(new PairError("paired_recently", "Clax paired a moment ago; try again shortly"));
    this.last = this.env.now();
    this.inflight = (async () => {
      try {
        const p = parse(await this.env.sendNative(HOST, { type: "pair", v: 1, extension_version: this.env.manifestVersion }));
        await this.env.session.set({ pairing: p });
        await this.reloadFor(p.claxVersion);
        return p;
      } finally {
        this.inflight = null;
      }
    })();
    return this.inflight;
  }

  /** Drops the stored pairing; the next request pairs again. */
  async forget(): Promise<void> {
    await this.env.session.remove("pairing");
  }

  private async reloadFor(v: string): Promise<void> {
    if (numeric(v) === numeric(this.env.manifestVersion)) return;
    if ((await this.env.local.get("reloadedFor")).reloadedFor === numeric(v)) return;
    await this.env.local.set({ reloadedFor: numeric(v) });
    this.env.reload();
  }
}
