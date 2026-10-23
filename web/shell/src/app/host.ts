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
// - Swap with a tap: a newer version published while the view shows the
//   latest is offered to the app (`newer {version}`, once per version, again
//   at `init`), never loaded by itself; the app's `reloadLatest` loads it in
//   place, on the page and fragment the frame shows, and the page is asked to
//   scroll back to where it last reported (`clax:scroll`), in the first
//   welcome only. A pinned view (an earlier version) is offered nothing.
// - The app's own history (`history {back}`, `back()`): the pages and
//   fragments the page moved through in this document (its links and its
//   `#route`s), and the routes it pushed with `history.pushState` (the
//   bridge's `clax:route`), newest last, at most `TRAIL_MAX`. `back()` moves
//   the frame to the one before in place, without a history entry, as the
//   shell's own back and forward do; to a route before, the bridge takes the
//   page back through its own entries (`clax:back`). A version swap keeps
//   the pages and fragments; a
//   page's routes go with its document. `history` is posted
//   whenever whether there is one to go back to changes, and at `init`.
// - Send: the composer's primary button posts the thread and sends it to the
//   agent (`sendTo`), in one press. A send that fails leaves the thread
//   posted and unsent: the host hears `sendFailed`, and `resend(id)` tries
//   again.
// - The app's composer is the send box: app mode hides the shell's own
//   (`app.css`), which stays in the document holding the draft. The app
//   shows what the comment is on from `state.draft` (`kind`, `label`,
//   `quote`) and the screenshot from `draftClip`, keeps the text, and sends
//   it with `submitDraft({text})`.
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
//   greeted. A second `init` in the same document is ignored, and so is one
//   naming another nonce than the document's `clax-nonce` meta, when it
//   carries one (an init the app ran for the WebView's previous load).
// - The app drops every message whose `nonce` is not the one it gave this
//   load (and every message before it called `init`).
// - The page cannot learn the nonce: the shell's document is cross-origin to
//   it (another origin in subdomain mode, an opaque one in sandbox mode), so
//   it can read neither the shell's variables nor `window.claxHost`, and the
//   shell never posts the nonce to the frame. The channel's messages go from
//   the shell's frame to the app, never through the page's.
// - The inspect view (`?app=1&inspect=1`): the app loads a draft of an app there, hidden, to
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
// - The migration view (`?app=1&inspect=1&migrate=1`) is the inspect view
//   whose frame is named `MIGRATE_FRAME`: the page's bridge ends the page's
//   markup before any of it runs. There only, `runMigration({file, from,
//   to, totalMs})` asks the bridge to run an app bundle's migration: import `file` in the page's
//   frame and call its default export with the page's `db` and `{from,
//   to}`; it posts `migration` `{ok: true}` or `{ok: false, error}`, under
//   the same rules as `tests`. With `journal: true` (the live run of an
//   update, spec §8.4) every `db` write the page makes, through any handle,
//   first sends the document's current value as `journal` `{seq, path,
//   before}` and waits for the app's `journalAck({seq, ok})`, which the app
//   makes once the entry is on disk; a refused or missing ack fails the
//   write. Once the run answers, every later write fails, until the view is
//   loaded again. `restoreJournal({entries})` (the migration view only) puts
//   such entries back, in reverse order, as this viewer (so the person's
//   own `data/users/<id>/` documents too), and posts `journalRestored`
//   `{ok: true}` or `{ok: false, error}`.
// - Questions: the
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
//   ok: false, error: {code, message}})`. A running `location.watch` gets
//   the app's news by `deviceEvent({cap: "location", watch, kind: "update",
//   value})` and, once, `deviceEvent({cap: "location", watch, kind: "end",
//   error: {code, message}})`. `home` takes the same way (`device {cap:
//   "home"}`); the app pushes its changes by `deviceEvent({cap: "home",
//   name, data})`. The inspect view links nothing, so
//   there `use()` resolves null for them. `init({nonce, platform})` names
//   the app's platform (`"ios"`, `"android"`, `"macos"`; anything else is
//   none): on iOS `camera`, `file`, `share` and `location` go through
//   WebKit in the shell, the app asked only for the grant
//   (`device {method: "grant"}`), and the app hiding the view
//   (`setVisible(false)`) ends their watches.
// - `openRoute(hash)` moves the page to a fragment (`#route`), as a
//   notification's tap asks.
// - Native overlays: `init`'s
//   `native` names the overlays the app draws itself, any of `ask` (the
//   consent prompt), `questions`, `notice` and `draft` (the drafts'
//   banner). The shell draws none of those, and posts them instead
//   (`native.ts`, loaded then): `ask`/`askCancel` answered by `askAnswer`,
//   `notice {text, tone, action?}` by `noticeAction`, `draft {state}` by
//   `showDraft`/`hideDraft`, and `gone` once the artifact is deleted. With
//   `questions` named the sheet is never built and `questions` carries the
//   open `ids` and `raise`. An overlay left out, and every one for an app
//   that sends no `native`, keeps its web version. `platform: "macos"`
//   gives controls the arrow cursor (`html.app.mac`, `app.css`).
// - The system bars: the page's bridge samples its background at the
//   viewport's top and bottom edges (`clax:chrome`, `bridge/src/chrome.ts`)
//   on its first paint and whenever either changes; the host posts them as
//   `chrome {top, bottom}` (`#rrggbb`, lowercase), and the latest again at
//   `init`. The app paints the status and navigation bars in them.
// - A slow page: while a page still parsing holds up what the view asked
//   of it, comment mode or a `use()` say (the bridge's `clax:wait`), the
//   host posts `pageWait {waiting: true}` once the wait has lasted
//   `PAGE_WAIT_MS`, and `pageWait {waiting: false}` when it ends or the
//   page goes; a wait under way at `init` is posted then. A shorter wait
//   is never posted.
// - The app's transport (`transport.ts`, its own script) carries the
//   shell's API calls and event stream over the same channel, in every shell
//   document the core marks with the `clax-embed` meta; the app answers it
//   through `window.claxTransport`, never `claxHost`.
// Its styles go in with it: the shell links no stylesheet a lazy chunk
// imports (`vite.shell.config.ts`, `inlineCss`).
import appCss from "./app.css?inline";
import { getArtifact } from "../api";
import { acceptByeFromFrame, acceptFromFrame } from "../bridge-link";
import { type DeviceCall, deviceLink, PLATFORMS } from "../caps/device-link";
import { CapError } from "../caps/errors";
import type { PromptKind } from "../caps/grants";
import { hostArrival } from "../caps/gesture";
import { visibility } from "../visibility";
import { now } from "../clock";
import { SEND_FAILED, failureText, report, scopedNotice } from "../failure";
import { type Thread, sendToAgent, upsert } from "../threads";
import { type Anchor, type Box, INDEX_FILE, type InspectError, type PickDesc } from "../../../bridge/src/protocol";
import type { ArtifactController, ViewState } from "../view/artifact-controller";
import { anchorLabel, anchorQuote } from "./label";
import { respaced } from "../view/composer-model";
import { appQuery, inspectQuery, migrateQuery, parseShellPath, shellPath } from "../route";
import { setUrl, validHash } from "../view/url";
import type { Inspector } from "./inspect";

