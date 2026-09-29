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

/** The message when it came from `parent` at one of `origins` with a shell message type. */
export function acceptFromShell(e: MessageEvent, parent: Window | null, origins: string[]): ShellToBridge | null {
  if (!parent || e.source !== parent || !origins.includes(e.origin)) return null;
  const d = e.data;
  if (!d || typeof d !== "object" || !SHELL_TYPES.has(d.type)) return null;
  return d as ShellToBridge;
}
