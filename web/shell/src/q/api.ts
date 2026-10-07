// The owner routes for questions and the inbox (spec
// 2026-10-06-agent-questions-and-inbox §6.2, §8.1). They load with the
// question module, after the first paint, so the eager entries carry none of
// this. A 403 (the caller is not the owner) resolves to "forbidden"; a 409
// on an answer, a skip or a move resolves to the question as it closed;
// any other failure throws an `ApiError`.
import { ApiError, type AnswerBody, type InboxFilter, type InboxItem, type InboxPage, type InboxSummary, type QuestionView } from "../api";

export type Forbidden = "forbidden";
/** The outcome of answering, skipping or moving a question: the question
 * after it, or, when something closed it first (409), the question as it is. */
export type Closing = { question: QuestionView } | { closed: QuestionView };

const JSON_BODY = { "content-type": "application/json" };

async function call<T>(path: string, init?: RequestInit, closed = false): Promise<T | Forbidden | { closed: QuestionView }> {
  const res = await fetch(path, init);
  if (res.ok) return (await res.json()) as T;
  if (res.status === 403) return "forbidden";
  let body: { error?: { code?: string; message?: string }; question?: QuestionView } = {};
  try { body = await res.json(); } catch { /* not json */ }
  if (closed && res.status === 409 && body.question) return { closed: body.question };
  throw new ApiError(res.status, body.error?.message ?? res.statusText, body.error?.code ?? null);
}
const get = <T>(path: string) => call<T>(path) as Promise<T | Forbidden>;
const post = <T>(path: string, body?: unknown) =>
  call<T>(path, { method: "POST", headers: JSON_BODY, body: JSON.stringify(body ?? {}) }) as Promise<T | Forbidden>;

/** Questions by status: open ones oldest first, closed ones most recently closed first. */
export const listQuestions = (status: "open" | "closed" | "all" = "open", limit = 50) =>
  get<{ questions: QuestionView[]; open: number }>(`/api/questions?status=${status}&limit=${limit}`);

const closing = (qid: string, verb: string, body?: unknown) =>
  call<{ question: QuestionView }>(`/api/questions/${encodeURIComponent(qid)}/${verb}`,
    { method: "POST", headers: JSON_BODY, body: JSON.stringify(body ?? {}) }, true) as Promise<Closing | Forbidden>;

export const answerQuestion = (qid: string, body: AnswerBody) => closing(qid, "answer", body);
export const declineQuestion = (qid: string) => closing(qid, "decline");
/** "Answer in the terminal" (mirrored questions only). */
export const releaseQuestion = (qid: string) => closing(qid, "release");

/** `f` as `GET /api/inbox` query parameters. */
export function inboxQuery(f: InboxFilter, before?: string | null, limit?: number): string {
  const u = new URLSearchParams();
  if (f.q?.trim()) u.set("q", f.q.trim());
  if (f.kind?.length) u.set("kind", f.kind.join(","));
  for (const k of ["artifact", "agent", "since", "until", "read"] as const) {
    const v = f[k];
    if (v) u.set(k, v);
  }
  if (before) u.set("before", before);
  if (limit) u.set("limit", String(limit));
  const s = u.toString();
  return s ? `?${s}` : "";
}

/** One page of items matching `f`, newest first; `before` is the previous page's `next_cursor`. */
export const listInbox = (f: InboxFilter, before?: string | null, limit?: number) => get<InboxPage>(`/api/inbox${inboxQuery(f, before, limit)}`);
export const inboxSummary = () => get<InboxSummary>("/api/inbox/summary");
/** Marks one item read or unread. */
export const markInbox = (id: string, read: boolean) =>
  post<{ item: InboxItem; unread: number }>(`/api/inbox/${encodeURIComponent(id)}/${read ? "read" : "unread"}`);
/** Marks items read: these IDs, or every unread item matching `filter` up
 * to `upto` (the newest `seq` shown, so an item made since stays unread). */
export function markInboxMany(m: { ids: string[] } | { all: true; filter?: InboxFilter; upto?: number }) {
  if ("ids" in m) return post<{ marked: number; unread: number }>("/api/inbox/read", { ids: m.ids });
  const f = m.filter ?? {};
  const filter = Object.fromEntries(Object.entries({ q: f.q?.trim() || undefined, kind: f.kind?.length ? f.kind : undefined, artifact: f.artifact, agent: f.agent, since: f.since, until: f.until }).filter(([, v]) => v !== undefined));
  return post<{ marked: number; unread: number }>("/api/inbox/read", { all: true, ...Object.keys(filter).length && { filter }, ...m.upto !== undefined && { upto: m.upto } });
}
