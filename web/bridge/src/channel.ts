// Which window and origins the bridge takes orders from (spec §9: the bridge
// never trusts messages that do not come from its shell's window and origin).

import { SHELL_TYPES, type ShellToBridge } from "./protocol";

/** The origins the shell can have, from the origin the bridge tag names
 * (`data-origin`, the daemon's public origin): that origin, and for a
 * local daemon's (`localhost` or `127.0.0.1`) each of those names
 * at its port, as the shell may be reached by any. None without one, or
 * for one that does not parse: the bridge then takes orders from no one. */
export function shellOrigins(origin: string | undefined): string[] {
  try {
    const u = new URL(origin!), loopback = ["localhost", "127.0.0.1"];
    return loopback.includes(u.hostname) ? loopback.map(h => `${u.protocol}//${h}${u.port && ":" + u.port}`) : [u.origin];
  } catch { return []; }
}

/** The keys the shell forwards to comment mode (`clax:key`). */
const FORWARDED_KEYS: ReadonlySet<string> = new Set(["Alt", "ArrowUp", "ArrowDown", "Escape"]);

/** The key and direction of a forwarded `clax:key`, or null for any other
 * key or a malformed message. */
export function forwardedKey(m: { key?: unknown; down?: unknown }): { key: string; down: boolean } | null {
  return typeof m.key === "string" && FORWARDED_KEYS.has(m.key) && typeof m.down === "boolean" ? { key: m.key, down: m.down } : null;
}

/** The message when the browser delivered it (a page's own `dispatchEvent`
 * of a made-up `MessageEvent`, which can name any source and origin, is not
 * trusted), from `parent` at one of `origins`, with a shell message type. */
export function acceptFromShell(e: MessageEvent, parent: Window | null, origins: string[]): ShellToBridge | null {
  if (!e.isTrusted || !parent || e.source !== parent || !origins.includes(e.origin)) return null;
  const d = e.data;
  if (!d || typeof d !== "object" || !SHELL_TYPES.has(d.type)) return null;
  return d as ShellToBridge;
}
