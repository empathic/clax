// comments.d.ts in the shell. The composer verbs (openComposer, compose) open
// the phase 3 composer and need no consent; the write verbs (create, reply,
// sendToClaude, resolve, delete) post through the thread routes as this
// viewer after the viewer's consent, under the full declaration only;
// customAnchors hands the page anonymous thread handles and takes pin
// positions back. Every argument the page supplies is checked here again (a
// page can post calls without the bridge); an anchor always names the page
// the frame shows and the version the view shows, whatever the page sent.
// Artifax has no public links: every view that serves the declaration gets
// the namespace.
import { AFFIX, MAX_QUOTE, MAX_SELECTOR } from "../../../bridge/src/anchor";
import { type Anchor, type AnchorRect, type Box, INDEX_FILE } from "../../../bridge/src/protocol";
import { nameProblem, textProblem } from "../../../bridge/src/text-rule";
import { ApiError, getArtifact } from "../api";
import { type Thread, createThread, deleteThread, reopenThread, resolveThread, sendToAgent } from "../threads";
import { declaredConfig } from "./availability";
import { seconds, takeSlot } from "./budget";
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

export { MAX_TEXT_BYTES, textProblem } from "../../../bridge/src/text-rule";

/** Programmatic composer and card opens (openComposer, compose, open): at
 * most `n` per `ms` per artifact in a tab. */
export const OPEN_RATE = { n: 5, ms: 10_000 };
/** Page writes (create, reply, sendToClaude, resolve, delete): at most `n`
 * per `ms` per artifact in a tab. */
export const WRITE_RATE = { n: 10, ms: 60_000 };
/** Most threads one `threads` push lists, and most entries a `placed` report may carry. */
export const MAX_LISTED = 256;
/** Longest `label` or `detail`, in UTF-16 code units, and the bytes of UTF-8 of a label kept. */
export const MAX_LABEL = 1024;
export const MAX_LABEL_BYTES = 128;
/** Largest clip a page-opened composer takes (the daemon's `MAX_CLIP_BYTES`). */
export const MAX_CLIP_BYTES = 5 * 1024 * 1024;
const THREAD_ID = /^[0-9A-Za-z]{1,64}$/;
const HTML_HASH = /^sha256:[0-9a-f]{64}$/;
const CONTROL = /[\p{Cc}\p{Zl}\p{Zp}]/u;

const invalid = (message: string) => new CapError("invalid", message);

/** Whether `body` mentions `@agent` as a word, by the daemon's rule
 * (`mentions_agent`): such a comment is sent to the agent when posted. */
export function mentionsAgent(body: string): boolean {
  const word = (c: string | undefined) => c !== undefined && /[\p{Alphabetic}\p{N}_-]/u.test(c);
  for (let i = body.indexOf("@agent"); i >= 0; i = body.indexOf("@agent", i + 1)) {
    const before = [...body.slice(0, i)].at(-1);
    const [after, next] = [...body.slice(i + "@agent".length, i + "@agent".length + 4)];
    const beforeOk = before === undefined || (!word(before) && before !== ".");
    const afterOk = after === undefined || (after === "." ? !word(next) : !word(after));
    if (beforeOk && afterOk) return true;
  }
  return false;
}