export type HostMessage =
  | { type: "ready"; artifact: string; version: number; latest: number; file: string }
  /** `draft.key`: the composer's identity (a new one for each pick; a
   * page's composer moved to another area keeps it); `kind`: its anchor's
   * kind; `label`: what the composer's anchor is, in words (`label.ts`),
   * or the label the page gave a composer it opened; `quote`: the text it
   * quotes as a reader sees it, for the app's own quote marks
   * (`anchorQuote`), null when none; `byPage`: the page opened this
   * composer (its comments capability), not the viewer's pick; `at`: where
   * the viewer's pick is in the shell's viewport (CSS pixels), as the page's
   * resolution of the draft's anchor last placed it (`pickAt`). */
  | { type: "state"; commenting: boolean; draft: { key: string; kind: Anchor["kind"]; label: string; quote: string | null; text: string; clip: "pending" | "ready" | "none"; byPage?: true; at?: { x: number; y: number; w: number; h: number } } | null; busy: boolean }
  /** The open composer's screenshot (`png`, base64), once for each
   * screenshot it takes, while composer `key` is open; at `init`, the open
   * one's again. */
  | { type: "draftClip"; key: string; png: string }
  /** `label`: what each thread's anchor is, in words (`label.ts`); `clip`:
   * the thread has a screenshot (the daemon's `/clip`); `remote`: the remote
   * a linked app's thread came from (its `linked`), absent for one started
   * here. */
  | { type: "threads"; threads: { id: string; status: string; summary: string; label: string; sent: boolean; comments: number; version: number; clip: boolean; remote?: string }[] }
  | { type: "sent"; thread: string }
  | { type: "landed"; version: number }
  /** A newer version than the one shown is published (see Swap with a tap
   * above); `reloadLatest` loads it. */
  | { type: "newer"; version: number }
  /** Whether `back()` has a page or fragment of the app to go back to (see
   * The app's own history above). */
  | { type: "history"; back: boolean }
  /** A shell notice. With `notice` native (see Native overlays above), its
   * `tone` and the `action` `noticeAction` runs. */
  | { type: "notice"; text: string; tone?: "info" | "error"; action?: { id: string; label: string } }
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
  /** The inspect view's answer to `runMigration`: the migration finished,
   * or `error`, why it did not (it threw, did not load, ran out of time, or
   * there was no run). */
  | { type: "migration"; ok: true }
  | { type: "migration"; ok: false; error: string }
  /** A journaled migration's write is waiting: `before` is the data of the
   * document at `path` (null when there is none). `journalAck` answers. */
  | { type: "journal"; seq: number; path: string; before: Record<string, unknown> | null }
  /** The answer to `restoreJournal`. */
  | { type: "journalRestored"; ok: true }
  | { type: "journalRestored"; ok: false; error: string }
  /** How many of the owner's questions about the artifact are `open`, and
   * whether the questions sheet is `showing`, posted whenever either
   * changes (see Questions above). With `questions` native, also the open
   * questions' `ids`, and `raise`: one of them was not open before in
   * this view (the first telling raises none) and the app shows the view. */
  | { type: "questions"; open: number; showing: boolean; ids?: string[]; raise?: boolean }
  /** The prompt at the head of the queue, with `ask` native (`native.ts`). */
  | { type: "ask"; id: string; kind: PromptKind; title: string; body: string; allow: string; deny: string; always?: string; armMs: number }
  /** Prompt `id` ended without the app's answer (`native.ts`). */
  | { type: "askCancel"; id: string }
  /** What the drafts' banner would say, with `draft` native (`native.ts`). */
  | { type: "draft"; state: DraftState | null }
  /** The artifact was deleted, once (`native.ts`). */
  | { type: "gone" }
  /** The page's background at the viewport's top and bottom edges
   * (`#rrggbb`), for the system bars (see The system bars above). */
  | { type: "chrome"; top: string; bottom: string }
  /** The page is slow to take comment mode or a `use()` (see A slow page
   * above). */
  | { type: "pageWait"; waiting: boolean }
  /** A device capability call of the page, for the app to make and answer
   * with `deviceResult` (see Device capabilities above). */
  | ({ type: "device" } & DeviceCall);
