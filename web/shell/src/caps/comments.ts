// comments.d.ts in the shell. The composer verbs (openComposer, compose) open
// the phase 3 composer and need no consent, only the viewer's gesture; the
// write verbs (create, reply, sendToClaude, resolve, delete) post through the
// thread routes as this viewer after the viewer's consent, under the full
// declaration only, marked as written by the page (`via_page`, which also
// keeps an `@agent` in page text inert). customAnchors hands the page
// anonymous thread handles and takes pin positions back.
//
// The page never sees a thread's store ID: `create` answers an opaque handle,
// and the write verbs act only on threads this page created in its current
// document (anything else is `not_found`, with no request). Every argument
// the page supplies is checked here again (a page can post calls without the
// bridge); an anchor always names the page the frame shows and the version the
// view shows. Whatever speaks for the viewer beyond their consent (opening the
// composer, sending to the agent, a reply the agent will receive) needs the
// viewer's own recent gesture inside the frame (gesture.ts); refusals for the
// lack of one are budgeted, so a page polling on a timer is soon cut off. Artifax has no public links: every view
// that serves the declaration gets the namespace.
import { AFFIX, MAX_QUOTE, MAX_SELECTOR } from "../../../bridge/src/anchor";
import { type Anchor, type AnchorRect, type Box, INDEX_FILE } from "../../../bridge/src/protocol";
import { nameProblem, textProblem } from "../../../bridge/src/text-rule";
import { ApiError, getArtifact } from "../api";
import type { ArtifactEvent } from "../events";
import { type Thread, createThread, deleteThread, reopenThread, resolveThread, sendToAgent } from "../threads";
import { declaredConfig } from "./availability";
import { seconds, takeSlot } from "./budget";
import { CapError } from "./errors";
import { frameGesture } from "./gesture";
import type { HandlerFactory } from "./host";

export { MAX_TEXT_BYTES, textProblem } from "../../../bridge/src/text-rule";

/** Programmatic composer and card opens (openComposer, compose, open): at
 * most `n` per `ms` per artifact in a tab. */
export const OPEN_RATE = { n: 5, ms: 10_000 };
/** Page writes (create, reply, sendToClaude, resolve, delete): at most `n`
 * per `ms` per artifact in a tab. */
export const WRITE_RATE = { n: 10, ms: 60_000 };
/** Gesture-gated calls refused for want of a frame gesture: at most `n` per
 * `ms` per artifact in a tab, then `rate_limited`. */
export const REFUSED_RATE = { n: 20, ms: 60_000 };
/** Most threads one `threads` push lists, and most entries a `placed` report may carry. */
export const MAX_LISTED = 256;
/** Longest `label` or `detail`, in UTF-16 code units, and the bytes of UTF-8 of a label shown. */
export const MAX_LABEL = 1024;
export const MAX_LABEL_BYTES = 128;
/** Largest clip a page-opened composer takes (the daemon's `MAX_CLIP_BYTES`). */
export const MAX_CLIP_BYTES = 5 * 1024 * 1024;
/** How long a `canSendToClaude` answer is reused when no event says the sessions changed. */
export const CAN_SEND_TTL_MS = 30_000;
const HTML_HASH = /^sha256:[0-9a-f]{64}$/;
const CONTROL = /[\p{Cc}\p{Zl}\p{Zp}]/u;

const invalid = (message: string) => new CapError("invalid", message);

/** Whether an area compose could start now (comments.d.ts `areas`): comment
 * mode is on and no post or send is in flight. */
export const canArea = (s: { mode: boolean; busy?: boolean }) => s.mode && !s.busy;

/** A one-shot nonce naming an area compose's pending clip. */
function clipNonce(): string {
  const b = new Uint8Array(12);
  crypto.getRandomValues(b);
  return Array.from(b, x => x.toString(16).padStart(2, "0")).join("");
}
const notFound = () => new CapError("not_found", "no such thread: the page can act only on threads it created in this visit");

/** An opaque, unguessable handle. */
function opaque(prefix: string): string {
  const b = new Uint8Array(12);
  crypto.getRandomValues(b);
  return prefix + Array.from(b, x => x.toString(16).padStart(2, "0")).join("");
}

/** The label a page-opened composer shows (it is never stored): whitespace
 * collapsed, control and invisible characters dropped, at most
 * [`MAX_LABEL_BYTES`] of UTF-8; null when no letter or digit remains. */
