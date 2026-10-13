// The Clax app's view (`/a/<id>?app=1`): the artifact view without chrome,
// driven by the app that hosts it in a WebView. `artifact-main.ts` loads this
// module only in app mode, so a browser's view carries none of it.
//
// - The host calls `window.claxHost` (by `runJavaScript`): comment mode, the
//   version shown, the open composer's text, Send and Cancel, a thread, and
//   the attestation of a finger on the page (`attestPointer`), a composer
//   on the whole app (`composeArtifact`), and the questions sheet
//   (`openQuestions`).
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
// - Visibility: a WebView may report the shell's document hidden while the
//   app shows it (macOS WKWebView does from load on), and a page hidden for
//   30 s stops following its artifact (`stream.ts`, `HIDDEN_MS`), so no
//   change would land. From `init` on, the shell counts itself visible
//   unless the app says otherwise with `claxHost.setVisible(false)` (its
//   lifecycle: paused, hidden, or the app screen not shown), and
//   `setVisible(true)` takes the artifact's events again, refetching what it
//   missed. Before `init` the document's own visibility applies.
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
// - The inspect view (`?app=1&inspect=1`, spec 2026-10-12-flutter-app-
//   round2-design R§5.2): the app loads a draft of an app there, hidden, to
//   see what the agent made. It shows no comment UI (comment mode, composers
//   and pins stay closed), follows no events (the page's stream stands
//   down, so no version is followed either), and answers every consent
//   prompt "Don't allow" without showing it. `inspect({settleMs, maxBytes})`
//   asks the page's bridge for an inspection (`bridge/src/inspect.ts`) and
//   posts it as `inspection`, under the nonce like every message: the
//   bridge's answer is taken only from the frame, only for the latest
//   request, and only in the shape below (the page shares the bridge's
//   window, so it can answer in the bridge's place: it can describe only
//   itself). Anywhere else `inspect` answers an error and asks nothing.
//   `runTests({files, testMs, totalMs})` asks the bridge to run the draft's
//   test files (`bridge/src/harness.ts`) and posts the results as `tests`,
//   under the same rules.
// - Questions (spec 2026-10-12-flutter-app-round2-design R§10.2): the
//   owner's open agent questions about the artifact are the shell's own
//   question cards (`q/QuestionCard.svelte`), which a browser shows at the
//   top of the sidebar. App mode hides the sidebar, so the host docks them
//   in a sheet at the bottom (`.app-questions`, over the composer), closed
//   until `openQuestions`; the person closes it (its Close button, Escape),
//   the app does (`closeQuestions`, say on Android's back), and it closes
//   once no card is left. The question module (`q/index.ts`) mounts the
//   cards there and tells each change of the questions about the artifact
//   (`clax-questions`, the IDs open and closed). The host posts
//   `questions {open, showing}` whenever the number open or whether the
//   sheet shows changes, from `init` on (at `init`, the latest, if the
//   module has told any). The question module loads only in an owner's
//   browser, after the first paint: until it has told a change, no
//   `questions` message is posted. The inspect view has no sheet.
// - Device capabilities (`caps/device-link.ts`): outside the inspect view the
//   host links itself as the way to the app for `notify`, `share`, `file`,
//   `camera` and `location`. Each call the page makes is posted as `device
//   {id, cap, method, args}` (calls made before `init` wait for it), and the
//   app answers with `deviceResult({id, ok, value})` or `deviceResult({id,
//   ok: false, error: {code, message}})`. The inspect view links nothing, so
//   there `use()` resolves null for them.
// - `openRoute(hash)` moves the page to a fragment (`#route`), as a
//   notification's tap asks.
// Its styles go in with it: the shell links no stylesheet a lazy chunk
// imports (`vite.shell.config.ts`, `inlineCss`).
import appCss from "./app.css?inline";
import { getArtifact } from "../api";
import { acceptFromFrame } from "../bridge-link";
import { type DeviceCall, deviceLink } from "../caps/device-link";
import { CapError } from "../caps/errors";
import { hostArrival } from "../caps/gesture";
import { visibility } from "../visibility";
import { now } from "../clock";
import { SEND_FAILED, failureText, report, scopedNotice } from "../failure";
import { type Thread, sendToAgent, upsert } from "../threads";
import { INDEX_FILE, type InspectError, type PickDesc } from "../../../bridge/src/protocol";
import type { ArtifactController, ViewState } from "../view/artifact-controller";
import { anchorLabel } from "./label";
import { appQuery, inspectQuery, parseShellPath } from "../route";
import { setUrl } from "../view/url";

