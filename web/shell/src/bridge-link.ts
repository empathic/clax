import { BRIDGE_TYPES, type BridgeToShell, type ShellToBridge } from "../../bridge/src/protocol";

/** The message when the browser delivered it (`isTrusted`, so no script in
 * the shell's origin forged it) from the content frame's window at
 * `frameOrigin`, the artifact's site origin. */
export function acceptFromFrame(e: MessageEvent, frame: Window | null, frameOrigin: string): BridgeToShell | null {
  if (!e.isTrusted || !frame || e.source !== frame || e.origin !== frameOrigin) return null;
  const d = e.data;
  if (!d || typeof d !== "object" || !BRIDGE_TYPES.has(d.type)) return null;
  return d as BridgeToShell;
}

/** A `clax:bye` from the frame's document as it goes away. The document
 * posts it on `pagehide`, and in Chromium it arrives once the next document
 * has replaced it, so its `source` is null rather than the frame's window:
 * it is taken from either, when the browser delivered it at the frame's
 * origin. With no source, the origin is the only check: another document
 * of the artifact's own site could post one. The most a forged bye does is
 * close the gate until the frame's next hello. */
export function acceptByeFromFrame(e: MessageEvent, frame: Window | null, frameOrigin: string): boolean {
  if (!e.isTrusted || !frame || (e.source !== frame && e.source !== null) || e.origin !== frameOrigin) return false;
  const d = e.data;
  return !!d && typeof d === "object" && d.type === "clax:bye";
}

/** The handshake's state (`clax:site-state`, posted by the site's
 * `/_clax/session/start`), when the browser delivered it from the content
 * frame's window at the site origin: the state and the path it leads to. */
export function acceptSiteState(e: MessageEvent, frame: Window | null, frameOrigin: string): { state: string; next: string } | null {
  if (!e.isTrusted || !frame || e.source !== frame || e.origin !== frameOrigin) return null;
  const d = e.data;
  if (!d || typeof d !== "object" || d.type !== "clax:site-state" || typeof d.state !== "string" || typeof d.next !== "string") return null;
  return { state: d.state, next: d.next };
}

/** Posts to the frame, at its origin only. */
export function sendToFrame(frame: Window | null, frameOrigin: string, m: ShellToBridge): void {
  frame?.postMessage(m, frameOrigin);
}

/** Whether a hello comes from the artifact and version the shell is showing;
 * any other hello (a stale or foreign document in the frame) is ignored. */
export function helloMatches(m: Extract<BridgeToShell, { type: "clax:hello" }>, artifact: string, version: number): boolean {
  return m.artifact === artifact && m.version === version;
}
