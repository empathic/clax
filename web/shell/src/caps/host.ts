// The shell end of the capability protocol: answers a frame's `artifax:use`
// from the declaration and the view (availability.ts), runs `artifax:call`s
// through one handler per capability, and relays SSE events to handlers that
// follow the stream. Every call is answered, with a value or `{code, message}`.
import type { Anchor, Box, BridgeToShell, ShellToBridge } from "../../../bridge/src/protocol";
import type { FileMeta } from "../api";
import type { ArtifactEvent } from "../events";
import type { Thread } from "../threads";
import { type Declared, declaredConfig, isAvailable } from "./availability";
import { CapError } from "./errors";
import { Grants, type Prompt, type PromptAnswer, grantsKey } from "./grants";
import { REGISTRY } from "./registry";

/** The shell's comment UI as the `comments` capability drives it. */
export interface CommentsUi {
  /** Opens the composer for a pick; false when a composer holds typed text,
   * unless `opts.area` (a page's drawn area), which moves that composer to
   * the new anchor with its text. `label` is the page's words for the spot,
   * shown in the composer only. */
  openComposer(d: { anchor: Anchor; version: number; clip: Blob | null; clipError?: string; label?: string; capturing?: boolean; clipToken?: string }, opts?: { area?: boolean }): boolean;
  /** The clip for the composer opened with `clipToken` (dropped when that
   * composer is gone or was opened again for another pick). */
  attachClip?(clipToken: string, clip: Blob | null, clipError?: string): void;
  upsert(t: Thread): void;
  /** Drops a thread the page deleted. */
  remove(threadId: string): void;
  /** Custom anchoring turned on or off: pins then come from `place` only. */
  setCustom(live: boolean): void;
  /** Pin positions from the page, by thread ID, in frame viewport pixels. */
  place(rects: Record<string, Box>): void;
  select(threadId: string): void;
  /** Leaves comment mode unless a composer holds typed text. */
  exitMode(): void;
  /** `busy`: a post or a send to the agent is in flight. */
  state(): { mode: boolean; composing: boolean; threads: Thread[]; selected: string | null; busy?: boolean };
  /** The viewer's click over an open composer or thread card: closes an empty
   * composer or the card (a composer holding typed text stays); true when
   * something closed. */
  dismiss?(): boolean;
  /** Starts comment mode (the page's own compose did). */
  enterMode?(): void;
}

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
  /** How many of this view's own `artifact.publish` calls are in flight (a
   * publish that ends in a reload keeps its count), so the SSE `version` event
   * one causes does not reload the view before the page hears the result.
   * `settled` runs when the count drops back to 0 without a reload. */
  ownPublish?: { active: number; settled?(): void };
  /** The files of the shown version, when known. */
  files?: Record<string, FileMeta>;
  /** The published file of the page in the frame, from its latest matching
   * hello (null while the frame shows no greeted page); `index.html` when absent. */
  page?(): string | null;
  /** The shell's comment UI, when this view has one. */
  comments?: CommentsUi;
}

export interface Handler {
  call(method: string, args: unknown[]): Promise<unknown>;
  onEvent?(e: ArtifactEvent): void;
  /** The frame loaded a new document: drop per-document state. */
  reset?(): void;
  /** The host is gone: drop all state and never post, fetch, or schedule again. */
  dispose?(): void;
  /** Comment mode, the composer, the selection, the threads, or the page changed. */
  uiChanged?(): void;
  /** Lets a custom-anchors page bring thread `threadId` into view; false when it anchors nothing. */
  reveal?(threadId: string): boolean;
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
  private dead = false;

  constructor(env: Promise<CapEnv>, private readonly factories: Record<string, HandlerFactory> = REGISTRY, storage: Storage | null = localStore()) {
    // Without a viewer (its lookup failed) grants are kept for this page load
    // only, so every request is still answered.
    this.ready = env.then(async given => {
      // After dispose nothing reaches the frame, whatever resolves late.
      const e: CapEnv = { ...given, post: m => { if (!this.dead) given.post(m); } };
      const viewer = await e.viewer().then(v => v.publicId, () => null);
      const grants = new Grants(grantsKey(e.aid, viewer ?? ""), viewer === null ? null : storage, e.declared, e.token !== null, e.prompt);
      return { env: e, grants };
    });
  }

  async handle(m: BridgeToShell): Promise<void> {
    if (this.dead || (m.type !== "artifax:use" && m.type !== "artifax:call")) return;
    const { env, grants } = await this.ready;
    if (this.dead) return;
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
    if (this.dead) return;
    for (const h of this.handlers.values()) h.onEvent?.(e);
  }

  reset(): void {
    for (const h of this.handlers.values()) h.reset?.();
  }

  /** Comment mode, the composer, the selection, the threads, or the page changed. */
  uiChanged(): void {
    if (this.dead) return;
    for (const h of this.handlers.values()) h.uiChanged?.();
  }

  /** Lets a custom-anchors page bring thread `threadId` into view; false when none is registered. */
  reveal(threadId: string): boolean {
    if (this.dead) return false;
    for (const h of this.handlers.values()) if (h.reveal?.(threadId)) return true;
    return false;
  }

  /** Ends this host (its artifact, version, or view was replaced, or the
   * shell unmounted): every handler is disposed (or reset), and nothing it
   * later answers, pushes, or fetches reaches any frame. */
  dispose(): void {
    if (this.dead) return;
    this.dead = true;
    for (const h of this.handlers.values()) {
      if (h.dispose) h.dispose();
      else h.reset?.();
    }
    this.handlers.clear();
  }
}
