import { BRIDGE_TYPES, type BridgeToShell, type ShellToBridge } from "../../bridge/src/protocol";

/** The message when it came from the content frame's window: at `frameOrigin`
 * in subdomain mode, or from an opaque origin ("null") in sandbox mode. */
export function acceptFromFrame(e: MessageEvent, frame: Window | null, frameOrigin: string | null): BridgeToShell | null {
  if (!frame || e.source !== frame) return null;
  if (frameOrigin ? e.origin !== frameOrigin : e.origin !== "null") return null;
  const d = e.data;
  if (!d || typeof d !== "object" || !BRIDGE_TYPES.has(d.type)) return null;
  return d as BridgeToShell;
}

/** Posts to the frame: to its origin in subdomain mode, to "*" for an opaque-origin sandbox. */
export function sendToFrame(frame: Window | null, frameOrigin: string | null, m: ShellToBridge): void {
  frame?.postMessage(m, frameOrigin ?? "*");
}

/** Whether a hello comes from the artifact and version the shell is showing;
 * any other hello (a stale or foreign document in the frame) is ignored. */
export function helloMatches(m: Extract<BridgeToShell, { type: "artifax:hello" }>, artifact: string, version: number): boolean {
  return m.artifact === artifact && m.version === version;
}