export type HostMessage =
  | { type: "ready"; artifact: string; version: number; latest: number; file: string }
  /** `draft.label`: what the composer's anchor is, in words (`label.ts`),
   * or the label the page gave a composer it opened; `byPage`: the page
   * opened this composer (its comments capability), not the viewer's pick. */
  | { type: "state"; commenting: boolean; draft: { label: string; text: string; clip: "pending" | "ready" | "none"; byPage?: true } | null; busy: boolean }
  /** `label`: what each thread's anchor is, in words (`label.ts`). */
  | { type: "threads"; threads: { id: string; status: string; summary: string; label: string; sent: boolean; comments: number; version: number }[] }
  | { type: "sent"; thread: string }
  | { type: "landed"; version: number }
  | { type: "notice"; text: string }
  /** A posted thread's send to the agent failed (`text` says why): it stays
   * unsent until `resend(thread)`. */
  | { type: "sendFailed"; thread: string; text: string }
  /** The inspect view's answer to `inspect`: `png`, the page's viewport (a
   * PNG, base64); `errors`, its newest errors since its first script;
   * `failed`, the URLs it failed to load; `text`, an outline of what it
   * shows; `title`; `height`, its scroll height in CSS pixels. Or `error`,
   * why there is none. */
  | { type: "inspection"; png: string; errors: InspectError[]; failed: string[]; text: string; title: string; height: number }
  | { type: "inspection"; error: string }
  /** The inspect view's answer to `runTests`: how many tests `passed`, each
   * that `failed` (or test file that did not load) with why, how many were
   * `skipped` for lack of time. Or `error`, why there was no run. */
  | { type: "tests"; passed: number; failed: { name: string; error: string }[]; skipped: number }
  | { type: "tests"; error: string }
  /** How many of the owner's questions about the artifact are `open`, and
   * whether the questions sheet is `showing`, posted whenever either
   * changes (see Questions above). */
  | { type: "questions"; open: number; showing: boolean }
  /** A device capability call of the page, for the app to make and answer
   * with `deviceResult` (see Device capabilities above). */
  | ({ type: "device" } & DeviceCall);
/** What goes on the channel: a `HostMessage` with this load's nonce. */
export type Posted = HostMessage & { nonce: string };

export type ClaxHost = {
  /** Starts the channel with this load's `nonce` (16 to 128 characters of
   * `A-Z a-z 0-9 _ -`); see the protocol above. Only the first valid call counts. */
  init(p: { nonce: string }): void;
  /** Whether the app shows this view (see the protocol above): visible from
   * `init` on unless told otherwise, before or after it. */
  setVisible(visible: boolean): void;
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
  /** The app's own thread view opened on thread `id`: its area stays
   * outlined while the view is open, until `focusThread(null)` (the view
   * closed) or comment mode ends. An ID of no listed thread clears it. */
  focusThread(id: string | null): void;
  /** Sends an open thread that is not yet sent to the agent (after
   * `sendFailed`), as Send does: `sent`, or `sendFailed` again. */
  resend(id: string): void;
  /** A real finger `pointer` pressed (`down`) or lifted (`up`) at (`x`,
   * `y`), CSS pixels in the WebView's viewport, as the app's listener saw it
   * reach the WebView. A press on the content frame itself counts as the
   * finger's arrival there until its release (at most `HELD_MAX_MS`;
   * `caps/gesture.ts`, "App mode's host attestation"); its release counts
   * for `ATTEST_MS`, and only a release whose press so counted: a press on a
   * shell control (which may be gone by the release) never does. A `cancel`
   * (the OS took the touch back) ends the pointer's press and counts as
   * nothing. */
  attestPointer(p: { pointer: number; phase: "down" | "up" | "cancel"; x: number; y: number }): void;
  /** Opens the composer on the whole app (an `artifact` anchor, no
   * screenshot), unless one holding typed text is open. Not in the inspect view. */
  composeArtifact(): void;
  /** In the inspect view, asks the page for an inspection once it has been
   * still for 300 ms, waiting `settleMs` at most (`INSPECT_SETTLE_MS` when
   * absent), its screenshot at most `maxBytes` (`INSPECT_MAX_BYTES`):
   * `inspection` answers. A later call replaces an earlier one. */
  inspect(p: { settleMs?: number; maxBytes?: number }): void;
  /** In the inspect view, asks the page to run the draft's test `files`
   * (`tests/<name>.test.js`, at most `TEST_FILES`) with the bridge's harness,
   * each test for at most `testMs`, the run for at most `totalMs`: `tests`
   * answers. A later call replaces an earlier one. */
  runTests(p: { files: string[]; testMs?: number; totalMs?: number }): void;
  /** Shows the questions sheet. With `question` (an ID), shows it with that
   * question's card in focus as soon as the shell knows it open: the app may
   * hear of a question before the shell's own feed does, so the request
   * waits, until that question shows, closes, or `closeQuestions` (or the
   * person) closes the sheet. Without, shows the cards there are, if any,
   * the first in focus. Focus already in a field of the sheet stays.
   * Nothing in the inspect view. */
  openQuestions(p?: { question?: string }): void;
  /** Closes the questions sheet, and drops a waiting `openQuestions`. */
  closeQuestions(): void;
  /** The app's answer to a `device` message: `{id, ok: true, value}` or
   * `{id, ok: false, error: {code, message}}`. */
  deviceResult(a: unknown): void;
  /** Moves the page to fragment `hash` (`#` and at most 512 characters). */
  openRoute(hash: string): void;
};

