// The shell end of the capability protocol: answers a frame's `artifax:use`
// from the declaration and the view (availability.ts), runs `artifax:call`s
// through one handler per capability, and relays SSE events to handlers that
// follow the stream. Every call is answered, with a value or `{code, message}`.
import type { BridgeToShell, ShellToBridge } from "../../../bridge/src/protocol";
import type { ArtifactEvent } from "../events";
import { type Declared, declaredConfig, isAvailable } from "./availability";
import { CapError } from "./errors";
import { Grants, type Prompt, type PromptAnswer, grantsKey } from "./grants";
import { REGISTRY } from "./registry";

export type ViewerInfo = { publicId: string; name: string | null };

/** What handlers know about the view. `token` is non-null only in the owner shell. */
export interface CapEnv {
  aid: string;
  /** The version the frame shows. */
  version: number;
  /** The shell is pinned to `/a/<aid>/v/<n>`. */
  pinned: boolean;
  token: string | null;
  viewer(): Promise<ViewerInfo>;
  declared: Declared;
  prompt(p: Prompt): Promise<PromptAnswer>;
  post(m: ShellToBridge): void;
  /** Loads the latest version in the shell. */
  reload(): void;
}

export interface Handler {
  call(method: string, args: unknown[]): Promise<unknown>;
  onEvent?(e: ArtifactEvent): void;
  /** The frame loaded a new document: drop per-document state. */
  reset?(): void;
}

export type HandlerFactory = (env: CapEnv, grants: Grants) => Handler;

function localStore(): Storage | null {
  try {
    return localStorage;
  } catch {
    return null;
  }
}

export class CapabilityHost {
  private readonly handlers = new Map<string, Handler>();
  private readonly ready: Promise<{ env: CapEnv; grants: Grants }>;

  constructor(env: Promise<CapEnv>, private readonly factories: Record<string, HandlerFactory> = REGISTRY, storage: Storage | null = localStore()) {
    // Without a viewer (its lookup failed) grants are kept for this page load
    // only, so every request is still answered.
    this.ready = env.then(async e => {
      const viewer = await e.viewer().then(v => v.publicId, () => null);
      const grants = new Grants(grantsKey(e.aid, viewer ?? ""), viewer === null ? null : storage, e.declared, e.token !== null, e.prompt);
      return { env: e, grants };
    });
  }

  async handle(m: BridgeToShell): Promise<void> {
    if (m.type !== "artifax:use" && m.type !== "artifax:call") return;
    const { env, grants } = await this.ready;
    const owner = env.token !== null;
    if (m.type === "artifax:use") {
      const granted = typeof m.name === "string" && isAvailable(m.name, env.declared, owner);
      env.post({ type: "artifax:use-result", id: m.id, granted, config: granted ? declaredConfig(m.name, env.declared) : null });
      return;
    }
    try {
      if (!isAvailable(m.ns, env.declared, owner)) throw new CapError("not_granted", `${m.ns} is not available to this view`);
      const value = await this.handler(m.ns, env, grants).call(m.method, Array.isArray(m.args) ? m.args : []);
      env.post({ type: "artifax:call-result", id: m.id, ok: true, value });
    } catch (e) {
      const error = e instanceof CapError
        ? { ...e.extra, code: e.code, message: e.message }
        : { code: "upstream_error", message: e instanceof Error ? e.message : String(e) };
      env.post({ type: "artifax:call-result", id: m.id, ok: false, error });
    }
  }

  private handler(ns: string, env: CapEnv, grants: Grants): Handler {
    let h = this.handlers.get(ns);
    if (!h) {
      const make = this.factories[ns];
      if (!make) throw new CapError("capability_removed", `${ns} is not part of this runtime`);
      h = make(env, grants);
      this.handlers.set(ns, h);
    }
    return h;
  }

  onEvent(e: ArtifactEvent): void {
    for (const h of this.handlers.values()) h.onEvent?.(e);
  }

  reset(): void {
    for (const h of this.handlers.values()) h.reset?.();
  }
}
