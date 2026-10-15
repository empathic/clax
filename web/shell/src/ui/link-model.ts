// How the Publish popover words a link's state (linked threads).

/** When a remote became unreachable: the time today, else the date and time. */
export function sinceLabel(at: string, now: Date = new Date()): string {
  const d = new Date(at);
  if (Number.isNaN(d.getTime())) return at;
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return d.toDateString() === now.toDateString() ? time : `${d.toLocaleDateString([], { month: "short", day: "numeric" })}, ${time}`;
}

/** What a link row's "Not sent" line counts: its comments and questions
 * not sent, as "1 comment", "2 questions" or "1 comment and 1 question". */
export function notSentLabel(comments: number, questions: number): string {
  const n = (k: number, one: string) => `${k} ${one}${k === 1 ? "" : "s"}`;
  const parts = [comments > 0 ? n(comments, "comment") : "", questions > 0 ? n(questions, "question") : ""].filter(Boolean);
  return parts.join(" and ");
}
