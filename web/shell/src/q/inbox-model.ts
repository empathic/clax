// The inbox's model (spec 2026-10-06-agent-questions-and-inbox §3.3, §9.2,
// §9.7): an item's title and one-line text, the search kept in `/inbox`'s
// URL, and a notification's text. Pure: no DOM.
import type { InboxFilter, InboxItem, InboxKind } from "../api";
import { agentLabel, cutHeader } from "./model";

export const INBOX_KINDS: readonly InboxKind[] = ["reply", "version", "published", "question", "finished"];

/** The longest notification body, in characters, "…" included. */
export const NOTIFY_BODY = 180;

const pageOf = (i: InboxItem): string => i.artifact?.title ?? "a deleted page";

/** An item's title: who did what, on which page ("claude replied on
 * Quarterly Review"). `others` are the items listed beside it, which
 * decide whether an agent is named by its harness alone. */
export function itemTitle(i: InboxItem, others: InboxItem[] = []): string {
  const who = agentLabel(i.agent, others.map(o => o.agent));
  switch (i.kind) {
    case "reply": return `${who} replied on ${pageOf(i)}`;
    case "version": {
      const n = i.version?.addressed.length ?? 0;
      const addressed = n ? ` · addressed ${n === 1 ? "1 of your threads" : `${n} of your threads`}` : "";
      return `${who} published v${i.version?.n ?? "?"} of ${pageOf(i)}${addressed}`;
    }
    case "published": return `${who} published ${pageOf(i)}`;
    case "question": {
      const headers = i.question?.questions.map(s => cutHeader(s.header)).join(", ");
      return headers ? `${who} asks: ${headers}` : `${who} asked a question`;
    }
    case "finished": return `${who} finished on ${pageOf(i)}`;
  }
}

/** An item's text on one line: the reply's body, the version's note, the
 * new artifact's description, the first question, or the finished work's
 * message; "" when it has none. */
export function itemText(i: InboxItem): string {
  const t = i.kind === "reply" ? i.reply?.body
    : i.kind === "version" ? i.version?.note
    : i.kind === "published" ? i.published?.description
    : i.kind === "question" ? i.question?.questions[0]?.question
    : i.work?.message;
  return (t ?? "").replace(/\s+/g, " ").trim();
}

const PARAMS = ["search", "kind", "artifact", "agent", "since", "until"] as const;

/** The search a `/inbox` URL's query string (`?search=…&kind=…`) keeps;
 * unknown kinds and empty values are dropped. */
export function filterFromUrl(search: string): InboxFilter {
  const u = new URLSearchParams(search);
  const f: InboxFilter = {};
  const get = (k: string) => u.get(k)?.trim() || undefined;
  const q = get("search");
  if (q) f.q = q;
  const kind = (get("kind") ?? "").split(",").filter((k): k is InboxKind => (INBOX_KINDS as string[]).includes(k));
  if (kind.length) f.kind = kind;
  for (const k of ["artifact", "agent", "since", "until"] as const) {
    const v = get(k);
    if (v) f[k] = v;
  }
  return f;
}

/** `f` as a `/inbox` query string ("" for no search), the inverse of
 * [`filterFromUrl`]; read state is not kept. */
export function filterToUrl(f: InboxFilter): string {
  const v: Record<(typeof PARAMS)[number], string | undefined> = {
    search: f.q?.trim() || undefined, kind: f.kind?.length ? f.kind.join(",") : undefined,
    artifact: f.artifact, agent: f.agent, since: f.since, until: f.until,
  };
  const parts = PARAMS.flatMap(k => (v[k] ? [`${k}=${k === "kind" ? v[k] : encodeURIComponent(v[k])}`] : []));
  return parts.length ? `?${parts.join("&")}` : "";
}

// C0 and C1 controls, the Arabic letter mark, LRM and RLM, the embeddings
// and overrides, and the isolates: none may steer a notification's text.
// oxlint-disable-next-line no-control-regex -- matching control characters is the point
const UNSAFE = /[\u0000-\u001f\u007f-\u009f\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/g;

const clean = (s: string): string => s.replace(/\s+/g, " ").replace(UNSAFE, "").replace(/ {2,}/g, " ").trim();

/** A notification's title and body for `i`: control and bidirectional
 * characters removed, the body cut to [`NOTIFY_BODY`] characters with "…". */
export function notificationText(i: InboxItem, others: InboxItem[] = []): { title: string; body: string } {
  const body = [...clean(itemText(i))];
  return {
    title: clean(itemTitle(i, others)),
    body: body.length > NOTIFY_BODY ? `${body.slice(0, NOTIFY_BODY - 1).join("").trimEnd()}…` : body.join(""),
  };
}
