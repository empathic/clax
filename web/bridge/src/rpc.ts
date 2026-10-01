// Requests from the page's capability namespaces to the shell, and the shell's
// answers and pushes (see protocol.ts). Nothing is sent before the shell's
// welcome: until then requests queue, so they go only to the shell's origin.
import type { BridgeToShell, ShellToBridge } from "./protocol";

/** How long `use()` waits for a shell that never answers before resolving null. */
export const USE_TIMEOUT_MS = 10_000;

/** A capability call's rejection: `code` is the contract's error code; extra
 * fields the shell sent (`live`, `paths`, ...) are copied onto the error. */
export class CapabilityError extends Error {
  readonly code: string;
  constructor(code: string, message: string, extra: Record<string, unknown> = {}) {
    super(message);
    this.name = "CapabilityError";
    this.code = code;
    for (const [k, v] of Object.entries(extra)) if (k !== "name" && k !== "stack") (this as Record<string, unknown>)[k] = v;
  }
}

/** Whether `e` is a `CapabilityError` from any copy of this module: the eager
 * bridge (whose `Rpc` rejects calls with its own) and each lazy part are
 * separate builds, each with its own class. */
export function isCapabilityError(e: unknown): e is CapabilityError {
  return e instanceof Error && e.name === "CapabilityError" && typeof (e as { code?: unknown }).code === "string";
}

type Pending = { resolve(v: unknown): void; reject(e: unknown): void };

export class Rpc {
  private seq = 0;
  private open = false;
  private readonly queue: BridgeToShell[] = [];
  private readonly uses = new Map<string, (r: { config: unknown } | null) => void>();
  private readonly calls = new Map<string, Pending>();
  private readonly listeners = new Map<string, Set<(data: unknown) => void>>();

  constructor(private readonly post: (m: BridgeToShell) => void, private readonly timeoutMs = USE_TIMEOUT_MS) {}

  /** The shell answered: send what queued, in order, and everything after at once. */
  connect(): void {
    if (this.open) return;
    this.open = true;
    for (const m of this.queue.splice(0)) this.post(m);
  }

  private send(m: BridgeToShell): void {
    if (this.open) this.post(m);
    else this.queue.push(m);
  }

  private nextId(): string {
    return `c${++this.seq}`;
  }

  /** The declared config when the shell grants `name`, else null (also after
   * [`USE_TIMEOUT_MS`] without an answer). Never rejects. */
  use(name: string): Promise<{ config: unknown } | null> {
    const id = this.nextId();
    return new Promise(resolve => {
      const timer = setTimeout(() => { this.uses.delete(id); resolve(null); }, this.timeoutMs);
      this.uses.set(id, r => { clearTimeout(timer); resolve(r); });
      this.send({ type: "clax:use", id, name });
    });
  }

  /** Calls `ns.method(...args)` in the shell. Arguments that cannot be
   * structured-cloned reject `transform_error` without reaching the shell. */
  call(ns: string, method: string, args: unknown[]): Promise<unknown> {
    try {
      structuredClone(args);
    } catch (e) {
      return Promise.reject(new CapabilityError("transform_error", `the arguments of ${ns}.${method} cannot be sent: ${e instanceof Error ? e.message : String(e)}`));
    }
    const id = this.nextId();
    return new Promise((resolve, reject) => {
      this.calls.set(id, { resolve, reject });
      this.send({ type: "clax:call", id, ns, method, args });
    });
  }

  /** Listens for `clax:event` pushes of `ns`/`topic`; returns the unsubscriber. */
  on(ns: string, topic: string, fn: (data: unknown) => void): () => void {
    const key = `${ns}\u0000${topic}`;
    let set = this.listeners.get(key);
    if (!set) this.listeners.set(key, (set = new Set()));
    set.add(fn);
    return () => { set!.delete(fn); };
  }

  /** Takes a shell message (already checked for window, origin, and type). */
  accept(m: ShellToBridge): void {
    switch (m.type) {
      case "clax:use-result": {
        const done = this.uses.get(m.id);
        if (!done) return;
        this.uses.delete(m.id);
        done(m.granted ? { config: m.config } : null);
        return;
      }
      case "clax:call-result": {
        const p = this.calls.get(m.id);
        if (!p) return;
        this.calls.delete(m.id);
        if (m.ok) p.resolve(m.value);
        else {
          const { code, message, ...extra } = m.error;
          p.reject(new CapabilityError(String(code), String(message), extra));
        }
        return;
      }
      case "clax:event":
        for (const fn of [...(this.listeners.get(`${m.ns}\u0000${m.topic}`) ?? [])]) {
          try { fn(m.data); } catch (e) { reportError(e); }
        }
        return;
      default:
        return;
    }
  }
}
