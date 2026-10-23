// The `home` namespace (clax-extensions.d.ts `ClaxExtensions.Home`), built
// whole in the caps part: each member in HOME_METHODS is a plain shell
// call, and `on(name, fn)` hears the shell's `home`/<name> pushes (`apps`,
// `makes`, `unread`, `key`, `chrome`), each the topic's whole latest value, and
// returns the function that stops it.
import type { Rpc } from "../rpc";

export const HOME_METHODS = ["apps", "makes", "unread", "needsKey", "open", "openMake", "newApp", "menu", "openInbox", "openSettings", "importApp", "getApps", "shown", "tiles", "chrome"] as const;

export const homeNamespace = (rpc: Pick<Rpc, "call" | "on">): Readonly<Record<string, unknown>> => Object.freeze({
  ...Object.fromEntries(HOME_METHODS.map(m => [m, (...args: unknown[]) => rpc.call("home", m, args)])),
  on: (name: string, fn: (v: unknown) => void) => rpc.on("home", name, fn),
});
