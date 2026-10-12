// The Clax app's view (`/a/<id>?app=1`): the artifact view without chrome,
// driven by the app that hosts it in a WebView. `artifact-main.ts` loads this
// module only in app mode, so a browser's view carries none of it.
//
// - The host calls `window.claxHost` (by `runJavaScript`): comment mode, the
//   version shown, the open composer's text, Send and Cancel, a thread, and
//   the attestation of a finger on the page (`attestPointer`).
// - The view tells the host what changed through the WebView's JavaScript
//   channel `window.ClaxHost` (`HostMessage`, as JSON); a page without it (a
//   browser) drops them.
// - Auto-follow: a newer version loads in place, unless the view is pinned to
//   a version or a composer is open (it follows once that closes), on the page
//   and fragment the frame shows, and the page is asked to scroll back to
//   where it last reported (`clax:scroll`), in the first welcome only.
// - Send: the composer's primary button posts the thread and sends it to the
//   agent (`sendTo`), in one press. A send that fails leaves the thread
//   posted and unsent: the host hears `sendFailed`, and `resend(id)` tries
//   again.
//
// The channel protocol (what the app, plan Task F8, relies on):
// - The host is installed only in the top-level document at a shell path
//   `/a/<id>[/v/<n>][/<file>]` with `app=1` in its query; anywhere else
//   `installHost` does nothing and there is no `window.claxHost`.
// - The WebView injects the `ClaxHost` channel into every frame, the
//   artifact's page included, so the page can post on it too. To tell the
//   shell's messages from the page's, the app makes a fresh random nonce for
//   each load of the shell document and, once that document has loaded,
//   calls `claxHost.init({nonce})` by `runJavaScript` (main frame only).
// - Before `init` the shell posts nothing on the channel. From `init` on,
//   every message it posts carries `nonce`, and `init` first posts the
//   current `state`, `threads`, and the `ready` of the page shown if one
//   greeted. A second `init` in the same document is ignored.
// - The app drops every message whose `nonce` is not the one it gave this
//   load (and every message before it called `init`).
// - The page cannot learn the nonce: the shell's document is cross-origin to
//   it (another origin in subdomain mode, an opaque one in sandbox mode), so
//   it can read neither the shell's variables nor `window.claxHost`, and the
//   shell never posts the nonce to the frame. The channel's messages go from
//   the shell's frame to the app, never through the page's.
// Its styles go in with it: the shell links no stylesheet a lazy chunk
// imports (`vite.shell.config.ts`, `inlineCss`).
import appCss from "./app.css?inline";
import { getArtifact } from "../api";
import { acceptFromFrame } from "../bridge-link";
import { hostArrival } from "../caps/gesture";
import { now } from "../clock";
import { SEND_FAILED, failureText, report, scopedNotice } from "../failure";
import { type Thread, sendToAgent, upsert } from "../threads";
import type { ArtifactController, ViewState } from "../view/artifact-controller";
import { composerQuote } from "../view/composer-model";
import { appQuery, parseShellPath } from "../route";
import { setUrl } from "../view/url";

export type HostMessage =
  | { type: "ready"; artifact: string; version: number; latest: number; file: string }
  | { type: "state"; commenting: boolean; draft: { label: string; text: string; clip: "pending" | "ready" | "none" } | null; busy: boolean }
  | { type: "threads"; threads: { id: string; status: string; summary: string; sent: boolean; comments: number; version: number }[] }
  | { type: "sent"; thread: string }
  | { type: "landed"; version: number }
  | { type: "notice"; text: string }
  /** A posted thread's send to the agent failed (`text` says why): it stays
   * unsent until `resend(thread)`. */
  | { type: "sendFailed"; thread: string; text: string };
/** What goes on the channel: a `HostMessage` with this load's nonce. */
export type Posted = HostMessage & { nonce: string };

export type ClaxHost = {
  /** Starts the channel with this load's `nonce` (16 to 128 characters of
   * `A-Z a-z 0-9 _ -`); see the protocol above. Only the first valid call counts. */
  init(p: { nonce: string }): void;
  enterCommentMode(): void;
  exitCommentMode(): void;
  /** Shows version `n`, or the latest for null (a load of the shell). */
  loadVersion(n: number | null): void;
  /** Loads the latest version in place, keeping page, fragment and scroll. */
  reloadLatest(): void;
  /** Replaces the open composer's text; nothing without one. */
  setDraftText(text: string): void;
  /** Presses Send on the open composer (waiting for its screenshot as Send does). */
  submitDraft(): void;
  cancelDraft(): void;
  /** Selects the thread and brings it into view. */
  openThread(id: string): void;
  /** Sends an open thread that is not yet sent to the agent (after
   * `sendFailed`), as Send does: `sent`, or `sendFailed` again. */
  resend(id: string): void;
  /** A real finger `pointer` pressed (`down`) or lifted (`up`) at (`x`,
   * `y`), CSS pixels in the WebView's viewport, as the app's listener saw it
   * reach the WebView. A press on the content frame itself counts as the
   * finger's arrival there for `ATTEST_MS` (`caps/gesture.ts`, "App mode's
   * host attestation"); its release counts again, and only a release whose
   * press so counted: a press on a shell control (which may be gone by the
   * release) never does. */
  attestPointer(p: { pointer: number; phase: "down" | "up"; x: number; y: number }): void;
};

