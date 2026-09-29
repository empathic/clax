// Links between the pages of one version. A plain click on such a link is
// handed to the shell, which records it as one history entry and moves the
// frame without adding another.

import { INDEX_FILE } from "./protocol";

/** The published path of the page of this version that `href` leads to, when
 * followed from the page at `pageUrl` published as `file`; null for a link to
 * the same page (a fragment), one with a query, or one outside the version. */
export function linkedPage(href: string, pageUrl: string, file: string): string | null {
  let target: URL;
  let page: URL;
  try { target = new URL(href, pageUrl); page = new URL(pageUrl); } catch { return null; }
  if (target.origin !== page.origin || target.search || target.pathname === page.pathname) return null;
  // The version's root: the page's path without the segments of its file.
  const segs = page.pathname.split("/");
  const root = file === INDEX_FILE ? page.pathname : `${segs.slice(0, segs.length - file.split("/").length).join("/")}/`;
  if (!root.endsWith("/") || !target.pathname.startsWith(root)) return null;
  try {
    const rel = target.pathname.slice(root.length).split("/").map(decodeURIComponent).join("/");
    return rel === "" ? INDEX_FILE : rel;
  } catch {
    return null;
  }
}
