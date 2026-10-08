// Links between the pages of one version. A plain click on such a link is
// handed to the shell, which records it as one history entry and moves the
// frame without adding another.

import { INDEX_FILE } from "./protocol";

/** A page of the version a link leads to, and the link's fragment (`#…`, or ""). */
export interface PageLink { file: string; hash: string }

/** Where a link inside a page of the version leads, when the bridge handles it. */
export type LinkTarget =
  /** Another page of the version: handed to the shell. */
  | { kind: "page"; file: string; hash: string }
  /** This page under another spelling of its path (`index.html` for `/v/<n>/`):
   * followed in place, so it adds no history entry of its own. */
  | { kind: "self"; hash: string };

/** The version's page a link resolves to (`index.html` for the version's root),
 * with its fragment, or null outside the version or with a query. */
function resolve(href: string, pageUrl: string, file: string): { file: string; hash: string; respelled: boolean } | null {
  let target: URL;
  let page: URL;
  try { target = new URL(href, pageUrl); page = new URL(pageUrl); } catch { return null; }
  if (target.origin !== page.origin || target.search) return null;
  // The version's root: the page's path without the segments of its file.
  const segs = page.pathname.split("/");
  const root = file === INDEX_FILE ? page.pathname : `${segs.slice(0, segs.length - file.split("/").length).join("/")}/`;
  if (!root.endsWith("/") || !target.pathname.startsWith(root)) return null;
  let rel: string;
  try { rel = target.pathname.slice(root.length).split("/").map(decodeURIComponent).join("/"); } catch { return null; }
  return { file: rel === "" ? INDEX_FILE : rel, hash: target.hash, respelled: target.pathname !== page.pathname };
}

/** The page of this version that `href` leads to, when followed from the page
 * at `pageUrl` published as `file`; null for a link to the same page in any
 * spelling (the browser handles it and its fragment), one with a query, or
 * one outside the version (another origin, version, or artifact, or a
 * `javascript:`, `data:`, `blob:` or `mailto:` URL). */
export function linkedPage(href: string, pageUrl: string, file: string): PageLink | null {
  const r = resolve(href, pageUrl, file);
  return r && r.file !== file ? { file: r.file, hash: r.hash } : null;
}

/** What the bridge does with a click, or null when the browser should follow
 * it: the bridge must be welcomed; the click must be a plain, primary one the
 * page did not cancel; the link must not download, open in another browsing
 * context (its `target`, or the document's `<base target>`), or be marked
 * `rel="external"`; and it must lead to another page of the version (handed
 * to the shell) or to this page under another spelling of its path (followed
 * in place). A link to this page's own path, a fragment included, stays with
 * the browser. */
export function linkToHandOver(e: MouseEvent, o: { welcomed: boolean; pageUrl: string; file: string }): LinkTarget | null {
  if (!o.welcomed || e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return null;
  const a = (e.target as Element | null)?.closest?.("a[href]");
  if (!a || a.localName !== "a" || !("href" in a) || typeof a.href !== "string") return null;
  const link = a as HTMLAnchorElement;
  if (link.hasAttribute("download") || link.relList.contains("external")) return null;
  const target = link.getAttribute("target") || link.ownerDocument.querySelector("base[target]")?.getAttribute("target") || "";
  if (target !== "" && target.toLowerCase() !== "_self") return null;
  const r = resolve(link.href, o.pageUrl, o.file);
  if (!r) return null;
  if (r.file !== o.file) return { kind: "page", file: r.file, hash: r.hash };
  return r.respelled ? { kind: "self", hash: r.hash } : null;
}

/** The parts of `Location` that following a link in place uses. */
export type PlaceLocation = Pick<Location, "hash" | "href" | "replace">;

/** Follows a link to this page (under another spelling of its path) in place,
 * as a link to the page's own URL would be followed: to a new fragment, a
 * fragment navigation (one history entry); to the current fragment, that
 * fragment again, scrolling to its target (no entry); with no fragment, a
 * load of the page's URL without one in place of this entry (at the top of
 * the page). */
export function followInPlace(hash: string, loc: PlaceLocation): void {
  // Absolute: a URL relative to the page's would resolve against its
  // <base href>, which may name another site.
  const here = loc.href.split("#")[0];
  if (!hash) loc.replace(here);
  else if (hash === loc.hash) loc.href = here + hash;
  else loc.hash = hash;
}