/** `inspect`'s defaults and bounds (`bridge/src/inspect.ts` has the same). */
export const INSPECT_SETTLE_MS = 3000;
const INSPECT_SETTLE_MAX_MS = 10_000;
export const INSPECT_MAX_BYTES = 1024 * 1024;
const INSPECT_MAX_BYTES_MAX = 16 * 1024 * 1024;
/** What an inspection carries at most, whatever the page sends. */
const INSPECT_ERRORS = 20;
const INSPECT_ERROR_CHARS = 501;
const INSPECT_TEXT_CHARS = 3072;
const INSPECT_TITLE_CHARS = 200;
const INSPECT_KINDS = new Set(["error", "unhandledrejection", "console"]);
/** `runTests`' bounds, and what its answer carries at most. */
export const TEST_FILES = 50;
const TEST_FILE = /^tests\/[A-Za-z0-9_\-./]{1,200}\.test\.js$/;
const TEST_MS_MAX = 30_000;
const TEST_TOTAL_MS_MAX = 120_000;
const TEST_FAILED = 50;
const TEST_NAME_CHARS = 201;
/** What the composer calls the whole artifact in the app, and says of its
 * screenshot (as `view/whole-comment.ts` does in a browser). */
const WHOLE_APP = "The whole app";
const WHOLE_NO_CLIP = "it is about the whole app";

/** How long a host's attestation of a finger's release on the frame counts,
 * and a press's with no pairing: a long-press's pick comes at most about
 * 400 ms after the release (`web/bridge/src/comment-mode.ts`). */
export const ATTEST_MS = 1000;
/** How long an attested press counts, from that press, while its release is
 * not attested: a hold, then a drag, takes seconds, and a release the host
 * never reports (the app paused mid-touch) must not leave the frame's
 * gesture standing; the press is then forgotten. */
export const HELD_MAX_MS = 30_000;
/** How near (CSS pixels, each way) a shell press is to an attested press for
 * both to be the same finger: the app's and the WebView's coordinates of a
 * touch can differ by rounding and the finger's contact point. */
export const NEAR_PX = 24;
/** How many of the shell's presses of the last `ATTEST_MS` are kept. */
const MAX_PRESSES = 16;
/** A thread's summary: its first comment, cut to this many characters. */
const SUMMARY_CHARS = 140;
const FOLLOW_FAILED = "Could not load the latest version";

type Window_ = Window & { claxHost?: ClaxHost; ClaxHost?: { postMessage(m: string): void } };

function summary(t: Thread): string {
  const text = (t.comments[0]?.body ?? "").replace(/\s+/g, " ").trim();
  return text.length > SUMMARY_CHARS ? `${text.slice(0, SUMMARY_CHARS)}…` : text;
}

const NONCE = /^[A-Za-z0-9_-]{16,128}$/;
const CLOSE_ICON = `<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg>`;
const QUESTION_ID_CHARS = 128;
const isId = (v: unknown): v is string => typeof v === "string" && v.length > 0 && v.length <= QUESTION_ID_CHARS;
const ids = (v: unknown): string[] | null => (Array.isArray(v) && v.every(isId) ? v : null);

const bounded = (v: unknown, lo: number, hi: number, dflt: number) => (typeof v === "number" && Number.isFinite(v) ? Math.min(hi, Math.max(lo, Math.round(v))) : dflt);
const str = (v: unknown, n: number) => (typeof v === "string" ? v.slice(0, n) : "");

/** The base64 of `buf`. */
function base64(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf);
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

