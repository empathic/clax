// The inbox's model (spec 2026-10-06-agent-questions-and-inbox §3.3, §9.2,
// §9.7): an item's title and one-line text, the search kept in `/inbox`'s
// URL, and a notification's text. Pure: no DOM.
import type { CardPerson, InboxApproval, InboxFilter, InboxItem, InboxKind, QAgent } from "../api";
import { agentLabel, answerText, cutHeader } from "./model";

/** The kinds the inbox page filters by. */
export const INBOX_KINDS: readonly InboxKind[] = ["reply", "version", "published", "question", "finished", "approval", "connection", "ended"];

/** The longest notification body, in characters, "…" included. */
export const NOTIFY_BODY = 180;

/** The newest `seq` of `items`, the `upto` of a mark-all over what was
 * shown; 0 when none was shown, which marks nothing. */
export const newestSeq = (items: readonly InboxItem[]): number => items.reduce((n, i) => Math.max(n, i.seq), 0);

const pageOf = (i: InboxItem): string => i.artifact?.title ?? "a deleted page";

/** A person as an approval names them, the owner marked as such (the
 * daemon names an unnamed owner "The owner"). */
export const personName = (a: Pick<CardPerson, "name" | "owner">): string => (a.owner && a.name !== "The owner" ? `${a.name} (the owner)` : a.name);

/** An `approval` item's card, or null once its request is gone. */
export const approvalOf = (i: InboxItem): InboxApproval | null => (i.approval && "status" in i.approval ? i.approval : null);

/** What an approval request holds, on one line: "1 comment on Quarterly
 * Review «Quarterly goals»"; a page whose title is unknown (deleted) is
 * "one page". */
export function heldText(i: InboxItem, a: InboxApproval): string {
  const anchor = heldAnchor(i, a);
  return `${heldCount(a)} on ${i.artifact?.title ?? "one page"}${anchor ? ` ${anchor}` : ""}`;
}

/** How much a request holds: "1 comment", "3 items" (outside threads). */
export const heldCount = (a: InboxApproval): string =>
  `${a.count} ${a.thread_id ? (a.count === 1 ? "comment" : "comments") : a.count === 1 ? "item" : "items"}`;

/** The quoted anchor that ends a request's thread summary ("«Quarterly
 * goals»"), if any. */
export const heldAnchor = (i: InboxItem, a: InboxApproval): string | undefined =>
  (a.thread_id ? /«[^»]*»\s*$/.exec(i.thread?.summary ?? "")?.[0]?.trim() : undefined);

/** A reply's author line (spec 2026-10-08 §11 "Display"): `<name> (agent,
 * via <harness>)` for the recipient's own person's agent, else `<name>
 * (<person>'s agent, via <harness>)` (an inbox item's agent with `own`
 * always has `person_name`); null for an agent with no name. */
function replier(a: QAgent | null): string | null {
  if (!a?.name || a.own === undefined) return null;
  return `${a.name} (${a.own ? "" : `${a.person_name}'s `}agent, via ${a.harness})`;
}

/** An item's title: who did what, on which page ("claude replied on
 * Quarterly Review"). `others` are the items listed beside it, which
 * decide whether an agent is named by its harness alone. */
export function itemTitle(i: InboxItem, others: InboxItem[] = []): string {
  const who = agentLabel(i.agent, others.map(o => o.agent));
  switch (i.kind) {
    case "reply": return `${replier(i.agent) ?? who} replied on ${pageOf(i)}`;
    case "version": {
      const n = i.version?.addressed.length ?? 0;
      const addressed = n ? ` · addressed ${n === 1 ? "1 of your threads" : `${n} of your threads`}` : "";
      return `${who} published v${i.version?.n ?? "?"} of ${pageOf(i)}${addressed}`;
    }
    case "published": return `${who} published ${pageOf(i)}`;
    case "question": {
      const headers = i.question?.questions.map(s => cutHeader(s.header)).join(", ");
      // A withdrawn question stays unread (its person did not answer it),
      // but no longer asks anything.
      if (i.question?.status === "withdrawn") return headers ? `${who} withdrew its question: ${headers}` : `${who} withdrew a question`;
      return headers ? `${who} asks: ${headers}` : `${who} asked a question`;
    }
    case "finished": return `${who} finished on ${pageOf(i)}`;
    case "connection": {
      const c = i.connection;
      if (!c) return "A connection request";
      return c.via === "invite" && c.person
        ? `${c.person.display_name ?? "Someone"} connected from ${c.client.name}`
        : `Connection request from ${c.client.name} (${c.client.peer_ip})`;
    }
    case "approval": {
      const a = approvalOf(i);
      if (!a) return "An approval request";
      return `${personName(a.author)} wants to reach your ${a.agent ? `agent ${a.agent.name}` : "agents"}`;
    }
    case "ended": return `${who} ended`;
  }
}

/** An answered question's answers on one line: "Layout: Two · Notes: ok". */
const answersText = (q: NonNullable<InboxItem["question"]>): string | null =>
  q.answers && q.questions.map((s, n) => `${cutHeader(s.header)}: ${answerText(q, n)}`).join(" · ");

/** An item's text on one line: the reply's body, the version's note, the
 * new artifact's description, the answers to a question (else its first
 * question), what an approval request holds, a connecting machine's key,
 * the finished work's message, or what an ended agent's end cleaned up;
 * "" when it has none. */
export function itemText(i: InboxItem): string {
  const t = i.kind === "reply" ? i.reply?.body
    : i.kind === "version" ? i.version?.note
    : i.kind === "published" ? i.published?.description
    : i.kind === "question" ? (i.question && answersText(i.question)) ?? i.question?.questions[0]?.question
    : i.kind === "approval" ? ((a => a && heldText(i, a))(approvalOf(i)))
    : i.kind === "connection" ? (i.connection?.client.key ? `Key ${i.connection.client.key}` : null)
    : i.kind === "finished" ? i.work?.message
    // What the end cleaned up: the note's line after "<name> ended".
    : i.kind === "ended" ? i.ended?.text.split(" · ").slice(1).join(" · ")
    : null;
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
