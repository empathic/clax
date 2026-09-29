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

export type ShellToBridge =
  | { type: "artifax:welcome"; mode: "comment" | "view" }
  | { type: "artifax:comment-mode"; on: boolean }
  | { type: "artifax:resolve-anchors"; requestId: string; anchors: { id: string; anchor: Anchor }[] }
  | { type: "artifax:scroll-to"; anchor: Anchor };

export type BridgeToShell =
  | { type: "artifax:hello"; artifact: string; version: number }
  | { type: "artifax:hover"; selector: string | null; rect: Box | null }
  | { type: "artifax:pick"; pickId: string; version: number; anchor: Anchor; clipPng?: ArrayBuffer; clipError?: string }
  | { type: "artifax:anchors"; requestId: string | null; results: AnchorResult[] }
  | { type: "artifax:cancel" };

export const SHELL_TYPES: ReadonlySet<string> = new Set(["artifax:welcome", "artifax:comment-mode", "artifax:resolve-anchors", "artifax:scroll-to"]);
export const BRIDGE_TYPES: ReadonlySet<string> = new Set(["artifax:hello", "artifax:hover", "artifax:pick", "artifax:anchors", "artifax:cancel"]);
