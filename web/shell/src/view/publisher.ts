// The artifact view's line under the title. Its own module, so the gallery's
// and the artifact view's first-paint bundles share none of the gallery's model.
import type { AppInfo, Artifact, PersonCard } from "../api";

/** Who published the artifact: its owner session's agent name, else its harness, else the person
 * who started it from a brief, else the command line; a live page by its URL without the scheme
 * (`Live page · localhost:5173/settings`). A fork without an owner session (made in a browser)
 * names no publisher: its line says where it came from ([`setBy`]). */
export function publisherText(a: Artifact): string {
  const by = a.owner_name || a.owner_harness;
  if (a.live) return `Live page · ${a.live.page_url.replace(/^\w+:\/\//, "")}`;
  if (by) return `published by ${by}`;
  if (a.brief) return `started from a brief by ${a.brief.by}`;
  return a.forked_from ? "" : "published from the command line";
}

/** A person as the app header names them: `@login`, else their name. */
const handle = (p: PersonCard) => (p.login ? `@${p.login}` : p.name);

/** Whether two cards are the same person, as the Get in Clax page decides
 * it: the same name, login and public ID. */
const samePerson = (a: PersonCard, b: PersonCard) => a.name === b.name && a.login === b.login && (a.public_id ?? null) === (b.public_id ?? null);

/** An app's byline (spec 2026-10-21-app-push-pull §5.5, D7), the one the
 * Get in Clax page and the install and update sheets show: its newest
 * bundle's release, "v3 by @junp · by @akesling" when someone other than
 * the owner published that bundle, else "v3 · by @akesling". */
export function appByline(app: AppInfo): string {
  const owner = handle(app.owner);
  return samePerson(app.published_by, app.owner) ? `v${app.bundle_version} · by ${owner}` : `v${app.bundle_version} by ${handle(app.published_by)} · by ${owner}`;
}

/** An app's install count: "1 install", "12 installs". */
export const installsText = (n: number) => (n === 1 ? "1 install" : `${n} installs`);
/** An artifact's fork count: "1 fork", "4 forks". */
export const forksText = (n: number) => (n === 1 ? "1 fork" : `${n} forks`);

/** The line's text before any fork link: an app's byline and its installs
 * (when it has any), else the publisher; then the forks, when it has any. */
function lineText(a: Artifact): string {
  const parts = a.app ? [appByline(a.app)] : [publisherText(a)];
  if (a.app?.installs) parts.push(installsText(a.app.installs));
  if (a.forks) parts.push(forksText(a.forks));
  return parts.filter(Boolean).join(" · ");
}

/** Where a fork came from, as its line says it. */
export function lineageText(f: NonNullable<Artifact["forked_from"]>): string {
  return f.title === null ? "Forked from an artifact you cannot see" : `Forked from ${f.title} v${f.version}`;
}

/** Sets `el`, the line under the title, for `a` (null: none): the
 * publisher (for an app, its byline and installs), its forks' count, and for a fork its source, linked to that version when the
 * viewer may read it (`.lineage`, which phone width keeps). An app's line
 * is marked `.app`, which phone width keeps whole. Touches the DOM only
 * when the line changes. */
export function setBy(el: HTMLElement, a: Artifact | null): void {
  el.classList.toggle("app", !!a?.app);
  const pub = a ? lineText(a) : "";
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
