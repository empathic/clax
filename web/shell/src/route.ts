// Shell URLs. `/a/<id>` shows an artifact's latest version, `/a/<id>/v/<n>`
// version n; either may be followed by `/<file>`, the published path of the
// page the frame shows (segments percent-encoded; none for `index.html`).
// After `/a/<id>`, a `v` segment followed by an all-digit segment is a
// version; anything else starts the file path. So a file published under
// `v/<digits>/` is reachable only through the versioned form. `/a/<id>/get`
// (a trailing `/` allowed) is the daemon's Get in Clax page, not a file: a
// file published as `get` too is reachable only through the versioned form.

export const INDEX = "index.html";

const INSPECT_QUERY = "?app=1&inspect=1";
const MIGRATE_QUERY = "?app=1&inspect=1&migrate=1";
/** The shell URL query a view carries from page to page: `?app=1` for the
 * Clax app's view, `?app=1&inspect=1` for its inspect view (`app/host.ts`),
 * `?app=1&inspect=1&migrate=1` for its migration view (the inspect view
 * whose page runs none of its own markup, `MIGRATE_FRAME`), else
 * none (any other query, `inspect=1` without `app=1` included, stays where
 * it was given). */
export const appQuery = (search: string): string => {
  const q = new URLSearchParams(search);
  return q.get("app") !== "1" ? "" : q.get("inspect") !== "1" ? "?app=1" : q.get("migrate") === "1" ? MIGRATE_QUERY : INSPECT_QUERY;
};
/** Whether `search` asks for the Clax app's inspect view (the migration view
 * included): `inspect=1` with `app=1`. */
export const inspectQuery = (search: string): boolean => appQuery(search).startsWith(INSPECT_QUERY);
/** Whether `search` asks for the Clax app's migration view: `migrate=1` too. */
export const migrateQuery = (search: string): boolean => appQuery(search) === MIGRATE_QUERY;
const ID = /^[0-9a-hj-km-np-tv-z]{12}$/;

export type ShellRoute =
  | { kind: "gallery" }
  | { kind: "get"; id: string }
  | { kind: "artifact"; id: string; version: number | null; file: string };

/** The route a shell path names; anything that is not an artifact path is the gallery. */
export function parseShellPath(pathname: string): ShellRoute {
  const segs = pathname.split("/");
  if (segs[0] !== "" || segs[1] !== "a" || !ID.test(segs[2] ?? "")) return { kind: "gallery" };
  let rest = segs.slice(3);
  if (rest[0] === "get" && (rest.length === 1 || (rest.length === 2 && rest[1] === ""))) return { kind: "get", id: segs[2] };
  let version: number | null = null;
  if (rest[0] === "v" && /^\d+$/.test(rest[1] ?? "")) {
    version = Number(rest[1]);
    rest = rest.slice(2);
  }
  if (rest.at(-1) === "") rest = rest.slice(0, -1);
  let file: string;
  try { file = rest.map(decodeURIComponent).join("/"); } catch { return { kind: "gallery" }; }
  return { kind: "artifact", id: segs[2], version, file: file || INDEX };
}

/** The shell path for `file` of version `version` (null: the latest). A file
 * the unversioned form would read otherwise (`v/<digits>/…` as a version,
 * `get` as the Get in Clax page) is given `shown`, the version on screen. */
export function shellPath(id: string, version: number | null, file: string, shown?: number): string {
  if (version === null && (file === "get" || /^v\/\d+(\/|$)/.test(file)) && shown !== undefined) version = shown;
  const v = version === null ? "" : `/v/${version}`;
  const f = file === INDEX ? "" : `/${file.split("/").map(encodeURIComponent).join("/")}`;
  return `/a/${id}${v}${f}`;
}
