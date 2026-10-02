// Which capabilities a view serves (the Availability table of the phase 4 plan,
// spec §9): declared ones, `permissions` and `user` always (user.d.ts: its
// universal members need no declaration), `assets` only to the owner shell;
// `room` when declared; `sample` to the owner shell, when declared and the
// daemon has a provider; files and mcp never.

export type Declared = Record<string, Record<string, unknown> | undefined>;

/** The declarable capabilities this runtime serves, in the order `permissions.state()` lists them. */
/** What this daemon serves beyond the declaration: `sample` needs a provider
 * (`GET /api/artifacts/<aid>/sample`), asked when the page names it. */
export type Served = { sample?: boolean };

export const CAPABILITIES = ["artifact", "db", "downloads", "user", "comments", "assets", "room", "sample"] as const;

const declares = (name: string, declared: Declared) =>
  Object.prototype.hasOwnProperty.call(declared, name) || (name === "artifact" && Object.prototype.hasOwnProperty.call(declared, "self"));

/** Whether `use(name)` resolves a namespace for this view; `owner` is the shell holding the token. */
export function isAvailable(name: string, declared: Declared, owner: boolean, served: Served = {}): boolean {
  switch (name) {
    case "permissions": return true;
    case "user": return true; // user.d.ts: isOwner, canEdit, can, and me need no declaration
    case "artifact": case "db": case "downloads": case "comments": return declares(name, declared);
    case "room": return declares(name, declared);
    case "assets": return owner && declares(name, declared);
    // The owner's browser only: the key is the owner's, and a LAN viewer cannot spend it.
    case "sample": return owner && declares(name, declared) && served.sample === true;
    default: return false;
  }
}

/** Whether a capability asks the viewer before its first write. */
export function consentGated(name: string, declared: Declared): boolean {
  return name === "sample" || (name === "comments" && declared.comments?.composer_only !== true);
}

/** The declared object for `name` (`artifact` falls back to `self`), `{}` when declared without one. */
export function declaredConfig(name: string, declared: Declared): Record<string, unknown> {
  return declared[name] ?? (name === "artifact" ? declared.self : undefined) ?? {};
}
