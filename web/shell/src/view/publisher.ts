// The artifact view's line under the title. Its own module, so the gallery's
// and the artifact view's first-paint bundles share none of the gallery's model.
import type { Artifact } from "../api";

/** Who published the artifact: its owner session's harness, else the command
 * line; a live page by its URL without the scheme (`Live page · localhost:5173/settings`).
 * A fork without an owner session (made in a browser) names no publisher:
 * its line says where it came from ([`setBy`]). */
export function publisherText(a: Artifact): string {
  return a.live ? `Live page · ${a.live.page_url.replace(/^\w+:\/\//, "")}` : a.owner_harness ? `published by ${a.owner_harness}` : a.forked_from ? "" : "published from the command line";
}

/** Where a fork came from, as its line says it. */
export function lineageText(f: NonNullable<Artifact["forked_from"]>): string {
  return f.title === null ? "Forked from an artifact you cannot see" : `Forked from ${f.title} v${f.version}`;
}

/** Sets `el`, the line under the title, for `a` (null: none): the
 * publisher, and for a fork its source, linked to that version when the
 * viewer may read it (`.lineage`, which phone width keeps). Touches the DOM
 * only when the line changes. */
export function setBy(el: HTMLElement, a: Artifact | null): void {
  const pub = a ? publisherText(a) : "";
  const f = a?.forked_from;
  const key = f ? `${pub}\n${f.artifact_id}\n${f.version}\n${f.title}` : pub;
  if (el.dataset.k === key && (f || el.textContent === pub)) return;
  el.dataset.k = key;
  el.textContent = pub;
  if (!f) return;
  const doc = el.ownerDocument;
  if (pub) el.append(" · ");
  const l = doc.createElement(f.title === null ? "span" : "a");
  l.className = "lineage";
  l.textContent = lineageText(f);
  if (l instanceof HTMLAnchorElement) l.href = `/a/${encodeURIComponent(f.artifact_id)}/v/${f.version}`;
  el.append(l);
}