export function cleanLabel(label: string | undefined): string | null {
  if (label === undefined) return null;
  let s = label.replace(/[\p{Cf}]/gu, "").replace(/\s+/g, " ").replace(/\p{Cc}/gu, "").trim();
  const enc = new TextEncoder();
  let chars = [...s];
  while (chars.length && enc.encode(chars.join("")).length > MAX_LABEL_BYTES) chars = chars.slice(0, -1);
  s = chars.join("").trim();
  return /[\p{L}\p{N}]/u.test(s) ? s : null;
}

const optText = (v: unknown, max: number, name: string): string | null => {
  if (v === null || v === undefined) return null;
  if (typeof v !== "string" || v.length > max) throw invalid(`anchor.${name} is text of at most ${max} characters`);
  return v;
};
const finite = (v: unknown) => typeof v === "number" && Number.isFinite(v);

/** A DOM anchor the page built (element or range), rebuilt field by field on
 * page `file`; `invalid` for anything else. */
export function pageAnchor(a: unknown, file: string): Anchor {
  if (!a || typeof a !== "object") throw invalid("anchor is an object");
  const x = a as Record<string, unknown>;
  if (x.kind !== "element" && x.kind !== "range") throw invalid("anchor.kind is element or range");
  if (typeof x.selector !== "string" || !x.selector || x.selector.length > MAX_SELECTOR || CONTROL.test(x.selector)) {
    throw invalid(`anchor.selector is a CSS path of at most ${MAX_SELECTOR} characters`);
  }
  const hash = x.html_hash;
  if (hash !== null && hash !== undefined && (typeof hash !== "string" || !HTML_HASH.test(hash))) throw invalid("anchor.html_hash is sha256:<hex>");
  let rect: AnchorRect | null = null;
  if (x.rect !== null && x.rect !== undefined) {
    const r = x.rect as Record<string, unknown>;
    const keys = ["x", "y", "w", "h", "scrollX", "scrollY", "viewportW"] as const;
    if (typeof r !== "object" || !keys.every(k => finite(r[k]))) throw invalid("anchor.rect holds finite numbers");
    rect = Object.fromEntries(keys.map(k => [k, r[k]])) as unknown as AnchorRect;
  }
  return {
    kind: x.kind,
    selector: x.selector,
    quote: optText(x.quote, MAX_QUOTE, "quote"),
    prefix: optText(x.prefix, AFFIX, "prefix"),
    suffix: optText(x.suffix, AFFIX, "suffix"),
    html_hash: (hash as string | null | undefined) ?? null,
    rect,
    custom_name: null,
    file,
  };
}


function mapped(e: unknown): CapError {
  if (e instanceof CapError) return e;
  if (e instanceof ApiError) {
    if (e.status === 404) return new CapError("not_found", "no such thread on this artifact");
    if (e.status === 400 || e.status === 413) return invalid(e.message);
    if (e.status === 403) return new CapError("forbidden", e.message);
    if (e.status === 429) return new CapError("rate_limited", e.message);
    return new CapError("upstream_error", e.message);
  }
  return new CapError("unavailable", e instanceof Error ? e.message : String(e));
}

const event = (topic: string, data: unknown) => ({ type: "artifax:event" as const, ns: "comments", topic, data });
type Listed = { id: string; anchor: string; resolved: boolean; active: boolean };