/** How long a host's attestation of a finger on the frame counts. A
 * long-press's pick comes at most about 400 ms after the release
 * (`web/bridge/src/comment-mode.ts`), and the release is attested too. */
export const ATTEST_MS = 1000;
/** A thread's summary: its first comment, cut to this many characters. */
const SUMMARY_CHARS = 140;
const FOLLOW_FAILED = "Could not load the latest version";

type Window_ = Window & { claxHost?: ClaxHost; ClaxHost?: { postMessage(m: string): void } };

function summary(t: Thread): string {
  const text = (t.comments[0]?.body ?? "").replace(/\s+/g, " ").trim();
  return text.length > SUMMARY_CHARS ? `${text.slice(0, SUMMARY_CHARS)}…` : text;
}

const NONCE = /^[A-Za-z0-9_-]{16,128}$/;

/** Installs `window.claxHost` over `ctl`, and, from `init` on, posts every
 * `HostMessage` with the nonce to `window.ClaxHost` (the app's channel) when
 * it exists. Nothing, outside the top-level document at an app shell path.
 * Returns the uninstall. */
export function installHost(win: Window, ctl: ArtifactController): () => void {
  const w = win as Window_;
  if (win.top !== win || parseShellPath(win.location.pathname).kind !== "artifact" || !appQuery(win.location.search)) return () => {};
  const doc = win.document;
  doc.documentElement.classList.add("app");
  const style = doc.createElement("style");
  style.textContent = appCss;
  doc.head.append(style);
  let nonce: string | null = null;
  const post = (m: HostMessage) => {
    const ch = w.ClaxHost;
    if (nonce !== null && ch && typeof ch.postMessage === "function") ch.postMessage(JSON.stringify({ ...m, nonce } satisfies Posted));
  };
  const s = () => ctl.state.get();

  // The page's last reported scroll position, per page.
  const scrolled = new Map<string, number>();
  // The scroll position the first welcome after a newer version loaded asks
  // the page to restore, and the version whose page's greeting is `landed`.
  let restore: { file: string; y: number } | null = null;
  let landing: number | null = null;
  // The latest page's `ready`, for `init`.
  let ready: HostMessage | null = null;
  const sendThread = (id: string) => {
    const to = s().sendTo;
    const notice = scopedNotice(ctl.setNotice, SEND_FAILED);
    void ctl.whileBusy(ctl.sendTracked(sendToAgent(ctl.id, id, to), to)).then(sent => {
      notice(null);
      ctl.changeThreads(ts => upsert(ts, sent));
      post({ type: "sent", thread: id });
    }, (e: unknown) => {
      const text = failureText(SEND_FAILED, e);
      notice(text);
      post({ type: "sendFailed", thread: id, text });
    });
  };
  ctl.app = {
    // A page greeted (each document the frame loads): `ready`, and `landed`
    // once the version auto-follow loaded greets.
    welcome: file => {
      const version = ctl.shown();
      ready = { type: "ready", artifact: ctl.id, version, latest: ctl.latest(), file };
      post(ready);
      if (landing !== null && version >= landing) {
        landing = null;
        post({ type: "landed", version });
      }
      const r = restore;
      restore = null;
      return r && r.file === file ? { app: true, restoreScroll: r.y } : { app: true };
    },
    posted: t => sendThread(t.id),
  };

  // Loads the latest version in place. Unless `force` (the host asked), only
  // while the view is unpinned with no composer open.
  let following = false;
  const follow = async (force: boolean) => {
    if (following) return;
    following = true;
    let loaded = false;
    try {
      const d = await report(getArtifact(ctl.id), FOLLOW_FAILED, scopedNotice(ctl.setNotice, FOLLOW_FAILED));
      const v = s();
      if (!d || ctl.disposed || ctl.app === null) return;
      if (!force && (v.pinnedVersion !== null || v.draft)) return;
      const n = d.artifact.current_version;
      if (v.pinnedVersion === null && n <= ctl.shown(v)) {
        if (v.newer !== null && v.newer <= n) ctl.set({ newer: null });
        return;
      }
      const file = v.file ?? v.startFile;
      const y = scrolled.get(file);
      restore = y === undefined ? null : { file, y };
      landing = n;
      if (v.pinnedVersion !== null) setUrl(ctl.here(null) + location.hash);
      // The new frame opens on the page and fragment the frame shows now;
      // the outgoing document is done, and no page is greeted until the new
      // one is.
      ctl.startHash = ctl.frameHash;
      ctl.set({ pinnedVersion: null, startFile: file, file: null, resolved: {} });
      ctl.loaded(d);
      loaded = true;
    } finally {
      following = false;
    }
    // A version published while this one loaded is followed in turn.
    if (loaded) followIfDue(s());
  };
  const followIfDue = (v: ViewState) => { if (v.newer !== null && v.pinnedVersion === null && !v.draft) void follow(false); };

  let lastState = "";
  let lastThreads: Thread[] | null = null;
  let lastNotice: string | null = null;
  const postState = (v: ViewState) => {
    const d = v.draft;
    const m: HostMessage = {
      type: "state",
      commenting: v.commenting,
      draft: d ? { label: composerQuote(d), text: ctl.composerText, clip: d.capturing ? "pending" : d.clip ? "ready" : "none" } : null,
      busy: v.busy > 0,
    };
    const json = JSON.stringify(m);
    if (json === lastState) return;
    lastState = json;
    post(m);
  };
  const onState = (v: ViewState) => {
    postState(v);
    if (v.threads !== lastThreads) {
      lastThreads = v.threads;
      post({ type: "threads", threads: v.threads.map(t => ({ id: t.id, status: t.status, summary: summary(t), sent: t.sent_to_agent, comments: t.comments.length, version: t.version_n })) });
    }
    if (v.notice !== lastNotice) {
      lastNotice = v.notice;
      if (v.notice) post({ type: "notice", text: v.notice });
    }
    followIfDue(v);
  };
  const unsubscribe = ctl.state.subscribe(onState);

  // The composer's text changes outside the store.
  const onInput = (e: Event) => {
    if (e.target instanceof Element && e.target.closest(".composer")) postState(s());
  };
  // The page's scroll reports, for its current page.
  const onMessage = (e: MessageEvent) => {
    const m = acceptFromFrame(e, ctl.frame?.el?.contentWindow ?? null, s().origin ?? null);
    if (m?.type !== "clax:scroll" || typeof m.file !== "string" || m.file !== s().file) return;
    if (typeof m.y === "number" && Number.isFinite(m.y) && m.y >= 0) scrolled.set(m.file, m.y);
  };
  win.addEventListener("input", onInput);
  win.addEventListener("message", onMessage);
  // A page that greeted before this module loaded was welcomed without the
  // app's hooks (no `ready`, no `app: true`): it is asked to greet again.
  if (ctl.gate.open) ctl.send({ type: "clax:greet" });

  let attestedAt = -Infinity;
  // The pointers whose press was attested on the frame, until their release.
  const pressed = new Set<number>();
  hostArrival.fresh = since => attestedAt > since && now() - attestedAt < ATTEST_MS;
  const composer = () => doc.querySelector<HTMLFormElement>(".stage form.composer");
  const host: ClaxHost = {
    init: p => {
      const n = p?.nonce;
      if (nonce !== null || typeof n !== "string" || !NONCE.test(n)) return;
      nonce = n;
      lastState = "";
      lastThreads = null;
      lastNotice = null;
      onState(s());
      if (ready) post(ready);
    },
    enterCommentMode: () => { if (!s().commenting) ctl.toggleComment(); },
    exitCommentMode: () => { if (s().commenting) ctl.toggleComment(); },
    loadVersion: n => {
      if (n === null) ctl.chooseVersion(ctl.latest());
      else if (Number.isInteger(n) && n > 0) ctl.chooseVersion(n);
    },
    reloadLatest: () => { void follow(true); },
    setDraftText: text => {
      const ta = composer()?.querySelector("textarea");
      if (!ta || !s().draft) return;
      ta.value = String(text);
      ta.dispatchEvent(new Event("input", { bubbles: true }));
    },
    submitDraft: () => { if (s().draft) composer()?.requestSubmit(); },
    cancelDraft: () => ctl.cancelDraft(),
    openThread: id => {
      const t = s().threads.find(x => x.id === id);
      if (t) ctl.selectThread(t);
    },
    resend: id => {
      const t = s().threads.find(x => x.id === id);
      if (t && t.status === "open" && !t.sent_to_agent) sendThread(id);
    },
    attestPointer: p => {
      const { x, y, pointer, phase } = p ?? {};
      if (typeof x !== "number" || typeof y !== "number" || !Number.isFinite(x) || !Number.isFinite(y) || !Number.isInteger(pointer)) return;
      if (phase === "down") {
        // On the frame itself: not on a shell control over it (the
        // composer, a pin, a banner), whose press is the shell's own input.
        const at = doc.elementFromPoint(x, y);
        if (at && at === ctl.frame?.el) { pressed.add(pointer); attestedAt = now(); } else pressed.delete(pointer);
      } else if (phase === "up" && pressed.delete(pointer)) attestedAt = now();
    },
  };
  Object.defineProperty(w, "claxHost", { value: host, configurable: true });

  return () => {
    unsubscribe();
    win.removeEventListener("input", onInput);
    win.removeEventListener("message", onMessage);
    hostArrival.fresh = () => false;
    if (ctl.app) ctl.app = null;
    delete w.claxHost;
    doc.documentElement.classList.remove("app");
    style.remove();
  };
}
