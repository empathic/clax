// Messages between the shell and the bridge in the content frame. The shell
// imports these types from here.

export type AnchorKind = "element" | "range" | "custom" | "area";
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
  /** The published path of the page the anchor is on (`index.html` for the index). */
  file: string;
  /** A live page's route (`?query#/hash-route`), set by the daemon; absent on artifacts. */
  route?: string;
}
/** A rectangle in the content frame's viewport pixels. */
export interface Box { x: number; y: number; w: number; h: number }
export type ResolveMethod = "exact" | "selector" | "quote" | "custom";
export interface AnchorResult { id: string; found: boolean; method: ResolveMethod | null; rect: Box | null }

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
  | { type: "clax:welcome"; mode: "comment" | "view" }
  | { type: "clax:comment-mode"; on: boolean }
  /** `sameVersion`: the thread was made on the version the frame shows (an
   * area's element fingerprint is then not checked: its content may be live). */
  | { type: "clax:resolve-anchors"; requestId: string; anchors: { id: string; anchor: Anchor; sameVersion?: boolean }[] }
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
  | UseResult
  | CallResult
  | CapEvent;

export type BridgeToShell =
  | { type: "clax:hello"; artifact: string; version: number; file: string }
  | { type: "clax:hover"; selector: string | null; rect: Box | null }
  /** Sent at the viewer's pick itself (the click or release), before its
   * clip is rendered, with the pick's anchor: the shell opens the composer
   * for it at once, focused and waiting for the screenshot, only when this
   * arrived while the frame held the viewer's gesture. */
  | { type: "clax:pick-start"; pickId: string; version: number; anchor: Anchor }
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
   * its own words for `part`. */
  | { type: "clax:degraded"; part: "comment" | "clip" | "caps" | "room" | "sample"; message: string }
  | UseRequest
  | CallRequest;

export const SHELL_TYPES: ReadonlySet<string> = new Set(["clax:welcome", "clax:comment-mode", "clax:resolve-anchors", "clax:scroll-to", "clax:focus", "clax:key", "clax:pick-refused", "clax:composer-ready", "clax:use-result", "clax:call-result", "clax:event"]);
export const BRIDGE_TYPES: ReadonlySet<string> = new Set(["clax:hello", "clax:hover", "clax:pick-start", "clax:pick", "clax:anchors", "clax:cancel", "clax:bye", "clax:navigate", "clax:hash", "clax:degraded", "clax:use", "clax:call"]);
