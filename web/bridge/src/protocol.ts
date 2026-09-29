// Messages between the shell and the bridge in the content frame. The shell
// imports these types from here.

export type AnchorKind = "element" | "range" | "custom";
export interface AnchorRect { x: number; y: number; w: number; h: number; scrollX: number; scrollY: number; viewportW: number }
/** Spec §9 "Anchors"; field names match the daemon's JSON. */
export interface Anchor {
  kind: AnchorKind;
  selector: string | null;
  quote: string | null;
  prefix: string | null;
  suffix: string | null;
  html_hash: string | null;
  rect: AnchorRect | null;
  custom_name: string | null;
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
  | { type: "artifax:resolve-anchors"; requestId: string; anchors: { id: string; anchor: Anchor }[] }
  | { type: "artifax:scroll-to"; anchor: Anchor }
  | UseResult
  | CallResult
  | CapEvent;

export type BridgeToShell =
  | { type: "artifax:hello"; artifact: string; version: number }
  | { type: "artifax:hover"; selector: string | null; rect: Box | null }
  | { type: "artifax:pick"; pickId: string; version: number; anchor: Anchor; clipPng?: ArrayBuffer; clipError?: string }
  | { type: "artifax:anchors"; requestId: string | null; results: AnchorResult[] }
  | { type: "artifax:cancel" }
  | UseRequest
  | CallRequest;

export const SHELL_TYPES: ReadonlySet<string> = new Set(["artifax:welcome", "artifax:comment-mode", "artifax:resolve-anchors", "artifax:scroll-to", "artifax:use-result", "artifax:call-result", "artifax:event"]);
export const BRIDGE_TYPES: ReadonlySet<string> = new Set(["artifax:hello", "artifax:hover", "artifax:pick", "artifax:anchors", "artifax:cancel", "artifax:use", "artifax:call"]);