export const commentsHandler: HandlerFactory = (env, grants) => {
  const cfg = declaredConfig("comments", env.declared) as { composer_only?: unknown; customAnchors?: unknown };
  const composerOnly = cfg.composer_only === true;
  let disposed = false;
  /** Threads this page created in its current document: opaque handle to store ID. */
  let created = new Map<string, string>();
  /** A custom-anchors registration of the frame's current document is live. */
  let custom = false;
  let placedOnce = false;
  /** The page's own compose started the current comment-mode session: it is
   * sent no thread list (comments.d.ts). */
  let pageStarted = false;
  let handleToId = new Map<string, string>();
  let idToHandle = new Map<string, string>();
  /** What the page was last sent, so a push goes only when something changed.
   * `listed` holds the handles of the last list, which placements may name
   * (after comment mode ends too, so pins keep following the page). */
  let sent: { mode: boolean | null; canArea: boolean | null; composing: boolean | null; threads: string; listed: Set<string> } = { mode: null, canArea: null, composing: null, threads: "", listed: new Set() };
  /** Nonces of area composes whose clip is still to come (`composeClip`). */
  const pendingClips = new Set<string>();
  /** The cached `canSendToClaude` answer and when it was asked. */
  let canSendCache: { at: number; answer: Promise<"available" | "no_session"> } | null = null;

  const closed = () => new CapError("unavailable", "this view has closed");
  const ui = () => {
    if (!env.comments) throw new CapError("unavailable", "this view has no comments UI");
    return env.comments;
  };
  /** The published file of the page in the frame; `unavailable` while none greeted. */
  const pageFile = (): string => {
    const f = env.page ? env.page() : INDEX_FILE;
    if (f === null) throw new CapError("unavailable", "the frame shows no page of this artifact");
    return f;
  };
  const budget = (kind: "opens" | "writes" | "refusals") => {
    const limits = kind === "opens" ? OPEN_RATE : kind === "writes" ? WRITE_RATE : REFUSED_RATE;
    return takeSlot(`artifax.comment-${kind}.v1:${env.aid}`, { perWindow: limits });
  };
  const opening = () => {
    const wait = budget("opens");
    if (wait > 0) throw new CapError("rate_limited", `the page opens the composer too often; wait ${seconds(wait)} s`);
  };
  /** Whether the viewer just acted in the frame; a refusal is charged to its budget. */
  const gestured = (): boolean => {
    if (frameGesture()) return true;
    const wait = budget("refusals");
    if (wait > 0) throw new CapError("rate_limited", `the page calls without the viewer's gesture too often; wait ${seconds(wait)} s`);
    return false;
  };
  const writing = () => {
    const wait = budget("writes");
    if (wait > 0) throw new CapError("rate_limited", `the page writes comments too often; wait ${seconds(wait)} s`);
  };
  const text = (t: unknown) => {
    const p = textProblem(t);
    if (p) throw invalid(p);
    return t as string;
  };
  const writable = () => {
    if (composerOnly) throw new CapError("not_granted", "the composer-only declaration grants no write verbs");
  };
  /** The store ID behind a handle `create` gave this document; `not_found` otherwise. */
  const own = (v: unknown): string => {
    if (typeof v !== "string" || !v) throw invalid("threadId is the threadId create resolved with");
    const id = created.get(v);
    if (!id) throw notFound();
    return id;
  };
  async function consent(): Promise<void> {
    await grants.request(["comments"]);
    if (disposed) throw closed();
    if (grants.state("comments") !== "granted") {
      throw new CapError(grants.refusal("comments") ?? "consent_required", "the viewer has not allowed this page to comment as them");
    }
  }
  /** Runs one request; after dispose its result is dropped. */
  async function request<T>(p: () => Promise<T>): Promise<T> {
    let v: T;
    try {
      v = await p();
    } catch (e) {
      throw disposed ? closed() : mapped(e);
    }
    if (disposed) throw closed();
    return v;
  }

  const handleOf = (id: string) => {
    let h = idToHandle.get(id);
    if (!h) {
      h = `h${idToHandle.size + 1}`;
      idToHandle.set(id, h);
      handleToId.set(h, id);
    }
    return h;
  };
  /** The threads on the page the frame shows, as comments.d.ts discloses them. */
  const threadList = (): Listed[] => {
    const s = ui().state();
    const f = env.page ? env.page() : INDEX_FILE;
    return s.threads
      .filter(t => t.anchor.file === f)
      .slice(0, MAX_LISTED)
      .map(t => ({
        id: handleOf(t.id),
        anchor: t.anchor.kind === "custom" ? (t.anchor.custom_name ?? "") : (t.anchor.selector ?? ""),
        resolved: t.status === "resolved",
        active: s.selected === t.id,
      }));
  };
  const pushState = () => {
    if (!custom || disposed || !env.comments) return;
    const s = env.comments.state();
    if (s.mode !== sent.mode || canArea(s) !== sent.canArea) {
      sent.mode = s.mode;
      sent.canArea = canArea(s);
      env.post(event("mode", { on: s.mode, canArea: sent.canArea }));
    }
    if (s.composing !== sent.composing) {
      sent.composing = s.composing;
      env.post(event("composing", { open: s.composing }));
    }
    if (!s.mode) {
      // The next session gets the list again; the last list's handles stay
      // valid for placements, so pins keep following the page.
      sent.threads = "";
      pageStarted = false;
      return;
    }
    if (pageStarted) return;
    const list = threadList();
    const key = JSON.stringify(list);
    if (key === sent.threads) return;
    sent.threads = key;
    sent.listed = new Set(list.map(t => t.id));
    env.post(event("threads", { list }));
  };
  const endCustom = () => {
    const was = custom;
    custom = false;
    placedOnce = false;
    pageStarted = false;
    handleToId = new Map();
    idToHandle = new Map();
    sent = { mode: null, canArea: null, composing: null, threads: "", listed: new Set() };
    pendingClips.clear();
    if (was) env.comments?.setCustom(false);
  };

  function canSend(): Promise<"available" | "no_session" | "off"> {
    if (composerOnly) return Promise.resolve("off");
    if (!canSendCache || Date.now() - canSendCache.at > CAN_SEND_TTL_MS) {
      canSendCache = {
        at: Date.now(),
        answer: getArtifact(env.aid).then(a => (a.artifact.owner_live ? "available" as const : "no_session" as const), () => "no_session" as const),
      };
    }
    return canSendCache.answer;
  }
  /** Posts page-written text as a new thread or a reply; returns the thread
   * handle the page may use again, an opaque comment ID, and the thread. */
  async function post(target: { anchor?: Anchor; threadId?: string; text: string }): Promise<{ handle: string; commentId: string; thread: Thread }> {
    if (target.threadId !== undefined) {
      const tid = target.threadId;
      const thread = await request(() => addPageComment(env.aid, tid, target.text));
      env.comments?.upsert(thread);
      const handle = [...created].find(([, id]) => id === tid)?.[0] ?? opaque("t_");
      return { handle, commentId: opaque("c_"), thread };
    }
    const anchor = target.anchor!;
    const { thread } = await request(() => createThread(env.aid, { anchor, body: target.text, version: env.version, clip: null, viaPage: true }));
    env.comments?.upsert(thread);
    const handle = opaque("t_");
    created.set(handle, thread.id);
    return { handle, commentId: opaque("c_"), thread };
  }
  /** A reply into a thread the agent will receive speaks for the viewer to
   * the agent: it needs their recent gesture. */
  const replyGesture = (tid: string) => {
    const t = env.comments?.state().threads.find(x => x.id === tid);
    if ((t ? t.sent_to_agent : true) && !gestured()) {
      throw new CapError("unavailable", "a reply the agent receives needs the viewer's own gesture; call it from their click");
    }
  };

  return {
    async call(method, args) {
      if (disposed) throw closed();
      switch (method) {
        case "openComposer": {
          const d = (args[0] ?? {}) as { anchor?: unknown; clipPng?: unknown; clipError?: unknown };
          const anchor = pageAnchor(d.anchor, pageFile());
          const u = ui();
          // Only from the viewer's own gesture: a timer or load never opens (or focuses) it.
          if (!gestured()) return { opened: false };
          opening();
          const clip = d.clipPng instanceof ArrayBuffer && d.clipPng.byteLength > 0 && d.clipPng.byteLength <= MAX_CLIP_BYTES ? new Blob([d.clipPng], { type: "image/png" }) : null;
          const clipError = clip ? undefined : typeof d.clipError === "string" ? d.clipError.slice(0, 200) : d.clipPng instanceof ArrayBuffer ? "the screenshot was too large" : undefined;
          return { opened: u.openComposer({ anchor, version: env.version, clip, clipError }) };
        }
        case "create": {
          writable();
          const d = (args[0] ?? {}) as { anchor?: unknown; text?: unknown };
          const anchor = pageAnchor(d.anchor, pageFile());
          const body = text(d.text);
          await consent();
          writing();
          const r = await post({ anchor, text: body });
          return { threadId: r.handle, commentId: r.commentId };
        }
        case "reply": {
          writable();
          const tid = own(args[0]);
          const body = text(args[1]);
          replyGesture(tid);
          await consent();
          writing();
          return { commentId: (await post({ threadId: tid, text: body })).commentId };
        }
        case "resolve": {
          writable();
          const tid = own(args[0]);
          if (typeof args[1] !== "boolean") throw invalid("resolved is true or false");
          const reopen = args[1] === false;
          await consent();
          writing();
          const t = await request(() => (reopen ? reopenThread(env.aid, tid, env.token) : resolveThread(env.aid, tid)));
          env.comments?.upsert(t);
          return undefined;
        }
        case "delete": {
          writable();
          const handle = args[0];
          const tid = own(handle);
          await consent();
          writing();
          await request(() => deleteThread(env.aid, tid, env.token));
          created.delete(handle as string);
          env.comments?.remove(tid);
          return undefined;
        }
        case "canSendToClaude":
          return canSend();
        case "sendToClaude": {
          writable();
          const t = (args[0] ?? {}) as { anchor?: unknown; threadId?: unknown; text?: unknown };
          if ((t.threadId === undefined) === (t.anchor === undefined)) throw invalid("sendToClaude takes exactly one of anchor and threadId");
          const target = t.threadId !== undefined ? { threadId: own(t.threadId) } : { anchor: pageAnchor(t.anchor, pageFile()) };
          const body = text(t.text);
          // Decided before anything is written: the viewer's own recent
          // gesture, and a session that can receive it.
          if (!gestured()) throw new CapError("claude_unavailable", "sending to the agent needs the viewer's own gesture; nothing was posted");
          if ((await canSend()) !== "available") throw new CapError("claude_unavailable", "no agent session can receive it now; nothing was posted");
          if (disposed) throw closed();
          await consent();
          writing();
          const r = await post({ ...target, text: body });
          if (!r.thread.sent_to_agent) {
            const sentThread = await request(() => sendToAgent(env.aid, r.thread.id));
            env.comments?.upsert(sentThread);
          }
          return { threadId: r.handle, commentId: r.commentId };
        }
        case "register":
          if (cfg.customAnchors !== true) throw new CapError("not_granted", "the declaration does not carry \"customAnchors\": true");
          if (custom) throw invalid("a registration is already live");
          ui().setCustom(true);
          custom = true;
          pushState();
          return null;
        case "release":
          endCustom();
          return null;
        case "compose": {
          if (!custom) throw invalid("no custom-anchors registration is live");
          const d = (args[0] ?? {}) as { anchor?: unknown; dom?: unknown; label?: unknown; detail?: unknown; area?: unknown; clipPending?: unknown };
          const dom = d.dom === true;
          // `area` (comments.d.ts ComposeOptions) only while comment mode is on.
          const area = d.area === true && canArea(ui().state());
          if (dom) {
            if (typeof d.anchor !== "string" || !d.anchor || d.anchor.length > MAX_SELECTOR || CONTROL.test(d.anchor)) throw invalid("a domAnchor path is a CSS path");
          } else {
            const p = nameProblem(d.anchor);
            if (p) throw invalid(p);
          }
          if (composerOnly && !dom) throw invalid("the composer-only declaration keeps only domAnchor paths");
          for (const k of ["label", "detail"] as const) {
            if (d[k] !== undefined && (typeof d[k] !== "string" || (d[k] as string).length > MAX_LABEL)) throw invalid(`${k} is text of at most ${MAX_LABEL} characters`);
          }
          const file = pageFile();
          const u = ui();
          if (!gestured()) return { opened: false };
          // The page's compose is the viewer's click: over an open composer or
          // thread card it closes an empty one (typed text is kept) instead;
          // a drawn area (`area`) opens the composer there instead, moving one
          // that holds typed text to the new anchor.
          const s = u.state();
          if (!area && (s.composing || s.selected !== null)) {
            u.dismiss?.();
            return { opened: false };
          }
          opening();
          // Only the anchor is stored; the label is shown in the composer.
          const base = { quote: null, prefix: null, suffix: null, html_hash: null, rect: null, file };
          const label = cleanLabel(d.label as string | undefined) ?? undefined;
          // The page's rectangle is not known here, so an area on a domAnchor
          // path is anchored to that element; its clip, of the element, is
          // rendered after this check passed and arrives through
          // `composeClip` with the one-shot nonce answered here.
          if (area && dom && d.clipPending === true) {
            const nonce = clipNonce();
            const anchor: Anchor = { kind: "element", selector: d.anchor as string, custom_name: null, ...base };
            const opened = u.openComposer({ anchor, version: env.version, clip: null, clipError: undefined, label, capturing: true, clipToken: nonce }, { area });
            if (!opened) return { opened };
            pendingClips.add(nonce);
            return { opened, clipNonce: nonce };
          }
          const anchor: Anchor = dom
            ? { kind: "element", selector: d.anchor as string, custom_name: null, ...base }
            : { kind: "custom", selector: null, custom_name: d.anchor as string, ...base };
          if (!s.mode) {
            pageStarted = true;
            u.enterMode?.();
          }
          return { opened: u.openComposer({ anchor, version: env.version, clip: null, clipError: "anchored by the page", label }, area ? { area } : undefined) };
        }
        case "composeClip": {
          // The clip for a composer an area compose opened; a nonce is used once.
          const d = (args[0] ?? {}) as { nonce?: unknown; clipPng?: unknown; clipError?: unknown };
          if (typeof d.nonce !== "string" || !pendingClips.delete(d.nonce)) return null;
          const clip = d.clipPng instanceof ArrayBuffer && d.clipPng.byteLength > 0 && d.clipPng.byteLength <= MAX_CLIP_BYTES ? new Blob([d.clipPng], { type: "image/png" }) : null;
          const clipError = clip ? undefined : d.clipPng instanceof ArrayBuffer && d.clipPng.byteLength > MAX_CLIP_BYTES ? "the screenshot was too large" : typeof d.clipError === "string" ? d.clipError.slice(0, 200) : "no screenshot was taken";
          ui().attachClip?.(d.nonce, clip, clipError);
          return null;
        }
        case "openThread": {
          if (typeof args[0] !== "string") throw invalid("open takes a thread handle");
          const id = handleToId.get(args[0]);
          // Handles not in the current list, and opens past the rate, are dropped.
          if (!custom || !id || !sent.listed.has(args[0]) || budget("opens") > 0) return null;
          ui().select(id);
          return null;
        }
        case "placed": {
          if (!custom) return null;
          const map = args[0];
          if (!map || typeof map !== "object") throw invalid("placed takes a map of handles to points");
          placedOnce = true;
          const rects: Record<string, Box> = {};
          for (const [h, p] of Object.entries(map as Record<string, unknown>).slice(0, MAX_LISTED)) {
            const id = handleToId.get(h);
            const pt = p as { x?: unknown; y?: unknown } | null;
            if (id && sent.listed.has(h) && pt && finite(pt.x) && finite(pt.y)) rects[id] = { x: pt.x as number, y: pt.y as number, w: 0, h: 0 };
          }
          ui().place(rects);
          return null;
        }
        case "exitMode":
          if (custom) ui().exitMode();
          return null;
        default:
          throw new CapError("capability_removed", `comments.${String(method)} is not part of this runtime`);
      }
    },
    onEvent(e: ArtifactEvent) {
      // Sessions starting or ending show in these; the next check asks again.
      if (e.type === "version" || e.type === "feedback_state" || e.type === "ready" || e.type === "resync") canSendCache = null;
      if (e.type === "thread" || e.type === "thread_resolved" || e.type === "thread_deleted") pushState();
    },
    uiChanged() {
      pushState();
    },
    reveal(tid) {
      // While registered the shell never scrolls the frame itself; the page is
      // asked, once it has placed pins, for threads in the list it was sent.
      if (!custom || disposed) return false;
      const h = idToHandle.get(tid);
      if (placedOnce && h && sent.listed.has(h)) env.post(event("reveal", { id: h }));
      return true;
    },
    reset() {
      created = new Map();
      canSendCache = null;
      endCustom();
    },
    dispose() {
      disposed = true;
      created = new Map();
      endCustom();
    },
  };
};

/** Adds a viewer comment the page wrote (`via_page`); answers the thread. */
async function addPageComment(aid: string, tid: string, body: string): Promise<Thread> {
  const res = await fetch(`/api/artifacts/${encodeURIComponent(aid)}/threads/${encodeURIComponent(tid)}/comments`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ body, via_page: true }),
  });
  if (!res.ok) {
    let msg = res.statusText;
    try { msg = (await res.json()).error?.message ?? msg; } catch { /* not JSON */ }
    throw new ApiError(res.status, msg);
  }
  return ((await res.json()) as { thread: Thread }).thread;
}
