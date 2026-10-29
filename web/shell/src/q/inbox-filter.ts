// The search `/inbox` keeps in its URL (spec
// 2026-10-06-agent-questions-and-inbox §9.2): the kinds the inbox page
// filters by, and the query string to and from a filter. Only the inbox
// page loads it, so the question module's chunk carries none of it. Pure:
// no DOM.
import type { InboxFilter, InboxKind } from "../api";
import { tagsFromUrl, withTags } from "../view/tags-model";

/** The kinds the inbox page filters by. */
export const INBOX_KINDS: readonly InboxKind[] = ["reply", "version", "published", "question", "finished", "approval", "connection", "ended", "push"];
/** The kinds the Activity filter picks: notices that wait on no one. */
export const ACTIVITY_KINDS: readonly InboxKind[] = ["version", "published", "finished", "ended", "push"];

const PARAMS = ["search", "unread", "kind", "artifact", "agent", "since", "until"] as const;

/** The search a `/inbox` URL's query string (`?search=…&kind=…`) keeps,
 * `unread=1` the Unread filter (`read: "unread"`), each `tag` a tag the
 * items' artifacts carry (all of them); unknown kinds and empty values are
 * dropped. */
export function filterFromUrl(search: string): InboxFilter {
  const u = new URLSearchParams(search);
  const f: InboxFilter = {};
  const get = (k: string) => u.get(k)?.trim() || undefined;
  const q = get("search");
  if (q) f.q = q;
  if (get("unread") === "1") f.read = "unread";
  const kind = (get("kind") ?? "").split(",").filter((k): k is InboxKind => (INBOX_KINDS as string[]).includes(k));
  if (kind.length) f.kind = kind;
  for (const k of ["artifact", "agent", "since", "until"] as const) {
    const v = get(k);
    if (v) f[k] = v;
  }
  const tag = tagsFromUrl(search);
  if (tag.length) f.tag = tag;
  return f;
}

/** `f` as a `/inbox` query string ("" for no search), the inverse of
 * [`filterFromUrl`]: of the read states, only the Unread filter is kept. */
export function filterToUrl(f: InboxFilter): string {
  const v: Record<(typeof PARAMS)[number], string | undefined> = {
    search: f.q?.trim() || undefined, unread: f.read === "unread" ? "1" : undefined, kind: f.kind?.length ? f.kind.join(",") : undefined,
    artifact: f.artifact, agent: f.agent, since: f.since, until: f.until,
  };
  const parts = PARAMS.flatMap(k => (v[k] ? [`${k}=${k === "kind" ? v[k] : encodeURIComponent(v[k])}`] : []));
  const tags = withTags("", f.tag ?? []).slice(1);
  if (tags) parts.push(tags);
  return parts.length ? `?${parts.join("&")}` : "";
}
