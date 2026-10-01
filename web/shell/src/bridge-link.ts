import { BRIDGE_TYPES, type BridgeToShell, type ShellToBridge } from "../../bridge/src/protocol";

/** The message when the browser delivered it (`isTrusted`, so no script in
 * the shell's origin forged it) from the content frame's window: at
 * `frameOrigin` in subdomain mode, or from an opaque origin ("null") in
 * sandbox mode. */
export function acceptFromFrame(e: MessageEvent, frame: Window | null, frameOrigin: string | null): BridgeToShell | null {
  if (!e.isTrusted || !frame || e.source !== frame) return null;
  if (frameOrigin ? e.origin !== frameOrigin : e.origin !== "null") return null;
  const d = e.data;
  if (!d || typeof d !== "object" || !BRIDGE_TYPES.has(d.type)) return null;
  return d as BridgeToShell;
}

/** A `clax:bye` from the frame's document as it goes away. The document
 * posts it on `pagehide`, and in Chromium it arrives once the next document
 * has replaced it, so its `source` is null rather than the frame's window:
 * it is taken from either, when the browser delivered it at the frame's
 * origin. With no source, the origin is the only check: in sandbox mode any
 * opaque-origin document that holds a reference to the shell's window (a
 * sandboxed frame of a page that opened the shell, say) could post one. The
 * most a forged bye does is close the gate until the frame's next hello,
 * less than such an opener can do by navigating the shell. */
export function acceptByeFromFrame(e: MessageEvent, frame: Window | null, frameOrigin: string | null): boolean {
  if (!e.isTrusted || !frame || (e.source !== frame && e.source !== null)) return false;
  if (frameOrigin ? e.origin !== frameOrigin : e.origin !== "null") return false;
  const d = e.data;
  return !!d && typeof d === "object" && d.type === "clax:bye";
}

/** Posts to the frame: to its origin in subdomain mode, to "*" for an opaque-origin sandbox. */
export function sendToFrame(frame: Window | null, frameOrigin: string | null, m: ShellToBridge): void {
  frame?.postMessage(m, frameOrigin ?? "*");
}

/** Whether a hello comes from the artifact and version the shell is showing;
 * any other hello (a stale or foreign document in the frame) is ignored. */
export function helloMatches(m: Extract<BridgeToShell, { type: "clax:hello" }>, artifact: string, version: number): boolean {
  return m.artifact === artifact && m.version === version;
}
