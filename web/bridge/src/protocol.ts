// Messages between the shell and the bridge in the content frame. The shell
// imports these types from here.

/** `artifact`: the whole artifact, no spot on any page (its `file` is ""); the
 * shell never sends one to the bridge. */
export type AnchorKind = "element" | "range" | "custom" | "area" | "artifact";
/** The page an anchor names when it names none, and the index's `file`. */
export const INDEX_FILE = "index.html";
export interface AnchorRect { x: number; y: number; w: number; h: number; scrollX: number; scrollY: number; viewportW: number }
/** A drawn rectangle as fractions (0 to 1, 6 decimal places) of its
 * element's border box: `x` and `y` from its top left corner, `w` and `h` of
 * its width and height; `tag`, `text`, and `children` fingerprint the element
 * at draw time (see `anchor.ts` `fingerprint`). */
export interface AnchorArea { x: number; y: number; w: number; h: number; tag?: string; text?: string; children?: number }
/** Spec §9 "Anchors"; field names match the daemon's JSON. An `area` anchor
 * (a rectangle the viewer drew) names the smallest element containing the
 * rectangle in `selector`, places it in `area`, and holds it in viewport
 * pixels at draw time in `rect`; other kinds carry no `area`. */
export interface Anchor {
  kind: AnchorKind;
  selector: string | null;
  quote: string | null;
  prefix: string | null;
  suffix: string | null;
  html_hash: string | null;
  rect: AnchorRect | null;
  custom_name: string | null;
  area?: AnchorArea;
  /** The published path of the page the anchor is on (`index.html` for the
   * index; "" for an `artifact` anchor, which is on no page). */
  file: string;
  /** A live page's route (`?query#/hash-route`), set by the daemon; absent on artifacts. */
  route?: string;
  /** The quote as a reader sees it, spaced where its text runs from one
   * element into another (the quote is not, for re-anchoring); shown in
   * place of the quote. Absent when it would read as the quote. */
  label?: string;
}
/** A rectangle in the content frame's viewport pixels. */
export interface Box { x: number; y: number; w: number; h: number }
export type ResolveMethod = "exact" | "selector" | "quote" | "custom";
export interface AnchorResult { id: string; found: boolean; method: ResolveMethod | null; rect: Box | null }

/** A point in the page for the live layers (spec 2026-10-11-clax-dev-hosted-design
 * §16.3), anchored so it lands right at any width: the element at `path`
 * (child indexes from `<body>`, at most `LIVE_DEPTH` deep), the character
 * `offset` in its text (-1 when the point is on no text), and `x`, `y` as
 * fractions of the element's border box (used when `offset` is -1). */
export interface LivePoint { path: number[]; offset: number; x: number; y: number }
/** What a person has selected or is commenting on, as comment mode's picker
 * makes it: a text range (both ends on text), an element, or an area drawn
 * on an element (fractions of its border box). */
export type LiveSelection =
  | { kind: "range"; start: LivePoint; end: LivePoint }
  | { kind: "element"; path: number[] }
  | { kind: "area"; path: number[]; x: number; y: number; w: number; h: number };
/** Deepest element path a live point names. */
export const LIVE_DEPTH = 64;
/** Another person's pointer and selection as the frame draws them: an opaque
 * `key` (`k<n>`, stable within one document of the frame) and a colour
 * (`#rrggbb`); never a name or a person ID, since the artifact's own scripts
 * run in the frame. The shell draws the names (`clax:peer-at`). */
export interface FramePeer { key: string; color: string; anchor: LivePoint | null; selection: LiveSelection | null }
/** Most peers the frame draws. */
export const PEERS_MAX = 30;
/** The least time between two `clax:cursor` messages, in ms (about ten a second). */
export const LIVE_CURSOR_MS = 100;

