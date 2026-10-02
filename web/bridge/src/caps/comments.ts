// The `comments` namespace (comments.d.ts). The page opens the shell's
// composer on its own elements, writes threads as the viewer (full form,
// after the viewer's consent in the shell), and may take over anchoring for
// content the shell cannot anchor to (customAnchors). The shell renders every
// thread; the page never lists them. Every anchor built here names this
// page's published file. `canSendToClaude`, `resolve`, and `delete` are plain
// shell calls; the shell checks every argument again.
import { buildElementAnchor, buildRangeAnchor, cssPath } from "../anchor";
import type { Local } from "../capabilities";
import type { CommentsContext } from "../comments-context";
import type { ClipPart } from "../parts/types";
import type { Anchor } from "../protocol";
import { CapabilityError, type Rpc } from "../rpc";
import { nameProblem, textProblem } from "../text-rule";

export { MAX_TEXT_BYTES, textProblem } from "../text-rule";

/** What the eager bridge hands the page-side members: its shared state, and
 * the clip part, loaded on first use. */
export type CapsEnv = { ctx: CommentsContext; clip: () => Promise<ClipPart> };

/** Longest `label` or `detail`, in UTF-16 code units (comments.d.ts). */
export const MAX_LABEL = 1024;
const invalid = (message: string) => new CapabilityError("invalid", message);

type DocPoint = { x: number; y: number };
const isPoint = (p: unknown): p is DocPoint =>
  !!p && typeof p === "object" && Number.isFinite((p as DocPoint).x) && Number.isFinite((p as DocPoint).y);
const uncommentable = (n: Node | null) => {
  const el = n instanceof Element ? n : n?.parentElement ?? null;
  return !!el?.closest("[data-uncommentable]");
};
const center = (el: Element): DocPoint => {
  const r = el.getBoundingClientRect();
  return { x: r.x + r.width / 2 + scrollX, y: r.y + r.height / 2 + scrollY };
};

/** A thread anchor for a `comments.Anchor` (`{path, x, y}`): the element's
 * full anchor when the path still finds it, else the path alone. */
function toAnchor(ctx: CommentsContext, a: { path: string }): Anchor {
  let el: Element | null = null;
  try { el = document.querySelector(a.path); } catch { el = null; }
  if (el) return buildElementAnchor(document, el, ctx.file);
  return { kind: "element", selector: a.path, quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: ctx.file };
}

