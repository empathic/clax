// Pairing with the daemon through the native host (spec 2026-10-05 §9.1).
// The pairing (the daemon's URL and the credential) lives only in
// chrome.storage.session, in memory and out of content scripts' reach. A
// daemon of another Clax version reloads the extension once for that
// version: `clax extension install` has rewritten the unpacked files.
export const HOST = "dev.empathic.clax";
/** The least time between two pairings a failure asked for. */
export const REPAIR_MS = 10_000;
/** How long a pairing may take before the person is told Clax is slow to
 * start. Chrome starts the native host as a new process each time, which
 * normally answers in a second or two; on a busy machine starting it was
 * seen to stall for tens of seconds. The pairing keeps waiting: a late
 * answer still pairs. */
export const PAIR_SLOW_MS = 15_000;

export type Pairing = { daemon: string; credential: string; claxVersion: string };

export class PairError extends Error {
  /** `confirmation` names the browser confirmation a held pairing waits
   * on (code `confirmation_pending`). */
  constructor(readonly code: string, message: string, readonly confirmation: string | null = null) { super(message); }
}


type Area = { get(k: string): Promise<Record<string, unknown>>; set(v: Record<string, unknown>): Promise<void>; remove(k: string): Promise<void> };
export interface PairEnv {
  sendNative(host: string, msg: object): Promise<unknown>;
  session: Area;
  local: Area;
  manifestVersion: string;
  reload(): void;
  now(): number;
  /** Runs `fn` after `ms`; returns what cancels it. */
  after?(ms: number, fn: () => void): () => void;
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
  if (r.type === "error" && typeof r.code === "string") {
    // The host names a held pairing's confirmation, and checks the one sent back.
    const held = r.code === "confirmation_pending" && typeof r.confirmation_id === "string" ? r.confirmation_id : null;
    throw new PairError(r.code, typeof r.message === "string" ? r.message : r.code, held);
  }
  throw new PairError("bad_reply", "the native host answered something other than a pairing");
}

export class Pairer {
  private inflight: Promise<Pairing> | null = null;
  private last = Number.NEGATIVE_INFINITY;
  /** Told `true` when the pairing in flight has taken PAIR_SLOW_MS, and
   * `false` once that pairing ends, however it ends. */
  onSlow: ((slow: boolean) => void) | null = null;
  /** Called after each pairing that succeeds and is stored (the first one too). */
  onPaired: ((p: Pairing) => void) | null = null;
  /** Told `true` when the pairing in flight waits for the person to
   * confirm it in their Clax browser tab (the daemon holds an extension's
   * pairing for that while an agent session is live), and `false` once
   * that pairing ends, however it ends. */
  onConfirm: ((waiting: boolean) => void) | null = null;
  constructor(private readonly env: PairEnv) {}

  /** The stored pairing, else a new one. */
  async current(): Promise<Pairing> {
    const stored = (await this.env.session.get("pairing")).pairing;
    return isPairing(stored) ? stored : this.pair();
  }

  /** The stored pairing, else the pairing under way; null when there is
   * neither (no native host is started for it). */
  async joined(): Promise<Pairing | null> {
    const stored = (await this.env.session.get("pairing")).pairing;
    return isPairing(stored) ? stored : this.inflight;
  }

  /** A new pairing; concurrent callers share it, a Retry included (never
   * two native hosts at once), and another within `REPAIR_MS` of the last
   * is refused (`paired_recently`) unless `retry` (the person asked to try
   * again). */
  pair(retry = false): Promise<Pairing> {
    if (this.inflight) return this.inflight;
    if (!retry && this.env.now() - this.last < REPAIR_MS) return Promise.reject(new PairError("paired_recently", "Clax paired a moment ago; try again shortly"));
    this.last = this.env.now();
    let slow = false;
    const after = this.env.after ?? ((ms, fn) => { const h = setTimeout(fn, ms); return () => clearTimeout(h); });
    const cancel = after(PAIR_SLOW_MS, () => { slow = true; this.onSlow?.(true); });
    let confirming = false;
    this.inflight = (async () => {
      try {
        const msg = { type: "pair", v: 1, extension_version: this.env.manifestVersion };
        let p: Pairing;
        try {
          p = parse(await this.env.sendNative(HOST, msg));
        } catch (e) {
          // Held for the person's confirmation: say so, and ask again with
          // it; the host waits for their answer, then pairs.
          if (!(e instanceof PairError) || !e.confirmation) throw e;
          confirming = true;
          this.onConfirm?.(true);
          p = parse(await this.env.sendNative(HOST, { ...msg, confirmation: e.confirmation }));
        }
        await this.env.session.set({ pairing: p });
        this.onPaired?.(p);
        await this.reloadFor(p.claxVersion);
        return p;
      } finally {
        this.inflight = null;
        cancel();
        if (slow) this.onSlow?.(false);
        if (confirming) this.onConfirm?.(false);
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
