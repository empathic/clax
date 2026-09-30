// Which window and origins the bridge takes orders from (spec §9: the bridge
// never trusts messages that do not come from its shell's window and origin).

import { SHELL_TYPES, type ShellToBridge } from "./protocol";

/** The origins the shell can have. Content on `<aid>.localhost:<port>` is
 * framed by the shell on the loopback names at the same port; path-based
 * content (`/c/...`, possibly in an opaque-origin sandbox) is framed by the
 * shell at the content URL's own scheme, host, and port. */
export function shellOrigins(href: string): string[] {
  const u = new URL(href);
  const port = u.port ? `:${u.port}` : "";
  if (u.hostname.endsWith(".localhost")) return ["localhost", "127.0.0.1", "[::1]"].map(h => `${u.protocol}//${h}${port}`);
  return [`${u.protocol}//${u.host}`];
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