/** A page's `claude.use(name)`; `name` is canonical (`self` is sent as `artifact`). */
export type UseRequest = { type: "clax:use"; id: string; name: string };
/** One namespace method call; `args` are structured-cloned from the page. */
export type CallRequest = { type: "clax:call"; id: string; ns: string; method: string; args: unknown[] };
/** `granted: false` resolves the page's `use()` to null; `config` is the declared capability object. */
export type UseResult = { type: "clax:use-result"; id: string; granted: boolean; config: unknown };
export type CallError = { code: string; message: string; [k: string]: unknown };
export type CallResult =
  | { type: "clax:call-result"; id: string; ok: true; value: unknown }
  | { type: "clax:call-result"; id: string; ok: false; error: CallError };
/** A push from the shell to a capability (db snapshots, comments callbacks). */
export type CapEvent = { type: "clax:event"; ns: string; topic: string; data: unknown };

export type ShellToBridge =
  /** `app`: the shell is the Clax app's (`/a/<id>?app=1`); the page then
   * reports its scroll position (`clax:scroll`), and scrolls to
   * `restoreScroll` (CSS pixels from the top) once the document is tall
   * enough, or after 2 s as far as it reaches: the app shell sends it when it
   * loaded a newer version in place of the one the viewer was scrolling. */
  | { type: "clax:welcome"; mode: "comment" | "view"; app?: boolean; restoreScroll?: number }
  | { type: "clax:comment-mode"; on: boolean }
  /** Moves the page to the fragment `hash` (`#…`, or `""` for none) in
   * place, replacing its history entry (the page's own `clax:hash` reports
   * where it moved by itself): the page navigates itself, since in
   * Chromium a fragment navigation of a cross-origin or sandboxed frame
   * that the shell starts (its `src`, its `location`) fires a load event at
   * the frame element with no new document, which the shell could not tell
   * from a document that never greeted. */
  | { type: "clax:hash"; hash: string }
  /** Greet again: the page answers with its `clax:hello`. The shell asks
   * after a load that brought no hello, which in Chromium can come with no
   * new document (Back or Forward across the page's own fragments). */
  | { type: "clax:greet" }
  /** After a welcome with `app: true`: return to the page's route at
   * `depth` (below the one it shows, as its `clax:route` reported), back
   * through the page's own entries, or in place once a fragment entry lies
   * among them (`routes.ts`), with a `popstate` the page's router follows. */
  | { type: "clax:back"; depth: number }
  /** `sameVersion`: the thread was made on the version the frame shows (an
   * area's element fingerprint is then not checked: its content may be live). */
  /** `highlight`: an open thread, whose quote the page highlights while its anchor resolves to text. */
  | { type: "clax:resolve-anchors"; requestId: string; anchors: { id: string; anchor: Anchor; sameVersion?: boolean; highlight?: boolean }[] }
  | { type: "clax:scroll-to"; anchor: Anchor; sameVersion?: boolean }
  /** The thread whose drawn area the page outlines dashed (hovered in the
   * sidebar or selected), by its ID in the latest `resolve-anchors`; null
   * for none. Threads that are not area anchors show nothing. */
  | { type: "clax:focus"; id: string | null }
  /** A key the viewer pressed or released in the shell while comment mode is
   * on and the pointer is over the frame (Option widening, see
   * `comment-mode.ts`); Escape drops a drag in progress, else ends comment
   * mode (the bridge answers `clax:cancel`). */
  | { type: "clax:key"; key: "Alt" | "ArrowUp" | "ArrowDown" | "Escape"; down: boolean }
  /** The shell did not take the pick `pickId` (its start came without the
   * viewer's gesture, outside comment mode, or beside another pick): no clip
   * is rendered for it. */
  | { type: "clax:pick-refused"; pickId: string }
  /** The composer the pick `pickId`'s start opened has focus: the bridge
   * renders its clip now, never before, so that work (which can hold the main
   * thread the page may share with the shell) never delays that focus. */
  | { type: "clax:composer-ready"; pickId: string }
  /** Other people's pointers (unnamed arrows) and selections, drawn in their
   * colours; replaces the last set. The page answers with `clax:peer-at`.
   * `here`: someone else is on the artifact (whether or not they show
   * anything), so the page reports this viewer's pointer; until a message
   * draws someone or says `here`, the page reports nothing. */
  | { type: "clax:peers"; peers: FramePeer[]; here?: boolean }
  /** The app's inspect view (`/a/<id>?app=1&inspect=1`, `web/shell/src/app/host.ts`)
   * asks for an inspection of the page (`inspect.ts`): it waits until the
   * page settles (at most `settleMs`), then answers `clax:inspection` with
   * the same `requestId`, its screenshot at most `maxBytes`. */
  | { type: "clax:inspect"; requestId: string; settleMs?: number; maxBytes?: number }
  /** The app's inspect view asks the bridge to run the draft's test files
   * (`harness.ts`), paths relative to the page, each test for at most
   * `testMs`, the run for at most `totalMs`; `clax:tests` answers with the
   * same `requestId`. Honoured only in the frame named `INSPECT_FRAME`. */
  | { type: "clax:test"; requestId: string; files: string[]; testMs?: number; totalMs?: number }
  /** The app's inspect view asks the bridge to run an app bundle's
   * migration (`harness.ts`): import `file` (relative to the page), call its
   * default export with the page's `db` and `{from, to}`, all within
   * `totalMs`; `clax:migration` answers with the same `requestId`.
   * Honoured only in the frame named `MIGRATE_FRAME`. */
  | { type: "clax:migrate"; requestId: string; file: string; from: number; to: number; totalMs?: number }
  | UseResult
  | CallResult
  | CapEvent;