/** The host message for the page's answer `m` to an inspection: only the
 * fields and sizes `inspection` names, whatever else the page sent. */
function inspected(m: Record<string, unknown>, maxBytes: number): HostMessage {
  if (typeof m.error === "string") return { type: "inspection", error: m.error.slice(0, INSPECT_ERROR_CHARS) || "the inspection failed" };
  const png = m.png;
  if (!(png instanceof ArrayBuffer)) return { type: "inspection", error: "the page sent no screenshot" };
  if (png.byteLength > maxBytes) return { type: "inspection", error: `the screenshot is over ${maxBytes} bytes` };
  const errors = (Array.isArray(m.errors) ? m.errors : [])
    .filter((e): e is InspectError => !!e && typeof e === "object" && INSPECT_KINDS.has(e.kind) && typeof e.text === "string")
    .slice(-INSPECT_ERRORS)
    .map(e => ({ kind: e.kind, text: e.text.slice(0, INSPECT_ERROR_CHARS) }));
  const failed = (Array.isArray(m.failed) ? m.failed : []).filter((u): u is string => typeof u === "string").slice(-INSPECT_ERRORS).map(u => u.slice(0, INSPECT_ERROR_CHARS));
  const height = typeof m.height === "number" && Number.isFinite(m.height) && m.height >= 0 ? m.height : 0;
  return { type: "inspection", png: base64(png), errors, failed, text: str(m.text, INSPECT_TEXT_CHARS), title: str(m.title, INSPECT_TITLE_CHARS), height };
}

const count = (v: unknown) => (typeof v === "number" && Number.isInteger(v) && v >= 0 ? v : null);

/** The host message for the page's answer `m` to a test run: only the fields
 * and sizes `tests` names, whatever else the page sent. */
export function testsMessage(m: Record<string, unknown>): HostMessage {
  if (typeof m.error === "string") return { type: "tests", error: m.error.slice(0, INSPECT_ERROR_CHARS) || "the test run failed" };
  const passed = count(m.passed);
  const skipped = count(m.skipped);
  if (passed === null || skipped === null || !Array.isArray(m.failed)) return { type: "tests", error: "the page sent no test results" };
  const failed = m.failed
    .filter((f): f is { name: string; error: string } => !!f && typeof f === "object" && typeof f.name === "string" && typeof f.error === "string")
    .slice(0, TEST_FAILED)
    .map(f => ({ name: f.name.slice(0, TEST_NAME_CHARS), error: f.error.slice(0, INSPECT_ERROR_CHARS) }));
  return { type: "tests", passed, failed, skipped };
}

/** Installs `window.claxHost` over `ctl`, and, from `init` on, posts every
 * `HostMessage` with the nonce to `window.ClaxHost` (the app's channel) when
 * it exists. Nothing, outside the top-level document at an app shell path.
 * Returns the uninstall. */
