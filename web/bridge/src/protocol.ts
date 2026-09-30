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
}
/** A rectangle in the content frame's viewport pixels. */
export interface Box { x: number; y: number; w: number; h: number }
export type ResolveMethod = "exact" | "selector" | "quote" | "custom";
export interface AnchorResult { id: string; found: boolean; method: ResolveMethod | null; rect: Box | null }

/** A page's `claude.use(name)`; `name` is canonical (`self` is sent as `artifact`). */
export type UseRequest = { type: "artifax:use"; id: string; name: string };
/** One namespace method call; `args` are structured-cloned from the page. */
export type CallRequest = { type: "artifax:call"; id: string; ns: string; method: string; args: unknown[] };
/** `granted: false` resolves the page's `use()` to null; `config` is the declared capability object. */
export type UseResult = { type: "artifax:use-result"; id: string; granted: boolean; config: unknown };
export type CallError = { code: string; message: string; [k: string]: unknown };
export type CallResult =
  | { type: "artifax:call-result"; id: string; ok: true; value: unknown }
  | { type: "artifax:call-result"; id: string; ok: false; error: CallError };
/** A push from the shell to a capability (db snapshots, comments callbacks). */
export type CapEvent = { type: "artifax:event"; ns: string; topic: string; data: unknown };

export type ShellToBridge =
  | { type: "artifax:welcome"; mode: "comment" | "view" }
  | { type: "artifax:comment-mode"; on: boolean }
  /** `sameVersion`: the thread was made on the version the frame shows (an
   * area's element fingerprint is then not checked: its content may be live). */
  | { type: "artifax:resolve-anchors"; requestId: string; anchors: { id: string; anchor: Anchor; sameVersion?: boolean }[] }
  | { type: "artifax:scroll-to"; anchor: Anchor; sameVersion?: boolean }
  /** The thread whose drawn area the page outlines dashed (hovered in the
   * sidebar or selected), by its ID in the latest `resolve-anchors`; null
   * for none. Threads that are not area anchors show nothing. */
  | { type: "artifax:focus"; id: string | null }
  /** A key the viewer pressed or released in the shell while comment mode is
   * on and the pointer is over the frame (Option widening, see
   * `comment-mode.ts`); Escape drops a drag in progress, else ends comment
   * mode (the bridge answers `artifax:cancel`). */
  | { type: "artifax:key"; key: "Alt" | "ArrowUp" | "ArrowDown" | "Escape"; down: boolean }
  | UseResult
  | CallResult
  | CapEvent;

export type BridgeToShell =
  | { type: "artifax:hello"; artifact: string; version: number; file: string }
  | { type: "artifax:hover"; selector: string | null; rect: Box | null }
  /** Sent at the viewer's pick itself (the click or release), before its
   * clip is rendered: the shell takes the `artifax:pick` with this `pickId`
   * only when this arrived while the frame held the viewer's gesture. */
  | { type: "artifax:pick-start"; pickId: string }
  | { type: "artifax:pick"; pickId: string; version: number; anchor: Anchor; clipPng?: ArrayBuffer; clipError?: string }
  | { type: "artifax:anchors"; requestId: string | null; results: AnchorResult[] }
  | { type: "artifax:cancel" }
  /** A plain click on a link to another page of this version, cancelled in the
   * page and handed to the shell to follow (see `nav.ts`); `hash` is the
   * link's fragment, with its `#`. */
  | { type: "artifax:navigate"; file: string; hash?: string }
  /** The page's fragment (`#…`, or "" for none), after the welcome and on every change. */
  | { type: "artifax:hash"; hash: string }
  | UseRequest
  | CallRequest;

export const SHELL_TYPES: ReadonlySet<string> = new Set(["artifax:welcome", "artifax:comment-mode", "artifax:resolve-anchors", "artifax:scroll-to", "artifax:focus", "artifax:key", "artifax:use-result", "artifax:call-result", "artifax:event"]);
export const BRIDGE_TYPES: ReadonlySet<string> = new Set(["artifax:hello", "artifax:hover", "artifax:pick-start", "artifax:pick", "artifax:anchors", "artifax:cancel", "artifax:navigate", "artifax:hash", "artifax:use", "artifax:call"]);