/** The name (`window.name`) the app's inspect view gives its content frame:
 * the bridge keeps the page's errors from its first script only there. A
 * page can name its own frame so; it then only keeps its own errors. */
export const INSPECT_FRAME = "clax-inspect";
/** The name the app's migration view (`?app=1&inspect=1&migrate=1`) gives
 * its content frame: there the bridge, the document's first script, ends
 * the page's markup before any of it runs (the rest of the document parses
 * as hidden text), so only an app bundle's migration (`clax:migrate`) runs
 * against the page's `db`. */
export const MIGRATE_FRAME = "clax-migrate";
/** One of a page's errors since its first script (`inspect.ts`): an `error`
 * event, an unhandled promise rejection, or a `console.error` call. */
export interface InspectError { kind: "error" | "unhandledrejection" | "console"; text: string }
/** What an inspection found (`inspect.ts`): `png`, the viewport; `failed`,
 * the URLs of resources that failed to load; `text`, an outline of the
 * page; `height`, the document's scroll height in CSS pixels. */
export interface Inspection { png: ArrayBuffer; errors: InspectError[]; failed: string[]; text: string; title: string; height: number }
/** What a test run found (`harness.ts`): how many tests `passed`, each that
 * `failed` (or test file that did not load) with why, and how many were
 * `skipped` because the run's time ran out. */
export interface TestResults { passed: number; failed: { name: string; error: string }[]; skipped: number }
/** What a migration run found (`harness.ts`): it finished, or why not. */
export type MigrationResult = { ok: true } | { ok: false; error: string };

/** What a picked element is, for the app's words about it (`describe.ts`):
 * its tag, its ARIA role and its accessible name, as the page gives them, and
 * `text`, its text spaced where it runs from one element into another (the
 * anchor's quote is not), for showing in place of the quote. */
export interface PickDesc { tag: string; role: string | null; name: string | null; text?: string | null }