/** The drafts' banner, for the app: an agent (`by`, by its name) drafting,
 * or its draft shown over version `base`, `detail` saying what changed. */
export type DraftState = { kind: "drafting"; by: string; note?: string } | { kind: "showing"; by: string; base: number; detail: string };
/** How long a page's wait for its parse lasts before the app hears of it. */
export const PAGE_WAIT_MS = 400;
/** The overlays the app may draw itself (`init`'s `native`). */
const NATIVE = ["ask", "questions", "notice", "draft"] as const;
export type NativeOverlay = (typeof NATIVE)[number];
/** What goes on the channel: a `HostMessage` with this load's nonce. */
export type Posted = HostMessage & { nonce: string };

export type ClaxHost = {
  /** Starts the channel with this load's `nonce` (16 to 128 characters of
   * `A-Z a-z 0-9 _ -`); see the protocol above. Only the first valid call
   * counts. `native`: the overlays the app draws itself (see Native
   * overlays above). */
  init(p: { nonce: string; platform?: string; native?: string[] }): void;
  /** Answers prompt `id` (an `ask`): `allow`, `deny`, `always` (an Allow)
   * or `dismiss`. Any other ID or answer changes nothing. */
  askAnswer(id: string, answer: "allow" | "deny" | "always" | "dismiss"): void;
  /** Runs the action of a `notice` that carried one. */
  noticeAction(id: string): void;
  /** The drafts' banner's Show draft: the draft of the agent named `by`,
   * else the first. */
  showDraft(by?: string): void;
  /** The drafts' banner's Back. */
  hideDraft(): void;
  /** Whether the app shows this view (see the protocol above): visible from
   * `init` on unless told otherwise, before or after it. */
  setVisible(visible: boolean): void;
  enterCommentMode(): void;
  exitCommentMode(): void;
  /** Shows version `n`, or the latest for null (a load of the shell). */
  loadVersion(n: number | null): void;
  /** Loads the latest version in place, keeping page, fragment and scroll. */
  reloadLatest(): void;
  /** Moves the page back to the page or fragment it showed before, in place;
   * nothing when there is none (`history {back: false}`). */
  back(): void;
  /** Replaces the open composer's text; nothing without one. A focused text
   * field of the questions sheet takes it instead, unless `composer` (the
   * app's composer, whose text the composer's always is). */
  setDraftText(text: string, p?: { composer?: boolean }): void;
  /** Presses Send on the open composer (waiting for its screenshot as Send
   * does), with `text` as its comment when given (the app's composer). */
  submitDraft(p?: { text?: string }): void;
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
  /** In the migration view, asks the page to run the migration `file` (a
   * `.js` path in the app, `migrate.js` for a bundle) with the page's `db`
   * and `{from, to}` (versions, at least 1), the run for at most `totalMs`
   * (`MIGRATE_MS` when absent): `migration` answers. A later call replaces
   * an earlier one. */
  runMigration(p: { file: string; from: number; to: number; totalMs?: number; journal?: boolean }): void;
  /** The app has journal entry `seq` on disk (`ok`), or could not keep it:
   * the write waiting on it goes ahead, or fails. */
  journalAck(p: { seq: number; ok: boolean }): void;
  /** In the migration view, puts back a journaled migration's `entries`
   * (`{path, before}`, in the order they were journaled) in reverse order:
   * `journalRestored` answers. */
  restoreJournal(p: { entries: { path: string; before: Record<string, unknown> | null }[] }): void;
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
  /** The app's news for a running `location.watch`, or a change for
   * `home` (see Device capabilities above). */
  deviceEvent(a: unknown): void;
  /** Moves the page to fragment `hash` (`#` and at most 512 characters). */
  openRoute(hash: string): void;
};

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
/** The notice kind for a post the composer queued behind its screenshot
 * and then cancelled. */