/** The label a composer shows and the anchor keeps as its quote: whitespace
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

function threadId(v: unknown): string {
  if (typeof v !== "string" || !THREAD_ID.test(v)) throw invalid("threadId is a thread identifier");
  return v;
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
  /** A custom-anchors registration of the frame's current document is live. */
  let custom = false;
  let placedOnce = false;
  let handleToId = new Map<string, string>();
  let idToHandle = new Map<string, string>();
  /** What the page was last sent, so a push goes only when something changed. */
  let sent: { mode: boolean | null; composing: boolean | null; threads: string; listed: Set<string> } = { mode: null, composing: null, threads: "", listed: new Set() };

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
  const budget = (kind: "opens" | "writes") => {
    const limits = kind === "opens" ? OPEN_RATE : WRITE_RATE;
    return takeSlot(`artifax.comment-${kind}.v1:${env.aid}`, { perWindow: limits });
  };
  const opening = () => {
    const wait = budget("opens");
    if (wait > 0) throw new CapError("rate_limited", `the page opens the composer too often; wait ${seconds(wait)} s`);
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
  /** Page text for a plain write: `@agent` would send it to the agent, which
   * only sendToClaude may do. */
  const plain = (t: unknown) => {
    const body = text(t);
    if (mentionsAgent(body)) throw invalid("text mentioning @agent would send it to the agent; use sendToClaude for that");
    return body;
  };
  const writable = () => {
    if (composerOnly) throw new CapError("not_granted", "the composer-only declaration grants no write verbs");
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
    if (s.mode !== sent.mode) {
      sent.mode = s.mode;
      env.post(event("mode", { on: s.mode }));
    }
    if (s.composing !== sent.composing) {
      sent.composing = s.composing;
      env.post(event("composing", { open: s.composing }));
    }
    if (!s.mode) {
      sent.threads = "";
      sent.listed = new Set();
      return;
    }
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
    handleToId = new Map();
    idToHandle = new Map();
    sent = { mode: null, composing: null, threads: "", listed: new Set() };
    if (was) env.comments?.setCustom(false);
  };

  async function canSend(): Promise<"available" | "no_session" | "off"> {
    if (composerOnly) return "off";
    const a = await getArtifact(env.aid).catch(() => null);
    return a?.artifact.owner_live ? "available" : "no_session";
  }
  async function post(target: { anchor?: Anchor; threadId?: string; text: string }): Promise<{ threadId: string; commentId: string; thread: Thread }> {
    if (target.threadId !== undefined) {
      const tid = target.threadId;
      const r = await request(() => addCommentFull(env.aid, tid, target.text));
      env.comments?.upsert(r.thread);
      return { threadId: tid, commentId: r.commentId, thread: r.thread };
    }
    const anchor = target.anchor!;
    const { thread } = await request(() => createThread(env.aid, { anchor, body: target.text, version: env.version, clip: null }));
    env.comments?.upsert(thread);
    return { threadId: thread.id, commentId: thread.comments[0]?.id ?? "", thread };
  }

  return {
    async call(method, args) {
      if (disposed) throw closed();
      switch (method) {
        case "openComposer": {
          const d = (args[0] ?? {}) as { anchor?: unknown; clipPng?: unknown; clipError?: unknown };
          const anchor = pageAnchor(d.anchor, pageFile());
          const u = ui();
          opening();
          const clip = d.clipPng instanceof ArrayBuffer && d.clipPng.byteLength > 0 && d.clipPng.byteLength <= MAX_CLIP_BYTES ? new Blob([d.clipPng], { type: "image/png" }) : null;
          const clipError = clip ? undefined : typeof d.clipError === "string" ? d.clipError.slice(0, 200) : d.clipPng instanceof ArrayBuffer ? "the screenshot was too large" : undefined;
          return { opened: u.openComposer({ anchor, version: env.version, clip, clipError }) };
        }
        case "create": {
          writable();
          const d = (args[0] ?? {}) as { anchor?: unknown; text?: unknown };
          const anchor = pageAnchor(d.anchor, pageFile());
          const body = plain(d.text);
          await consent();
          writing();
          const r = await post({ anchor, text: body });
          return { threadId: r.threadId, commentId: r.commentId };
        }
        case "reply": {
          writable();
          const tid = threadId(args[0]);
          const body = plain(args[1]);
          await consent();
          writing();
          return { commentId: (await post({ threadId: tid, text: body })).commentId };
        }
        case "resolve": {
          writable();
          const tid = threadId(args[0]);
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
          const tid = threadId(args[0]);
          await consent();
          writing();
          await request(() => deleteThread(env.aid, tid, env.token));
          env.comments?.remove(tid);
          return undefined;
        }
        case "canSendToClaude":
          return canSend();
        case "sendToClaude": {
          writable();
          const t = (args[0] ?? {}) as { anchor?: unknown; threadId?: unknown; text?: unknown };
          if ((t.threadId === undefined) === (t.anchor === undefined)) throw invalid("sendToClaude takes exactly one of anchor and threadId");
          const target = t.threadId !== undefined ? { threadId: threadId(t.threadId) } : { anchor: pageAnchor(t.anchor, pageFile()) };
          const body = text(t.text);
          if ((await canSend()) !== "available") throw new CapError("claude_unavailable", "no agent session can receive it now; nothing was posted");
          if (disposed) throw closed();
          await consent();
          writing();
          const r = await post({ ...target, text: body });
          if (!r.thread.sent_to_agent) {
            const sentThread = await request(() => sendToAgent(env.aid, r.threadId));
            env.comments?.upsert(sentThread);
          }
          return { threadId: r.threadId, commentId: r.commentId };
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
          const d = (args[0] ?? {}) as { anchor?: unknown; dom?: unknown; label?: unknown; detail?: unknown };
          const dom = d.dom === true;
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
          opening();
          const base = { quote: cleanLabel(d.label as string | undefined), prefix: null, suffix: null, html_hash: null, rect: null, file };
          const anchor: Anchor = dom
            ? { kind: "element", selector: d.anchor as string, custom_name: null, ...base }
            : { kind: "custom", selector: null, custom_name: d.anchor as string, ...base };
          return { opened: u.openComposer({ anchor, version: env.version, clip: null, clipError: "anchored by the page" }) };
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
    onEvent(e) {
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
      endCustom();
    },
    dispose() {
      disposed = true;
      endCustom();
    },
  };
};

/** Adds a viewer comment, answering the new comment's ID with the thread
 * (`addComment` in threads.ts answers the thread only). */
async function addCommentFull(aid: string, tid: string, body: string): Promise<{ thread: Thread; commentId: string }> {
  const res = await fetch(`/api/artifacts/${encodeURIComponent(aid)}/threads/${encodeURIComponent(tid)}/comments`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ body }),
  });
  if (!res.ok) {
    let msg = res.statusText;
    try { msg = (await res.json()).error?.message ?? msg; } catch { /* not JSON */ }
    throw new ApiError(res.status, msg);
  }
  const v = (await res.json()) as { comment: { id: string }; thread: Thread };
  return { thread: v.thread, commentId: v.comment.id };
}