export type BridgeToShell =
  | { type: "clax:hello"; artifact: string; version: number; file: string }
  | { type: "clax:hover"; selector: string | null; rect: Box | null }
  /** Sent at the viewer's pick itself (the click or release), before its
   * clip is rendered, with the pick's anchor: the shell opens the composer
   * for it at once, focused and waiting for the screenshot, only when this
   * arrived while the frame held the viewer's gesture. */
  | { type: "clax:pick-start"; pickId: string; version: number; anchor: Anchor; desc?: PickDesc }
  /** The pick's screenshot (or why there is none); the shell takes it only
   * for the composer its accepted start opened, and ignores the anchor here. */
  | { type: "clax:pick"; pickId: string; version: number; anchor: Anchor; clipPng?: ArrayBuffer; clipError?: string }
  | { type: "clax:anchors"; requestId: string | null; results: AnchorResult[] }
  | { type: "clax:cancel" }
  /** The page is going away (`pagehide`): the shell closes the gate until the
   * next document greets, so nothing it sends reaches whatever document the
   * frame shows next. It may arrive once that document has replaced the page,
   * with no `source` (see `acceptByeFromFrame`). The page can post this too,
   * which only cuts off itself. */
  | { type: "clax:bye" }
  /** A plain click on a link to another page of this version, cancelled in the
   * page and handed to the shell to follow (see `nav.ts`); `hash` is the
   * link's fragment, with its `#`. */
  | { type: "clax:navigate"; file: string; hash?: string }
  /** The page's fragment (`#…`, or "" for none), after the welcome and on every change. */
  | { type: "clax:hash"; hash: string }
  /** A lazy part of the bridge could not load (the page's own CSP forbids it,
   * say); sent once per part, after the welcome. `message` is the browser's
   * error, for debugging: the page can post this too, so the shell shows only
   * its own words for `part`, and none for `live` (the live layers are best
   * effort: others' pointers are simply not drawn). */
  | { type: "clax:degraded"; part: "comment" | "clip" | "caps" | "room" | "sample" | "live" | "test"; message: string }
  /** This viewer's pointer (null once it left the page) and selection: the
   * text selected, or the area being drawn in comment mode; and `pick`, the
   * last pick made in comment mode (what the shell shows others while that
   * pick's composer is open). At most every `LIVE_CURSOR_MS` and only when
   * it changed; sent only once someone else is here (`clax:peers`). */
  | { type: "clax:cursor"; anchor: LivePoint | null; selection: LiveSelection | null; pick?: LiveSelection | null }
  /** Where the page drew each peer's pointer, in its viewport's pixels,
   * after each draw, scroll and resize: the shell puts the names there. */
  | { type: "clax:peer-at"; peers: { key: string; x: number; y: number }[] }
  /** The page's vertical scroll position (CSS pixels) on its `file`, after a
   * welcome with `app: true`: at most every 250 ms while the page scrolls,
   * the last position of a burst always sent; only the app shell reads it
   * (`web/shell/src/app/host.ts`). */
  | { type: "clax:scroll"; file: string; y: number }
  /** The page moved among its own `history.pushState` routes, after a
   * welcome with `app: true` (`routes.ts`): `depth` is the number of the
   * page's pushes above the entry it loaded at, `hash` its fragment; only the
   * app shell reads it, for its back. */
  | { type: "clax:route"; depth: number; hash: string }
  /** The answer to `clax:inspect` (`inspect.ts`): the viewport as a PNG,
   * the page's errors and failed loads, an outline of its text, its title
   * and height; or why there is none. The page can post this too, so the
   * shell takes it only for the request it has open and checks its shape. */
  | ({ type: "clax:inspection"; requestId: string } & (Inspection | { error: string }))
  /** The answer to `clax:test` (`harness.ts`), or why there is none. The page
   * can post this too: the shell takes it only for the request it has open
   * and checks its shape. */
  | ({ type: "clax:tests"; requestId: string } & (TestResults | { error: string }))
  /** The answer to `clax:migrate` (`harness.ts`). The page can post this
   * too: the shell takes it only for the request it has open and checks its
   * shape. */
  | ({ type: "clax:migration"; requestId: string } & MigrationResult)
  | UseRequest
  | CallRequest;

export const SHELL_TYPES: ReadonlySet<string> = new Set(["clax:welcome", "clax:comment-mode", "clax:hash", "clax:greet", "clax:back", "clax:resolve-anchors", "clax:scroll-to", "clax:focus", "clax:key", "clax:pick-refused", "clax:composer-ready", "clax:inspect", "clax:test", "clax:migrate", "clax:peers", "clax:use-result", "clax:call-result", "clax:event"]);
export const BRIDGE_TYPES: ReadonlySet<string> = new Set(["clax:hello", "clax:hover", "clax:pick-start", "clax:pick", "clax:anchors", "clax:cancel", "clax:bye", "clax:navigate", "clax:hash", "clax:degraded", "clax:cursor", "clax:peer-at", "clax:scroll", "clax:route", "clax:inspection", "clax:tests", "clax:migration", "clax:use", "clax:call"]);
