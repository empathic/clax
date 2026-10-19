// The inbox's model (spec 2026-10-06-agent-questions-and-inbox §3.3,
// §9.2): an item's title and one-line text. The search kept in `/inbox`'s
// URL is `inbox-filter.ts`, and a notification's text `notify.ts`, so the
// question module's chunk carries neither. Pure: no DOM.
import type { CardPerson, InboxApproval, InboxItem, QAgent } from "../api";
import { agentLabel, answerText, cutHeader } from "./model";
import { whose } from "../view/working-model";
import { plainText } from "../view/md-load.svelte";

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

/** An agent as every item names it: its name (else its harness), with its
 * person when it is not the reader's own person's (spec 2026-10-08 §11
 * "Display"): "sam-notes (Sam's agent)". */
function named(a: QAgent | null, others: InboxItem[]): string {
  const who = agentLabel(a, others.map(o => o.agent));
  return a?.own === false && a.person_name ? `${who} (${whose(a.person_name)} agent)` : who;
}

/** An item's title: who did what, on which page ("claude replied on
 * Quarterly Review"), or without the page under its page's heading
 * (`page: false`: "claude replied"). `others` are the items listed beside
 * it, which decide whether an agent is named by its harness alone. */
export function itemTitle(i: InboxItem, others: InboxItem[] = [], { page = true } = {}): string {
  const on = (pre: string) => (page ? `${pre}${pageOf(i)}` : "");
  // A reply that came from a remote is named by its author there.
  const who = i.kind === "reply" && !i.agent && i.linked?.author ? i.linked.author : named(i.agent, others);
  switch (i.kind) {
    case "reply": return `${who} replied${on(" on ")}`;
    case "version": {
      const n = i.version?.addressed.length ?? 0;
      const addressed = n ? ` · addressed ${n === 1 ? "1 of your threads" : `${n} of your threads`}` : "";
      return `${who} published v${i.version?.n ?? "?"}${on(" of ")}${addressed}`;
    }
    case "published": return `${who} published ${page ? pageOf(i) : "the page"}`;
    case "question": {
      const headers = i.question?.questions.map(s => cutHeader(s.header)).join(", ");
      // A withdrawn question stays unread (its person did not answer it),
      // but no longer asks anything.
      if (i.question?.status === "withdrawn") return headers ? `${who} withdrew its question: ${headers}` : `${who} withdrew a question`;
      return headers ? `${who} asks: ${headers}` : `${who} asked a question`;
    }
    case "finished": return `${who} finished${on(" on ")}`;
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
    case "push": return `${i.push?.origin ?? "A public Clax"} refused v${i.push?.n ?? "?"}${on(" of ")}`;
  }
}

/** An answered question's answers on one line: "Layout: Two · Notes: ok". */
const answersText = (q: NonNullable<InboxItem["question"]>): string | null =>
  q.answers && q.questions.map((s, n) => `${cutHeader(s.header)}: ${answerText(q, n)}`).join(" · ");

/** An item's text on one line: the reply's body as text (`plainText`), the version's note, the
 * new artifact's description, the answers to a question (else its first
 * question), what an approval request holds, a connecting machine's key,
 * the finished work's message, or what an ended agent's end cleaned up;
 * "" when it has none. */
export function itemText(i: InboxItem): string {
  const t = i.kind === "reply" ? (i.reply?.body ? plainText(i.reply.body) : null)
    : i.kind === "version" ? i.version?.note
    : i.kind === "published" ? i.published?.description
    : i.kind === "question" ? (i.question && answersText(i.question)) ?? i.question?.questions[0]?.question
    : i.kind === "approval" ? ((a => a && heldText(i, a))(approvalOf(i)))
    : i.kind === "connection" ? (i.connection?.client.key ? `Key ${i.connection.client.key}` : null)
    : i.kind === "finished" ? i.work?.message
    // What the end cleaned up: the note's line after "<name> ended".
    : i.kind === "ended" ? i.ended?.text.split(" · ").slice(1).join(" · ")
    : i.kind === "push" ? i.push?.message
    : null;
  return (t ?? "").replace(/\s+/g, " ").trim();
}
