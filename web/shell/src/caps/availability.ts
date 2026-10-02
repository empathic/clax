// Which capabilities a view serves (the Availability table of the phase 4 plan,
// spec §9): declared ones, `permissions` and `user` always (user.d.ts: its
// universal members need no declaration), `assets` only to the owner shell;
// `room` when declared; files, mcp, and sample never.

export type Declared = Record<string, Record<string, unknown> | undefined>;

/** The declarable capabilities this runtime serves, in the order `permissions.state()` lists them. */
export const CAPABILITIES = ["artifact", "db", "downloads", "user", "comments", "assets", "room"] as const;

const declares = (name: string, declared: Declared) =>
  Object.prototype.hasOwnProperty.call(declared, name) || (name === "artifact" && Object.prototype.hasOwnProperty.call(declared, "self"));

/** Whether `use(name)` resolves a namespace for this view; `owner` is the shell holding the token. */
export function isAvailable(name: string, declared: Declared, owner: boolean): boolean {
  switch (name) {
    case "permissions": return true;
    case "user": return true; // user.d.ts: isOwner, canEdit, can, and me need no declaration
    case "artifact": case "db": case "downloads": case "comments": return declares(name, declared);
    case "room": return declares(name, declared);
    case "assets": return owner && declares(name, declared);
    default: return false;
  }
}

/** Whether a capability asks the viewer before its first write. */
export function consentGated(name: string, declared: Declared): boolean {
  return name === "comments" && declared.comments?.composer_only !== true;
}

/** The declared object for `name` (`artifact` falls back to `self`), `{}` when declared without one. */
export function declaredConfig(name: string, declared: Declared): Record<string, unknown> {
  return declared[name] ?? (name === "artifact" ? declared.self : undefined) ?? {};
}