const POST_DROPPED = "Not sent";
/** How many pages and fragments the app's own history keeps. */
export const TRAIL_MAX = 50;

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

/** `v` when it is a rectangle {x, y, w, h} of finite numbers with no
 * negative size, else undefined: the page's frame placed it. */
function boxOf(v: unknown): Box | undefined {
  const { x, y, w, h } = (v ?? {}) as Record<string, unknown>;
  return [x, y, w, h].every(Number.isFinite) && (w as number) >= 0 && (h as number) >= 0 ? { x, y, w, h } as Box : undefined;
}
/** Whether draft `d` is the viewer's pick on the page, which the app places
 * its composer at: not one the page opened, nor one on the whole app. */
const picksAt = (d: { byPage?: boolean; anchor: Anchor }) => !d.byPage && d.anchor.kind !== "artifact";



/** Installs `window.claxHost` over `ctl`, and, from `init` on, posts every
 * `HostMessage` with the nonce to `window.ClaxHost` (the app's channel) when
 * it exists. Nothing, outside the top-level document at an app shell path.
 * Returns the uninstall. */
export function installHost(win: Window, ctl: ArtifactController): () => void {
  const w = win as Window_;
  if (win.top !== win || parseShellPath(win.location.pathname).kind !== "artifact" || !appQuery(win.location.search)) return () => {};
  const doc = win.document;
  const inspecting = inspectQuery(win.location.search);
  const migrating = migrateQuery(win.location.search);
  doc.documentElement.classList.add("app");
  if (inspecting) doc.documentElement.classList.add("inspect");
  const style = doc.createElement("style");
  style.textContent = appCss;
  doc.head.append(style);
  let nonce: string | null = null;
  // The load's nonce, when the document came with it (the app's scheme
  // handler writes it).
  const meta = doc.querySelector('meta[name="clax-nonce"]')?.getAttribute("content") ?? null;
  const given = meta !== null && NONCE.test(meta) ? meta : null;
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
  let homeAsked: Promise<boolean> | undefined;
  if (!inspecting) {
    deviceLink.call = deviceCall;
    deviceLink.home = () => homeAsked ??= deviceCall("home", "available", []).then(v => v === true, () => false);
  }
  const s = () => ctl.state.get();

  // The page's edge colours as last reported, posted again at `init`.
  let chrome: HostMessage | null = null;
  // The page's wait for its parse: the timer until it is told, and whether
  // it was (and is told again at `init`).
  let waitTimer: ReturnType<typeof setTimeout> | undefined;
  let toldWait = false;
  const pageWaiting = (on: boolean) => {
    if (on) {
      if (!toldWait) waitTimer ??= setTimeout(() => post({ type: "pageWait", waiting: toldWait = true }), PAGE_WAIT_MS);
      return;
    }
    clearTimeout(waitTimer);
    waitTimer = undefined;
    if (toldWait) post({ type: "pageWait", waiting: toldWait = false });
  };
  // The page's last reported scroll position, per page.
  const scrolled = new Map<string, number>();
  // The app's own history: the pages, fragments and pushState routes the
  // page showed in this document, newest last, and whether `history` last
  // said there is one to go back to. `route` is the depth of the page's
  // route (its pushes above the entry it loaded at, 0 for none) and `depth`
  // the one the frame's document shows.
  let trail: { file: string; hash: string; route: number }[] = [];
  let depth = 0;
  let toldBack: boolean | null = null;
  const postHistory = () => {
    const back = trail.length > 1;
    if (nonce === null || back === toldBack) return;
    toldBack = back;
    post({ type: "history", back });
  };
  /** The page shows `file` at fragment `hash`: a new entry unless it is the
   * newest one (a version swap, or the move `back()` made). */
  const visit = (file: string, hash: string) => {
    if (inspecting) return;
    const top = trail.at(-1);
    if (top && top.file === file && top.hash === hash && top.route === depth) return;
    trail = [...trail, { file, hash, route: depth }].slice(-TRAIL_MAX);
    postHistory();
  };
  /** A document greeted: the routes of the one before cannot be returned
   * to, so they leave the trail. */
  const newDocument = () => {
    depth = 0;
    trail = trail.filter(e => e.route === 0);
  };
  /** The page moved to its route at `to` (a push, or its own back or
   * forward): the routes above it leave the trail, and a new one is an
   * entry. */
  const route = (file: string, to: number, hash: string) => {
    if (inspecting) return;
    depth = to;
    while (trail.length > 1 && trail.at(-1)!.route > to) trail = trail.slice(0, -1);
    postHistory();
    visit(file, hash);
  };
  /** Moves the frame to the entry before the newest, in place: the shell URL
   * is replaced and the controller's own back (`popstate`) moves the frame,
   * to another page without a history entry, or to the fragment, which the
   * page takes in place. */
  const goBack = () => {
    if (trail.length < 2) return;
    const from = trail.at(-1)!;
    trail = trail.slice(0, -1);
    const to = trail.at(-1)!;
    postHistory();
    if (from.route > to.route) {
      depth = to.route;
      if (ctl.gate.open) ctl.send({ type: "clax:back", depth: to.route });
    }
    setUrl(shellPath(ctl.id, s().pinnedVersion, to.file, ctl.shown()) + appQuery(location.search) + to.hash);
    win.dispatchEvent(new PopStateEvent("popstate"));
  };
  // The latest pick start's element description (the bridge's), by pick ID.
  const picked = new Map<string, PickDesc>();
  // The spaced text of the elements picked in this load, by their quote: a
  // thread on one is labelled with it, not its run-together quote.
  const shown = new Map<string, string>();
  const shownFor = (a: Thread["anchor"]) => (a.quote ? { text: shown.get(a.quote) } : null);
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
    // once the version auto-follow loaded greets. A document that greets
    // `again` (asked after a load that brought no new document, as Chromium
    // fires for a traversal of the page's own entries) keeps its routes.
    welcome: (file, again) => {
      if (!again) newDocument();
      visit(file, ctl.frameHash);
      // Routes a new document dropped can leave nothing to go back to.
      postHistory();
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
    // The viewer's open pick, placed by the page like the threads' anchors.
    anchors: () => {
      const d = s().draft;
      // As for threads: an area's fingerprint is skipped only on the version
      // it was picked on, not after a follow to a newer one.
      return d && picksAt(d) && d.anchor.file === s().file ? [{ id: d.pickId, anchor: d.anchor, sameVersion: d.version === ctl.shown() }] : [];
    },
    // The app's Send waits on the post the composer dropped: a fresh notice
    // each time tells it nothing was sent (on the web the composer's own
    // status line says so).
    dropped: () => {
      const notice = scopedNotice(ctl.setNotice, POST_DROPPED);
      notice(null);
      notice(`${POST_DROPPED}: the comment changed before its screenshot was taken. Send it again.`);
    },
  };

  // Loads the latest version in place (the app's `reloadLatest`).
  let following = false;
  const follow = async () => {
    if (following) return;
    following = true;
    let loaded = false;
    try {
      const d = await report(getArtifact(ctl.id), FOLLOW_FAILED, scopedNotice(ctl.setNotice, FOLLOW_FAILED));
      const v = s();
      if (!d || ctl.disposed || ctl.app === null) return;
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
    // A version published while this one loaded is offered in turn.
    if (loaded) offerIfDue(s());
  };
  // The newest version offered to the app (`newer`), so each is offered once.
  let offered = 0;
  const offerIfDue = (v: ViewState) => {
    if (inspecting || nonce === null || v.newer === null || v.pinnedVersion !== null || v.newer <= offered || v.newer <= ctl.shown(v)) return;
    offered = v.newer;
    post({ type: "newer", version: v.newer });
  };
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
  /** Where the viewer's pick `d` is in the shell's viewport: the place the
   * page last resolved its anchor to (`anchors` below), offset by the
   * frame's; none for a composer the page opened or one on the whole app,
   * or while the page has not placed it. */
  const pickAt = (v: ViewState, d: NonNullable<ViewState["draft"]>) => {
    const r = boxOf(v.resolved[d.pickId]?.rect);
    const f = ctl.frame?.el?.getBoundingClientRect();
    return r && f && picksAt(d) ? { at: { x: r.x + f.left, y: r.y + f.top, w: r.w, h: r.h } } : {};
  };
  const postState = (v: ViewState) => {
    const d = v.draft;
    const m: HostMessage = {
      type: "state",
      commenting: v.commenting,
      draft: d ? {
        key: d.pickId, kind: d.anchor.kind, label: d.label ?? anchorLabel(d.anchor, picked.get(d.pickId)), quote: anchorQuote(d.anchor, picked.get(d.pickId)),
        text: ctl.composerText, clip: d.capturing ? "pending" : d.clip ? "ready" : "none", ...(d.byPage ? { byPage: true as const } : {}), ...pickAt(v, d),
      } : null,
      busy: v.busy > 0,
    };
    const json = JSON.stringify(m);
    if (json !== lastState) {
      lastState = json;
      post(m);
    }
    postClip(d);
  };
  // The screenshot last posted (or being read to post).
  let lastClip: Blob | null = null;
  const postClip = (d: ViewState["draft"]) => {
    const clip = d?.clip ?? null;
    if (!d || !clip || clip === lastClip || nonce === null) return;
    lastClip = clip;
    const reader = new FileReader();
    reader.onload = () => {
      const open = s().draft;
      const url = typeof reader.result === "string" ? reader.result : "";
      const png = url.slice(url.indexOf(",") + 1);
      if (open?.clip === clip && png) post({ type: "draftClip", key: open.pickId, png });
    };
    reader.readAsDataURL(clip);
  };
  // The shell's own composer shows the same words for a pick (its quote line
  // reads a draft's `label` first), so no selector is shown in the app.
  const labelled = new Set<string>();
  const labelDraft = (v: ViewState): boolean => {
    const d = v.draft;
    const text = d?.anchor.quote && respaced(d.anchor.quote, picked.get(d.pickId)?.text);
    if (d?.anchor.quote && text) shown.set(d.anchor.quote, text);
    // A page's composer keeps a label the page gave it; one without gets these words too.
    if (!d || (d.label !== undefined && !labelled.has(d.pickId))) return false;
    const label = anchorLabel(d.anchor, picked.get(d.pickId));
    if (d.label === label) return false;
    labelled.add(d.pickId);
    ctl.set(c => (c.draft === d ? { draft: { ...d, label } } : {}));
    return true;
  };
  // The pick whose place the page was last asked for.
  let placing: string | null = null;
  const onState = (v: ViewState) => {
    // The inspect view closes what would ask the viewer anything; each sets
    // the state again, which posts it.
    if (inspecting && closeForInspect(v)) return;
    // A new pick: the page places it with the threads (`anchors` above).
    const d = v.draft;
    if (d && picksAt(d) && d.pickId !== placing) {
      placing = d.pickId;
      ctl.resolveAll();
    }
    // Labelling sets the state again, which posts it.
    if (labelDraft(v)) return;
    postState(v);
    if (v.threads !== lastThreads) {
      lastThreads = v.threads;
      post({ type: "threads", threads: v.threads.map(t => ({ id: t.id, status: t.status, summary: summary(t), label: anchorLabel(t.anchor, shownFor(t.anchor)), sent: t.sent_to_agent, comments: t.comments.length, version: t.version_n, clip: t.has_clip, ...(t.linked?.origin === "remote" ? { remote: t.linked.remote } : {}) })) });
    }
    if (v.notice !== lastNotice) {
      lastNotice = v.notice;
      // A native notice is `native.ts`'s.
      if (v.notice && !v.native?.includes("notice")) post({ type: "notice", text: v.notice });
    }
    offerIfDue(v);
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
    const frameWin = ctl.frame?.el?.contentWindow ?? null;
    if (acceptByeFromFrame(e, frameWin, s().origin ?? null)) return pageWaiting(false);
    const m = acceptFromFrame(e, frameWin, s().origin ?? null);
    if (m?.type === "clax:wait") {
      if (typeof m.waiting === "boolean") pageWaiting(m.waiting);
      return;
    }
    if (m?.type === "clax:inspection" || m?.type === "clax:tests" || m?.type === "clax:migration") {
      inspector?.answered(m as unknown as Record<string, unknown> & { type: string });
      return;
    }
    if ((m?.type === "clax:hash" || m?.type === "clax:route") && typeof m.hash === "string" && validHash(m.hash)) {
      const file = s().file;
      if (file === null) return;
      if (m.type === "clax:hash") visit(file, m.hash);
      else if (Number.isInteger(m.depth) && m.depth >= 0) route(file, m.depth, m.hash);
      return;
    }
    if (m?.type === "clax:chrome") {
      const hex = (v: unknown) => typeof v === "string" && /^#[0-9a-f]{6}$/i.test(v) ? v.toLowerCase() : null;
      const top = hex(m.top), bottom = hex(m.bottom);
      if (top && bottom) post(chrome = { type: "chrome", top, bottom });
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
  // The inspect view's handlers (`inspect.ts`), a chunk of their own: loaded
  // at install in the inspect view, else on the first call to one. Calls
  // made while it loads share the one load; a failed load is answered by
  // each call's `refused` message, naming the cause, and the next call
  // loads it again.
  let inspector: Inspector | null = null;
  let loading: Promise<Inspector> | null = null;
  const loadInspector = () => (loading ??= import("./inspect").then(
    m => (inspector ??= m.inspector({ ctl, post, inspecting, migrating, given: () => nonce !== null })),
    (e: unknown) => { loading = null; throw e; },
  ));
  const inspectorDo = (f: (i: Inspector) => void, refused?: (error: string) => HostMessage) => {
    if (inspector) return f(inspector);
    void loadInspector().then(f, (e: unknown) => {
      if (refused && nonce !== null) post(refused(`the inspect view's code did not load: ${e instanceof Error ? e.message : String(e)}`.slice(0, 500)));
    });
  };
  if (inspecting) void loadInspector().catch(() => {});
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

  // The questions sheet, built at `init` unless the app draws the questions
  // (none in the inspect view), and what the question module last told of
  // the questions about the artifact: null until it has told anything.
  let dock: HTMLElement | null = null;
  let told: { open: string[]; closed: string[] } | null = null;
  // With `questions` native: the IDs told open so far (null until the
  // first telling), and whether the latest telling raises a new one.
  let nativeQ = false;
  let known: Set<string> | null = null;
  let raise = false;
  let lastQuestions = "";
  // The question `openQuestions` waits to show, if any.
  let awaited: string | null = null;
  // Focuses a card once it is mounted (the module tells a change before rendering it).
  let mounting: MutationObserver | null = null;
  const postQuestions = () => {
    if (nonce === null || told === null || !(dock || nativeQ)) return;
    const m: HostMessage = dock ? { type: "questions", open: told.open.length, showing: !dock.hidden } : { type: "questions", open: told.open.length, showing: false, ids: told.open, raise };
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
    const sheet = dock;
    if (!sheet) return;
    const a = doc.activeElement;
    if (a && a !== sheet && sheet.contains(a) && !a.classList.contains("qcard")) return;
    const c = cardOf(id);
    if (c) { c.focus({ preventScroll: true }); return; }
    mounting = new MutationObserver(() => {
      const found = cardOf(id);
      if (!found && !sheet.hidden) return;
      mounting?.disconnect();
      mounting = null;
      if (found && !sheet.hidden) found.focus({ preventScroll: true });
    });
    mounting.observe(sheet, { childList: true, subtree: true, attributes: true, attributeFilter: ["hidden"] });
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
    raise = !!known && hostHidden === false && open.some(id => !known!.has(id));
    known = new Set([...(known ?? []), ...open]);
    if (awaited !== null && closed.includes(awaited)) awaited = null;
    if (!open.length && !closed.length) hideQuestions();
    else if (awaited !== null && open.includes(awaited)) showQuestions(awaited);
    postQuestions();
  };
  if (!inspecting) doc.addEventListener("clax-questions", onQuestions);
  const buildDock = () => {
    const el = doc.createElement("section");
    el.className = "app-questions";
    el.hidden = true;
    el.setAttribute("aria-label", "Questions for you");
    const close = doc.createElement("button");
    close.type = "button";
    close.className = "ghost icon app-questions-close";
    close.setAttribute("aria-label", "Close questions");
    close.innerHTML = CLOSE_ICON;
    close.addEventListener("click", dismissQuestions);
    const slot = doc.createElement("div");
    slot.className = "questions-slot";
    el.append(close, slot);
    el.addEventListener("keydown", e => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      dismissQuestions();
    });
    doc.body.append(el);
    // A question module already showing the cards in the hidden sidebar moves them here.
    slot.dispatchEvent(new CustomEvent("clax-questions-slot", { bubbles: true }));
    return el;
  };
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
  // The native overlays (`native.ts`), once loaded.
  let overlays: import("./native").NativeOverlays | null = null;
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
    if (hidden) for (const f of [...deviceLink.hidden]) f();
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
      // A document that came with its nonce takes no other: an init the app
      // ran for the WebView's previous load can reach this one first.
      if (given !== null && n !== given) return;
      nonce = n;
      const platform = PLATFORMS.find(x => x === p.platform);
      if (!inspecting && platform) deviceLink.platform = platform;
      if (platform === "macos") doc.documentElement.classList.add("mac");
      const native = Array.isArray(p.native) ? NATIVE.filter(k => p.native!.includes(k)) : null;
      if (native) ctl.set({ native });
      nativeQ = !!native?.includes("questions");
      if (!inspecting && !nativeQ) dock = buildDock();
      if (!inspecting && native) {
        void import("./native").then(m => {
          if (!uninstalled) overlays = m.nativeOverlays({ win, ctl, post, native, retry: () => void follow(), followFailed: FOLLOW_FAILED });
        }, () => {});
      }
      visibleNow(!wanted);
      lastState = "";
      lastClip = null;
      lastThreads = null;
      lastNotice = null;
      onState(s());
      if (ready) post(ready);
      lastQuestions = "";
      postQuestions();
      postHistory();
      if (chrome) post(chrome);
      if (toldWait) post({ type: "pageWait", waiting: true });
      for (const c of deviceQueue.splice(0)) post({ type: "device", ...c });
    },
    enterCommentMode: () => { if (!inspecting && !s().commenting) ctl.toggleComment(); },
    exitCommentMode: () => { if (s().commenting) ctl.toggleComment(); },
    loadVersion: n => {
      if (n === null) ctl.chooseVersion(ctl.latest());
      else if (Number.isInteger(n) && n > 0) ctl.chooseVersion(n);
    },
    reloadLatest: () => { void follow(); },
    back: () => goBack(),
    setDraftText: (text, p) => {
      // A text field of a question card in the shown sheet, when it has
      // focus, unless the app's composer is typing.
      const a = doc.activeElement;
      if (p?.composer !== true && dock && !dock.hidden && dock.contains(a) && (a instanceof HTMLTextAreaElement || (a instanceof HTMLInputElement && a.type === "text"))) {
        a.value = String(text);
        a.dispatchEvent(new Event("input", { bubbles: true }));
        return;
      }
      const ta = composer()?.querySelector("textarea");
      if (!ta || !s().draft) return;
      ta.value = String(text);
      ta.dispatchEvent(new Event("input", { bubbles: true }));
    },
    submitDraft: p => {
      const form = composer();
      if (!s().draft || !form) return;
      const text = p?.text;
      const ta = form.querySelector("textarea");
      if (typeof text === "string" && ta) {
        ta.value = text;
        ta.dispatchEvent(new Event("input", { bubbles: true }));
      }
      form.requestSubmit();
    },
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
        if (at && at === ctl.frame?.el && !onShell) { pressed.set(pointer, t); attestedAt = t; hostArrival.presses++; hostArrival.attested(); } else pressed.delete(pointer);
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
    inspect: p => inspectorDo(i => i.inspect(p), error => ({ type: "inspection", error })),
    runTests: p => inspectorDo(i => i.runTests(p), error => ({ type: "tests", error })),
    runMigration: p => inspectorDo(i => i.runMigration(p), error => ({ type: "migration", ok: false, error })),
    // Nothing waits on an ack's answer, and no journal is open before the
    // chunk has loaded.
    journalAck: p => inspectorDo(i => i.journalAck(p)),
    restoreJournal: p => inspectorDo(i => i.restoreJournal(p), error => ({ type: "journalRestored", ok: false, error })),
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
    askAnswer: (id, a) => overlays?.askAnswer(id, a),
    noticeAction: id => overlays?.noticeAction(id),
    showDraft: by => overlays?.showDraft(by),
    hideDraft: () => overlays?.hideDraft(),
    deviceResult: a => {
      const { id, ok, value, error } = (a && typeof a === "object" ? a : {}) as { id?: unknown; ok?: unknown; value?: unknown; error?: unknown };
      const p = typeof id === "string" ? devicePending.get(id) : undefined;
      if (!p) return;
      devicePending.delete(id as string);
      if (ok === true) { p.resolve(value ?? null); return; }
      const e = (error && typeof error === "object" ? error : {}) as { code?: unknown; message?: unknown };
      p.reject(new CapError(typeof e.code === "string" ? e.code : "unavailable", typeof e.message === "string" ? e.message : "the device call failed"));
    },
    // The device listeners (caps/device.ts) check the news's shape.
    deviceEvent: a => { for (const f of [...deviceLink.events]) f(a); },
    openRoute: hash => {
      if (typeof hash !== "string" || !hash.startsWith("#") || hash.length > 512) return;
      win.location.hash = hash;
    },
  };
  Object.defineProperty(w, "claxHost", { value: host, configurable: true });

  return () => {
    unsubscribe();
    clearTimeout(waitTimer);
    if (deviceLink.call === deviceCall) {
      deviceLink.call = null;
      deviceLink.home = undefined;
      deviceLink.platform = null;
    }
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
    doc.removeEventListener("clax-questions", onQuestions);
    if (dock) {
      mounting?.disconnect();
      dock.remove();
    }
    overlays?.dispose();
    delete w.claxHost;
    doc.documentElement.classList.remove("app", "inspect", "mac");
    style.remove();
  };
}