export function installHost(win: Window, ctl: ArtifactController): () => void {
  const w = win as Window_;
  if (win.top !== win || parseShellPath(win.location.pathname).kind !== "artifact" || !appQuery(win.location.search)) return () => {};
  const doc = win.document;
  const inspecting = inspectQuery(win.location.search);
  doc.documentElement.classList.add("app");
  if (inspecting) doc.documentElement.classList.add("inspect");
  const style = doc.createElement("style");
  style.textContent = appCss;
  doc.head.append(style);
  let nonce: string | null = null;
  const post = (m: HostMessage) => {
    const ch = w.ClaxHost;
    if (nonce !== null && ch && typeof ch.postMessage === "function") ch.postMessage(JSON.stringify({ ...m, nonce } satisfies Posted));
  };
  // The page's device calls waiting for the app's answer, by request ID,
  // and those made before `init`, which posts them.
  const devicePending = new Map<string, { resolve(v: unknown): void; reject(e: CapError): void }>();
  const deviceQueue: DeviceCall[] = [];
  // Request IDs carry a random prefix per load: an answer the app makes to
  // a call of an earlier load of this document resolves nothing here.
  const devicePrefix = Array.from(crypto.getRandomValues(new Uint8Array(6)), b => b.toString(16).padStart(2, "0")).join("");
  let deviceSeq = 0;
  const deviceCall = (cap: DeviceCall["cap"], method: string, args: unknown[]) => new Promise<unknown>((resolve, reject) => {
    const c: DeviceCall = { id: `${devicePrefix}-${++deviceSeq}`, cap, method, args };
    devicePending.set(c.id, { resolve, reject });
    if (nonce === null) deviceQueue.push(c);
    else post({ type: "device", ...c });
  });
  if (!inspecting) deviceLink.call = deviceCall;
  const s = () => ctl.state.get();

  // The page's last reported scroll position, per page.
  const scrolled = new Map<string, number>();
  // The latest pick start's element description (the bridge's), by pick ID.
  const picked = new Map<string, PickDesc>();
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
  const followIfDue = (v: ViewState) => { if (!inspecting && v.newer !== null && v.pinnedVersion === null && !v.draft) void follow(false); };
  /** In the inspect view: a consent prompt is answered "Don't allow", and a
   * composer or comment mode (the page's comments capability may open them)
   * closed. True when it changed something. */
  const closeForInspect = (v: ViewState): boolean => {
    if (v.ask) { v.ask.answer("deny"); return true; }
    if (v.draft) { ctl.cancelDraft(); return true; }
    if (v.commenting) { ctl.toggleComment(); return true; }
    return false;
  };

  let lastState = "";
  let lastThreads: Thread[] | null = null;
  let lastNotice: string | null = null;
  const postState = (v: ViewState) => {
    const d = v.draft;
    const m: HostMessage = {
      type: "state",
      commenting: v.commenting,
      draft: d ? { label: d.label ?? anchorLabel(d.anchor, picked.get(d.pickId)), text: ctl.composerText, clip: d.capturing ? "pending" : d.clip ? "ready" : "none", ...(d.byPage ? { byPage: true as const } : {}) } : null,
      busy: v.busy > 0,
    };
    const json = JSON.stringify(m);
    if (json === lastState) return;
    lastState = json;
    post(m);
  };
  // The shell's own composer shows the same words for a pick (its quote line
  // reads a draft's `label` first), so no selector is shown in the app.
  const labelled = new Set<string>();
  const labelDraft = (v: ViewState): boolean => {
    const d = v.draft;
    // A page's composer keeps a label the page gave it; one without gets these words too.
    if (!d || (d.label !== undefined && !labelled.has(d.pickId))) return false;
    const label = anchorLabel(d.anchor, picked.get(d.pickId));
    if (d.label === label) return false;
    labelled.add(d.pickId);
    ctl.set(c => (c.draft === d ? { draft: { ...d, label } } : {}));
    return true;
  };
  const onState = (v: ViewState) => {
    // The inspect view closes what would ask the viewer anything; each sets
    // the state again, which posts it.
    if (inspecting && closeForInspect(v)) return;
    // Labelling sets the state again, which posts it.
    if (labelDraft(v)) return;
    postState(v);
    if (v.threads !== lastThreads) {
      lastThreads = v.threads;
      post({ type: "threads", threads: v.threads.map(t => ({ id: t.id, status: t.status, summary: summary(t), label: anchorLabel(t.anchor), sent: t.sent_to_agent, comments: t.comments.length, version: t.version_n })) });
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
  // The page's scroll reports, for its current page, and what each pick's
  // element is (the controller has taken the start by now: the draft's
  // label is posted again with it).
  const onMessage = (e: MessageEvent) => {
    const m = acceptFromFrame(e, ctl.frame?.el?.contentWindow ?? null, s().origin ?? null);
    if (m?.type === "clax:inspection") {
      if (!inspection || m.requestId !== inspection.requestId) return;
      const { maxBytes } = inspection;
      inspection = null;
      post(inspected(m as unknown as Record<string, unknown>, maxBytes));
      return;
    }
    if (m?.type === "clax:tests") {
      if (testRun === null || m.requestId !== testRun) return;
      testRun = null;
      post(testsMessage(m as unknown as Record<string, unknown>));
      return;
    }
    if (m?.type === "clax:pick-start" && typeof m.pickId === "string" && m.desc) {
      picked.clear();
      picked.set(m.pickId, m.desc);
      if (!labelDraft(s())) postState(s());
    }
    if (m?.type !== "clax:scroll" || typeof m.file !== "string" || m.file !== s().file) return;
    if (typeof m.y === "number" && Number.isFinite(m.y) && m.y >= 0) scrolled.set(m.file, m.y);
  };
  // The inspection asked of the page, until it answers.
  let inspection: { requestId: string; maxBytes: number } | null = null;
  let inspections = 0;
  // The test run asked of the page, until it answers.
  let testRun: string | null = null;
  // The shell's own trusted presses and releases, where and when.
  let shellPresses: { x: number; y: number; at: number }[] = [];
  const onPress = (e: Event) => {
    const p = e as MouseEvent;
    if (!e.isTrusted) return;
    const t = now();
    shellPresses = [...shellPresses.filter(q => t - q.at < ATTEST_MS).slice(-(MAX_PRESSES - 1)), { x: p.clientX, y: p.clientY, at: t }];
  };
  win.addEventListener("input", onInput);
  win.addEventListener("message", onMessage);
  win.addEventListener("pointerdown", onPress, true);
  win.addEventListener("pointerup", onPress, true);
  // A page that greeted before this module loaded was welcomed without the
  // app's hooks (no `ready`, no `app: true`): it is asked to greet again.
  if (ctl.gate.open) ctl.send({ type: "clax:greet" });

  // When the latest counted attestation (a press or a paired release) ran.
  let attestedAt = -Infinity;
  // The pointers whose press was attested on the frame, with when, until
  // their release or cancel, or `HELD_MAX_MS` after that press.
  const pressed = new Map<number, number>();
  /** Drops presses held longer than `HELD_MAX_MS`: their release is lost. */
  const live = () => {
    const t = now();
    for (const [p, at] of pressed) if (t - at >= HELD_MAX_MS) pressed.delete(p);
    return pressed;
  };
  hostArrival.fresh = since => {
    for (const at of live().values()) if (at > since) return true;
    return attestedAt > since && now() - attestedAt < ATTEST_MS;
  };
  hostArrival.pending = () => live().size > 0;
  const composer = () => doc.querySelector<HTMLFormElement>(".stage form.composer");

  // The questions sheet (none in the inspect view), and what the question
  // module last told of the questions about the artifact: null until it has
  // told anything.
  const dock = inspecting ? null : doc.createElement("section");
  let told: { open: string[]; closed: string[] } | null = null;
  let lastQuestions = "";
  // The question `openQuestions` waits to show, if any.
  let awaited: string | null = null;
  // Focuses a card once it is mounted (the module tells a change before rendering it).
  let mounting: MutationObserver | null = null;
  const postQuestions = () => {
    if (nonce === null || told === null || !dock) return;
    const m: HostMessage = { type: "questions", open: told.open.length, showing: !dock.hidden };
    const json = JSON.stringify(m);
    if (json === lastQuestions) return;
    lastQuestions = json;
    post(m);
  };
  const cardOf = (id: string | null) => {
    for (const c of dock?.querySelectorAll<HTMLElement>(".qcard") ?? []) if (id === null || c.dataset.question === id) return c;
    return null;
  };
  /** Focuses question `id`'s card (the first card for null) now or once it
   * is mounted, unless focus is in a field of the sheet, the person's. */
  const focusCard = (id: string | null) => {
    mounting?.disconnect();
    mounting = null;
    if (!dock) return;
    const a = doc.activeElement;
    if (a && a !== dock && dock.contains(a) && !a.classList.contains("qcard")) return;
    const c = cardOf(id);
    if (c) { c.focus({ preventScroll: true }); return; }
    mounting = new MutationObserver(() => {
      const found = cardOf(id);
      if (!found && !dock.hidden) return;
      mounting?.disconnect();
      mounting = null;
      if (found && !dock.hidden) found.focus({ preventScroll: true });
    });
    mounting.observe(dock, { childList: true, subtree: true, attributes: true, attributeFilter: ["hidden"] });
  };
  const showQuestions = (id: string | null) => {
    if (!dock) return;
    awaited = null;
    dock.hidden = false;
    focusCard(id);
    postQuestions();
  };
  /** Hides the sheet. A waiting `openQuestions` stays: only the person or
   * the app closing the sheet (`dismissQuestions`), or the awaited question
   * closing, drops it. */
  const hideQuestions = () => {
    if (!dock || dock.hidden) return;
    mounting?.disconnect();
    mounting = null;
    const had = dock.contains(doc.activeElement);
    dock.hidden = true;
    if (had) (doc.activeElement as HTMLElement | null)?.blur?.();
    postQuestions();
  };
  const dismissQuestions = () => {
    awaited = null;
    hideQuestions();
  };
  const onQuestions = (e: Event) => {
    const d = (e as CustomEvent<{ open?: unknown; closed?: unknown }>).detail;
    const open = ids(d?.open);
    const closed = ids(d?.closed);
    if (!open || !closed) return;
    told = { open, closed };
    if (awaited !== null && closed.includes(awaited)) awaited = null;
    if (!open.length && !closed.length) hideQuestions();
    else if (awaited !== null && open.includes(awaited)) showQuestions(awaited);
    postQuestions();
  };
  if (dock) {
    dock.className = "app-questions";
    dock.hidden = true;
    dock.setAttribute("aria-label", "Questions for you");
    const close = doc.createElement("button");
    close.type = "button";
    close.className = "ghost icon app-questions-close";
    close.setAttribute("aria-label", "Close questions");
    close.innerHTML = CLOSE_ICON;
    close.addEventListener("click", dismissQuestions);
    const slot = doc.createElement("div");
    slot.className = "questions-slot";
    dock.append(close, slot);
    dock.addEventListener("keydown", e => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      dismissQuestions();
    });
    doc.body.append(dock);
    doc.addEventListener("clax-questions", onQuestions);
    // A question module already showing the cards in the hidden sidebar moves them here.
    slot.dispatchEvent(new CustomEvent("clax-questions-slot", { bubbles: true }));
  }
  // The app's word on visibility: null until `init` (the document's own applies).
  let hostHidden: boolean | null = null;
  // What `setVisible` said before `init`, which applies from `init` on.
  let wanted = true;
  const docHidden = visibility.hidden;
  const docShown = visibility.shown;
  visibility.hidden = d => hostHidden ?? docHidden(d);
  visibility.shown = () => (hostHidden === null ? null : !hostHidden);
  // The stream module, loaded on its own (the view loads it after its first
  // paint), so this chunk stays small: told of a change once it is in.
  let stream: typeof import("../stream") | null = null;
  let uninstalled = false;
  // The inspect view follows no events: the page's stream forgets its
  // watchers and takes no new ones, for this document's life.
  let quieted: { watch: unknown } | null = null;
  void import("../stream").then(m => {
    if (uninstalled) return;
    stream = m;
    if (inspecting) {
      const ps = m.pageStream();
      ps.close();
      ps.watch = () => () => {};
      quieted = ps;
    }
    if (hostHidden !== null) m.pageStream().visibilityChanged();
  }, () => {});
  const visibleNow = (hidden: boolean) => {
    if (hostHidden === hidden) return;
    hostHidden = hidden;
    stream?.pageStream().visibilityChanged();
    visibility.changed();
  };
  const host: ClaxHost = {
    setVisible: v => {
      if (typeof v !== "boolean") return;
      if (nonce === null) wanted = v;
      else visibleNow(!v);
    },
    init: p => {
      const n = p?.nonce;
      if (nonce !== null || typeof n !== "string" || !NONCE.test(n)) return;
      nonce = n;
      visibleNow(!wanted);
      lastState = "";
      lastThreads = null;
      lastNotice = null;
      onState(s());
      if (ready) post(ready);
      lastQuestions = "";
      postQuestions();
      for (const c of deviceQueue.splice(0)) post({ type: "device", ...c });
    },
    enterCommentMode: () => { if (!inspecting && !s().commenting) ctl.toggleComment(); },
    exitCommentMode: () => { if (s().commenting) ctl.toggleComment(); },
    loadVersion: n => {
      if (n === null) ctl.chooseVersion(ctl.latest());
      else if (Number.isInteger(n) && n > 0) ctl.chooseVersion(n);
    },
    reloadLatest: () => { void follow(true); },
    setDraftText: text => {
      // A text field of a question card in the shown sheet, when it has focus.
      const a = doc.activeElement;
      if (dock && !dock.hidden && dock.contains(a) && (a instanceof HTMLTextAreaElement || (a instanceof HTMLInputElement && a.type === "text"))) {
        a.value = String(text);
        a.dispatchEvent(new Event("input", { bubbles: true }));
        return;
      }
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
    focusThread: id => {
      ctl.focusThread(typeof id === "string" && s().threads.some(t => t.id === id) ? id : null);
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
        // Hit-tested when this runs, which may be after that control closed
        // on the tap's click: a press the shell itself saw near here within
        // `ATTEST_MS` was that finger, on the shell.
        const at = doc.elementFromPoint(x, y);
        const t = now();
        const onShell = shellPresses.some(q => t - q.at < ATTEST_MS && Math.abs(q.x - x) <= NEAR_PX && Math.abs(q.y - y) <= NEAR_PX);
        if (at && at === ctl.frame?.el && !onShell) { pressed.set(pointer, t); attestedAt = t; hostArrival.attested(); } else pressed.delete(pointer);
      } else if (phase === "up") {
        if (live().delete(pointer)) { attestedAt = now(); hostArrival.attested(); }
      } else if (phase === "cancel") {
        // The OS took the touch back (a system gesture, the app paused): the
        // press ends, and nothing is attested.
        pressed.delete(pointer);
      }
    },
    composeArtifact: () => {
      const v = s();
      if (inspecting || v.data === null || (v.draft && ctl.composerText.trim())) return;
      // On no page: the daemon stores the index's `file` as none.
      const anchor = { kind: "artifact" as const, selector: null, quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: INDEX_FILE };
      ctl.set({ draft: { pickId: `whole-${now()}`, anchor, version: ctl.shown(v), clip: null, clipError: WHOLE_NO_CLIP, label: WHOLE_APP } });
    },
    inspect: p => {
      if (nonce === null) return;
      if (!inspecting) { post({ type: "inspection", error: "this view is not the inspect view" }); return; }
      if (!ctl.gate.open) { post({ type: "inspection", error: "the page has not loaded" }); return; }
      const settleMs = bounded(p?.settleMs, 0, INSPECT_SETTLE_MAX_MS, INSPECT_SETTLE_MS);
      const maxBytes = bounded(p?.maxBytes, 1, INSPECT_MAX_BYTES_MAX, INSPECT_MAX_BYTES);
      inspection = { requestId: `inspect-${++inspections}`, maxBytes };
      ctl.send({ type: "clax:inspect", requestId: inspection.requestId, settleMs, maxBytes });
    },
    runTests: p => {
      if (nonce === null) return;
      if (!inspecting) { post({ type: "tests", error: "this view is not the inspect view" }); return; }
      const files = p?.files;
      if (!Array.isArray(files) || !files.length || files.length > TEST_FILES || !files.every(f => typeof f === "string" && TEST_FILE.test(f) && !f.split("/").includes(".."))) {
        post({ type: "tests", error: `files is a list of 1 to ${TEST_FILES} paths tests/<name>.test.js` });
        return;
      }
      if (!ctl.gate.open) { post({ type: "tests", error: "the page has not loaded" }); return; }
      testRun = `test-${++inspections}`;
      ctl.send({ type: "clax:test", requestId: testRun, files: [...files], testMs: bounded(p.testMs, 1, TEST_MS_MAX, 5000), totalMs: bounded(p.totalMs, 1, TEST_TOTAL_MS_MAX, 30_000) });
    },
    openQuestions: p => {
      if (!dock) return;
      const id = p?.question;
      if (id === undefined) {
        if (told && (told.open.length || told.closed.length)) showQuestions(null);
        return;
      }
      if (!isId(id) || told?.closed.includes(id)) return;
      if (told?.open.includes(id)) showQuestions(id);
      else awaited = id;
    },
    closeQuestions: () => { if (dock) dismissQuestions(); },
    deviceResult: a => {
      const { id, ok, value, error } = (a && typeof a === "object" ? a : {}) as { id?: unknown; ok?: unknown; value?: unknown; error?: unknown };
      const p = typeof id === "string" ? devicePending.get(id) : undefined;
      if (!p) return;
      devicePending.delete(id as string);
      if (ok === true) { p.resolve(value ?? null); return; }
      const e = (error && typeof error === "object" ? error : {}) as { code?: unknown; message?: unknown };
      p.reject(new CapError(typeof e.code === "string" ? e.code : "unavailable", typeof e.message === "string" ? e.message : "the device call failed"));
    },
    openRoute: hash => {
      if (typeof hash !== "string" || !hash.startsWith("#") || hash.length > 512) return;
      win.location.hash = hash;
    },
  };
  Object.defineProperty(w, "claxHost", { value: host, configurable: true });

  return () => {
    unsubscribe();
    if (deviceLink.call === deviceCall) deviceLink.call = null;
    for (const p of devicePending.values()) p.reject(new CapError("unavailable", "the app closed this view"));
    devicePending.clear();
    win.removeEventListener("input", onInput);
    win.removeEventListener("message", onMessage);
    win.removeEventListener("pointerdown", onPress, true);
    win.removeEventListener("pointerup", onPress, true);
    hostArrival.fresh = () => false;
    hostArrival.pending = () => false;
    uninstalled = true;
    if (quieted) delete quieted.watch;
    visibility.hidden = docHidden;
    visibility.shown = docShown;
    if (hostHidden !== null) visibility.changed();
    if (stream && hostHidden !== null) stream.pageStream().visibilityChanged();
    if (ctl.app) ctl.app = null;
    if (dock) {
      doc.removeEventListener("clax-questions", onQuestions);
      mounting?.disconnect();
      dock.remove();
    }
    delete w.claxHost;
    doc.documentElement.classList.remove("app", "inspect");
    style.remove();
  };
}
