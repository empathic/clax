// Shell URLs. `/a/<id>` shows an artifact's latest version, `/a/<id>/v/<n>`
// version n; either may be followed by `/<file>`, the published path of the
// page the frame shows (segments percent-encoded; none for `index.html`).
// After `/a/<id>`, a `v` segment followed by an all-digit segment is a
// version; anything else starts the file path. So a file published under
// `v/<digits>/` is reachable only through the versioned form.

export const INDEX = "index.html";
const ID = /^[0-9a-hj-km-np-tv-z]{12}$/;

export type ShellRoute =
  | { kind: "gallery" }
  | { kind: "artifact"; id: string; version: number | null; file: string };

/** The route a shell path names; anything that is not an artifact path is the gallery. */
export function parseShellPath(pathname: string): ShellRoute {
  const segs = pathname.split("/");
  if (segs[0] !== "" || segs[1] !== "a" || !ID.test(segs[2] ?? "")) return { kind: "gallery" };
  let rest = segs.slice(3);
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
 * the unversioned form would read as a version (`v/<digits>/…`) is given
 * `shown`, the version on screen. */
export function shellPath(id: string, version: number | null, file: string, shown?: number): string {
  if (version === null && /^v\/\d+(\/|$)/.test(file) && shown !== undefined) version = shown;
  const v = version === null ? "" : `/v/${version}`;
  const f = file === INDEX ? "" : `/${file.split("/").map(encodeURIComponent).join("/")}`;
  return `/a/${id}${v}${f}`;
}
