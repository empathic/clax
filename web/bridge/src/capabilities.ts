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

/** Capabilities whose page side is a lazy part of its own (parts/<name>.ts):
 * the part builds the whole frozen namespace, and its member list lives there. */
export const PART_CAPABILITIES = ["room", "sample"] as const;
export type PartCapabilityName = (typeof PART_CAPABILITIES)[number];
export type UsableName = CapabilityName | PartCapabilityName;

export function isPartCapability(name: string): name is PartCapabilityName {
  return (PART_CAPABILITIES as readonly string[]).includes(name);
}

export function isCapabilityName(name: string): name is UsableName {
  return Object.prototype.hasOwnProperty.call(CAPABILITY_METHODS, name) || isPartCapability(name);
}

/** Members implemented in the page (validation, builders, DOM access) instead of a plain shell call. */
export type Local = Partial<Record<string, (...args: never[]) => unknown>>;

/** The frozen namespace of `name`: each member from `local`, else a call to the shell. */
export function buildNamespace(name: CapabilityName, rpc: Pick<Rpc, "call">, local: Local = {}): Readonly<Record<string, unknown>> {
  const ns: Record<string, unknown> = {};
  for (const m of CAPABILITY_METHODS[name]) ns[m] = local[m] ?? ((...args: unknown[]) => rpc.call(name, m, args));
  return Object.freeze(ns);
}
