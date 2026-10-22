/** The site hosts the shell frames each artifact on, as the daemon names
 * them (the bootstrap's and `GET /api/artifacts/<id>`'s `site`). In the Clax
 * app they are the app's content origins, which its core serves the sites
 * on: `clax` or `https`, under its content domain, with no port. */
export type SiteHosts = { domain: string; scheme: string; port: number | null };

/** Artifact `id`'s site origin: `<scheme>://<id>.<domain>[:<port>]`. */
export function artifactOrigin(id: string, site: SiteHosts): string {
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
  // By prefix, not by `URL.origin`: a URL of the app's `clax:` origins has
  // no origin of its own.
  const rest = target.startsWith(site) ? target.slice(site.length) : "";
  const next = rest.startsWith("/") ? rest : "/";
  return `${site}/_clax/session/start?next=${encodeURIComponent(next)}`;
}