export function commentsLocals(rpc: Pick<Rpc, "call" | "on">, config: unknown, env: CapsEnv): Local {
  const cfg = (config ?? {}) as { composer_only?: unknown; customAnchors?: unknown };
  const text = (t: unknown) => { const p = textProblem(t); if (p) throw invalid(p); return t as string; };

  const openComposer = async (target: unknown) => {
    const t = target as { element?: unknown; range?: unknown } | null;
    let anchor: Anchor;
    let of: Element | Range;
    if (t && t.element instanceof Element && t.element.isConnected) {
      if (uncommentable(t.element)) return { opened: false };
      anchor = buildElementAnchor(document, t.element, env.ctx.file);
      of = t.element;
    } else if (t && t.range instanceof Range && t.range.startContainer.isConnected && t.range.endContainer.isConnected) {
      if (uncommentable(t.range.commonAncestorContainer)) return { opened: false };
      anchor = buildRangeAnchor(document, t.range, env.ctx.file);
      of = t.range;
    } else {
      throw invalid("openComposer takes {element} or {range}, attached to the document");
    }
    let clipPng: ArrayBuffer | undefined;
    let clipError: string | undefined;
    try { clipPng = await (await env.clip()).renderTargetClip(of); } catch (e) { clipError = e instanceof Error ? e.message : String(e); }
    return rpc.call("comments", "openComposer", [{ anchor, version: env.ctx.version, clipPng, clipError }]);
  };

  const anchorFor = async (el: unknown) => {
    if (!(el instanceof Element) || !el.isConnected) throw invalid("anchorFor takes an element attached to the document");
    return { path: cssPath(el), ...center(el) };
  };

  const checkAnchor = (a: unknown): { path: string } => {
    if (!a || typeof (a as { path?: unknown }).path !== "string" || !(a as { path: string }).path || !isPoint(a)) throw invalid("anchor is what anchorFor returned");
    return a as unknown as { path: string };
  };

  let registered = false;
  const customAnchors = async (callbacks: unknown) => {
    if (cfg.customAnchors !== true) throw new CapabilityError("not_granted", "the declaration does not carry \"customAnchors\": true");
    const cb = callbacks as { mode?: unknown; threads?: unknown; reveal?: unknown; composing?: unknown } | null;
    if (!cb || typeof cb.mode !== "function" || typeof cb.threads !== "function" || typeof cb.reveal !== "function") throw invalid("customAnchors needs mode, threads, and reveal callbacks");
    if (registered) throw invalid("a registration is already live; release it first");
    registered = true;
    let released = false;
    // Whether an area compose could start, as the shell last reported it
    // with the mode (comment mode on, no post or send in flight); `areas`
    // and `compose`'s `area` follow it.
    let canArea = false;
    // Comment mode as last passed to the page's `mode` callback.
    let modeOn: boolean | null = null;
    let placedOnce = false;
    let lastPlaced: Record<string, DocPoint> = {};
    let frame = 0;
    const minted = new Set<string>();
    // Callbacks are cheap and infallible by contract; a throw is discarded.
    const safe = (f: () => void) => { try { f(); } catch { /* discarded */ } };
    const offs = [
      // `mode` fires only when comment mode starts or ends; a change of
      // `canArea` alone (a post or send starting or ending) is taken silently.
      rpc.on("comments", "mode", d => {
        const on = (d as { on?: unknown })?.on === true;
        canArea = on && (d as { canArea?: unknown })?.canArea !== false;
        if (on === modeOn) return;
        modeOn = on;
        safe(() => (cb.mode as (on: boolean) => void)(on));
      }),
      rpc.on("comments", "threads", d => safe(() => (cb.threads as (l: unknown) => void)((d as { list?: unknown })?.list ?? []))),
      rpc.on("comments", "reveal", d => { if (placedOnce) safe(() => (cb.reveal as (id: string) => void)(String((d as { id?: unknown })?.id))); }),
      rpc.on("comments", "composing", d => { if (typeof cb.composing === "function") safe(() => (cb.composing as (o: boolean) => void)((d as { open?: unknown })?.open === true)); }),
    ];
    // Positions go to the shell in frame viewport pixels, re-projected on
    // every scroll and resize (at most once per animation frame).
    const sendPlaced = () => {
      const viewport: Record<string, DocPoint> = {};
      for (const [id, p] of Object.entries(lastPlaced)) viewport[id] = { x: p.x - scrollX, y: p.y - scrollY };
      void rpc.call("comments", "placed", [viewport]).catch(() => {});
    };
    env.ctx.live = true;
    env.ctx.reflow = () => {
      if (!placedOnce || frame) return;
      frame = requestAnimationFrame(() => { frame = 0; if (!released) sendPlaced(); });
    };
    env.ctx.liveChanged?.();
    const release = () => {
      if (released) return;
      released = true;
      registered = false;
      for (const off of offs) off();
      if (frame) cancelAnimationFrame(frame);
      env.ctx.live = false;
      env.ctx.reflow = null;
      env.ctx.liveChanged?.();
      void rpc.call("comments", "release", []).catch(() => {});
    };
    try {
      await rpc.call("comments", "register", []);
    } catch (e) {
      release();
      throw e;
    }
    const gone = () => Promise.reject(invalid("this registration was released"));
    return {
      compose(anchor: unknown, at: unknown, opts?: unknown) {
        if (released) return gone();
        const dom = typeof anchor === "string" && minted.has(anchor);
        if (!dom) {
          const p = nameProblem(anchor);
          if (p) return Promise.reject(invalid(p));
        }
        if (cfg.composer_only === true && !dom) return Promise.reject(invalid("the composer-only declaration keeps only domAnchor paths"));
        if (at instanceof Element) {
          if (!at.isConnected) return Promise.reject(invalid("at is an element attached to the document or a point"));
          if (uncommentable(at)) return Promise.resolve({ opened: false });
        } else if (!isPoint(at)) {
          return Promise.reject(invalid("at is an element or a {x, y} point"));
        }
        const o = (opts ?? {}) as { label?: unknown; detail?: unknown; area?: unknown };
        for (const k of ["label", "detail"] as const) {
          if (o[k] !== undefined && (typeof o[k] !== "string" || (o[k] as string).length > MAX_LABEL)) return Promise.reject(invalid(`${k} is text of at most ${MAX_LABEL} characters`));
        }
        const base = { anchor, dom, label: o.label as string | undefined, detail: o.detail as string | undefined, version: env.ctx.version };
        // `area` is honoured only while it could be (comments.d.ts: `areas`).
        if (o.area !== true || !canArea) return rpc.call("comments", "compose", [base]);
        // An area on a domAnchor path: the shell checks the viewer's gesture
        // and opens the composer at once; the element's clip is rendered only
        // then and sent after it under the one-shot nonce the shell answered.
        // The page's `compose` result never carries the nonce (though the
        // page, sharing this window, could read the shell's reply).
        let el: Element | null = null;
        if (dom) { try { el = document.querySelector(anchor as string); } catch { el = null; } }
        return rpc.call("comments", "compose", [{ ...base, area: true, clipPending: !!el }]).then(r => {
          const res = (r ?? {}) as { opened?: unknown; clipNonce?: unknown };
          const nonce = res.clipNonce;
          if (el && typeof nonce === "string") {
            void (async () => {
              let clipPng: ArrayBuffer | undefined;
              let clipError: string | undefined;
              try { clipPng = await (await env.clip()).renderTargetClip(el); } catch (e) { clipError = e instanceof Error ? e.message : String(e); }
              await rpc.call("comments", "composeClip", [{ nonce, clipPng, clipError }]).catch(() => {});
            })();
          }
          return { opened: res.opened === true };
        });
      },
      open(id: unknown, at: unknown) {
        if (released) return gone();
        if (typeof id !== "string" || !id || !(at instanceof Element || isPoint(at))) return Promise.reject(invalid("open takes a thread handle and an element or point"));
        return rpc.call("comments", "openThread", [id]).then(() => undefined);
      },
      placed(map: unknown) {
        if (released || !map || typeof map !== "object") return;
        lastPlaced = Object.fromEntries(Object.entries(map as Record<string, unknown>).filter(([, p]) => isPoint(p)).map(([k, p]) => [k, { x: (p as DocPoint).x, y: (p as DocPoint).y }]));
        placedOnce = true;
        sendPlaced();
      },
      domAnchor(el: unknown, ev?: { clientX: number; clientY: number }): [string, DocPoint] {
        if (!(el instanceof Element) || !el.isConnected) throw new TypeError("domAnchor takes an element attached to the document");
        const path = cssPath(el);
        minted.add(path);
        const at = ev && Number.isFinite(ev.clientX) && Number.isFinite(ev.clientY) ? { x: ev.clientX + scrollX, y: ev.clientY + scrollY } : center(el);
        return [path, at];
      },
      exitMode() {
        if (!released) void rpc.call("comments", "exitMode", []).catch(() => {});
      },
      release,
      get areas() {
        return !released && canArea;
      },
    };
  };

  // Clax extensions (clax-extensions.d.ts): who is working, read-only, under
  // either declaration form. One shell subscription serves every `onWorking`.
  const workingFns = new Set<(s: unknown) => void>();
  let offWorking: (() => void) | null = null;
  const working = () => rpc.call("comments", "working", []);
  const onWorking = async (fn: unknown) => {
    if (typeof fn !== "function") throw invalid("onWorking takes a function");
    const f = fn as (s: unknown) => void;
    workingFns.add(f);
    if (!offWorking) {
      offWorking = rpc.on("comments", "working", d => { for (const g of workingFns) { try { g(d); } catch { /* the page's own error */ } } });
      await rpc.call("comments", "watchWorking", []);
    } else {
      try { f(await working()); } catch { /* the page's own error */ }
    }
    return () => {
      workingFns.delete(f);
      if (workingFns.size === 0 && offWorking) {
        offWorking();
        offWorking = null;
        void rpc.call("comments", "unwatchWorking", []).catch(() => {});
      }
    };
  };

  return {
    openComposer,
    anchorFor,
    working,
    onWorking,
    create: async (opts: unknown) => {
      const o = (opts ?? {}) as { anchor?: unknown; text?: unknown };
      const a = checkAnchor(o.anchor);
      return rpc.call("comments", "create", [{ anchor: toAnchor(env.ctx, a), text: text(o.text), version: env.ctx.version }]);
    },
    reply: async (threadId: unknown, t: unknown) => {
      if (typeof threadId !== "string" || !threadId) throw invalid("threadId is a string");
      return rpc.call("comments", "reply", [threadId, text(t)]);
    },
    sendToClaude: async (target: unknown) => {
      const t = (target ?? {}) as { anchor?: unknown; threadId?: unknown; text?: unknown };
      const body = text(t.text);
      if ((t.threadId === undefined) === (t.anchor === undefined)) throw invalid("sendToClaude takes exactly one of anchor and threadId");
      if (t.threadId !== undefined) {
        if (typeof t.threadId !== "string" || !t.threadId) throw invalid("threadId is a string");
        return rpc.call("comments", "sendToClaude", [{ threadId: t.threadId, text: body }]);
      }
      return rpc.call("comments", "sendToClaude", [{ anchor: toAnchor(env.ctx, checkAnchor(t.anchor)), text: body, version: env.ctx.version }]);
    },
    customAnchors,
  };
}
