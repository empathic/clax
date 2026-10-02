// The capability namespaces of contract 0.2.61 (web/contract/0.2.61): each
// name `use()` can resolve and the members its namespace carries. The lists
// are checked against the .d.ts files (bridge/test/capabilities.test.ts).
import type { Rpc } from "./rpc";

export const CAPABILITY_METHODS = {
  permissions: ["state", "request"],
  artifact: ["publish", "edit", "sync"],
  db: ["doc", "collection"],
  downloads: ["save"],
  user: ["isOwner", "canEdit", "can", "me", "id", "profiles", "name", "avatarUrl", "search", "email"],
  comments: ["openComposer", "anchorFor", "create", "reply", "sendToClaude", "canSendToClaude", "resolve", "delete", "customAnchors", "working", "onWorking"],
  assets: ["upload", "list", "delete"],
} as const satisfies Record<string, readonly string[]>;

export type CapabilityName = keyof typeof CAPABILITY_METHODS;

/** Other spellings `use()` accepts, mapped to their canonical name. */
export const ALIASES: Readonly<Record<string, CapabilityName>> = Object.freeze({ self: "artifact" });

export function isCapabilityName(name: string): name is CapabilityName {
  return Object.prototype.hasOwnProperty.call(CAPABILITY_METHODS, name);
}

/** Members implemented in the page (validation, builders, DOM access) instead of a plain shell call. */
export type Local = Partial<Record<string, (...args: never[]) => unknown>>;

/** The frozen namespace of `name`: each member from `local`, else a call to the shell. */
export function buildNamespace(name: CapabilityName, rpc: Pick<Rpc, "call">, local: Local = {}): Readonly<Record<string, unknown>> {
  const ns: Record<string, unknown> = {};
  for (const m of CAPABILITY_METHODS[name]) ns[m] = local[m] ?? ((...args: unknown[]) => rpc.call(name, m, args));
  return Object.freeze(ns);
}
