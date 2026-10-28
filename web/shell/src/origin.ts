/** The site hosts the shell frames each artifact on, as the daemon names
 * them (the bootstrap's and `GET /api/artifacts/<id>`'s `site`). In the Clax
 * app they are the app's content origins, which its core serves the sites
 * on: `clax` or `https`, under its content domain, with no port. */
export type SiteHosts = { domain: string; scheme: string; port: number | null; path?: string | null };

/** Where a shell on a trusted host frames each artifact when the daemon
 * says so (`site.path`, `[serve] trusted_host_frames`): a path of the
 * shell's own host, as `FRAME_PATH` in crates/clax-http/src/host.rs. */
export const FRAME_PATH = "/_clax/frame/";

/** The sandbox of a frame on a path, as `FRAME_SANDBOX` in
 * crates/clax-http/src/host.rs: no `allow-same-origin`, so the page's
 * origin is opaque. */
export const FRAME_SANDBOX = "allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-downloads allow-modals";

let pathFrames = false;

/** Whether this shell frames its artifacts on a path (`site.path`): their
 * runtime's site calls are then relayed by the shell (caps/site.ts). */
export function onPathFrames(): boolean {
  return pathFrames;
}

/** Whether `site` (an artifact's site origin, as `artifactOrigin` made it)
 * is a frame on a path. */
export function isPathFrame(site: string): boolean {
  return site.startsWith(FRAME_PATH);
}

/** The origin messages from and to the frame of `site` carry: the site's
 * own, or `"null"` for a frame on a path, whose sandbox makes its origin
 * opaque (the frame's window is what tells it apart). */
export function frameOrigin(site: string): string {
  return isPathFrame(site) ? "null" : site;
}

/** Artifact `id`'s site origin: `<scheme>://<id>.<domain>[:<port>]`; for a
 * frame on a path, that path of the shell's own host, `<path><id>`. */
export function artifactOrigin(id: string, site: SiteHosts): string {
  if (site.path) {
    pathFrames = true;
    return `${site.path}${id}`;
  }
  return `${site.scheme}://${id}.${site.domain}${site.port === null ? "" : `:${site.port}`}`;
}

/** A version's content root on the artifact's site origin `site`:
 * `/_clax/v/<n>/`. */
export function contentSrc(n: number, site: string): string {
  return `${site}/_clax/v/${n}/`;
}

/** Where the frame shows the page published at `file`, as `page_src` in
 * crates/clax-server/src/shell/boot.rs: the version's root for
 * `index.html`, else the file's path under it, each segment encoded. */
export function pageSrc(n: number, site: string, file: string): string {
  const root = contentSrc(n, site);
  return file === "index.html" ? root : root + file.split("/").map(encodeURIComponent).join("/");
}

/** The handshake that opens `target` (an address on the site origin `site`)
 * in the frame, as `start_src` in
 * crates/clax-server/src/shell/boot.rs: the site's `/_clax/session/start`,
 * with `target`'s path and fragment as `next`. */
export function startSrc(site: string, target: string): string {
  // A frame on a path has no handshake: the shell's own host knows the viewer.
  if (isPathFrame(site)) return target;
  // By prefix, not by `URL.origin`: a URL of the app's `clax:` origins has
  // no origin of its own.
  const rest = target.startsWith(site) ? target.slice(site.length) : "";
  const next = rest.startsWith("/") ? rest : "/";
  return `${site}/_clax/session/start?next=${encodeURIComponent(next)}`;
}
