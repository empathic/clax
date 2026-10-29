// The session question routes and the owner's thread routes, for the
// question and inbox end-to-end tests (spec 2026-10-06-agent-questions-and-inbox
// §6.1, §7.1). Each call carries the daemon's token; the owner's are made as a
// browser of the owner's (its owner cookie and the shell's Origin).

import { api, ownerSessionOf } from "./fixtures";

export type QuestionView = {
  id: string;
  status: string;
  answers: { selected: string[]; text: string | null }[] | null;
  answered_via: string | null;
};

/** One question of an ask: no options is a free-text question. */
export type QuestionSpec = {
  question: string;
  header: string;
  options?: { label: string; description?: string; preview?: string }[];
  multi_select?: boolean;
  other?: boolean;
};

/** `POST /api/sessions/<sid>/questions` → `{question, mode, …}`: about
 * `artifact_id`, in its thread `thread_id`, or asked in the terminal
 * (`in_terminal`). */
export async function ask(base: string, token: string, sid: string, body: { source: "ask" | "hook"; artifact_id?: string; thread_id?: string; in_terminal?: boolean; questions: QuestionSpec[] }) {
  const questions = body.questions.map(q => ({ multi_select: false, other: false, options: [], ...q }));
  return api(base, token, `/api/sessions/${sid}/questions`, { method: "POST", body: JSON.stringify({ ...body, questions }) }) as Promise<{ question: QuestionView; mode: string }>;
}

/** The question once it is no longer open, from the session's long poll (up to 10 s). */
export async function waitAnswer(base: string, token: string, sid: string, qid: string): Promise<QuestionView> {
  return ((await api(base, token, `/api/sessions/${sid}/questions/${qid}?wait=10`)) as { question: QuestionView }).question;
}

/** The owner cookie a browser of the owner's carries: the owner browser
 * session the worker's browsers hold on the daemon at `base`
 * (crates/clax-http/src/local_trust.rs). `_token` is the daemon's, which
 * names no cookie. */
export function ownerCookie(base: string, _token?: string): string {
  const port = new URL(base).port || "80";
  return `clax_owner_${port}=${ownerSessionOf(base)}`;
}

/** A thread the owner opens on `aid` v1 (on the `h2` of its page `file`) and sends to the agent; returns its ID. */
export async function threadAsOwner(base: string, token: string, aid: string, body: string, file = "index.html"): Promise<string> {
  const headers = { cookie: ownerCookie(base, token), origin: base };
  const form = new FormData();
  form.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file }));
  form.set("body", body);
  form.set("version", "1");
  const res = await fetch(`${base}/api/artifacts/${aid}/threads`, { method: "POST", body: form, headers });
  if (!res.ok) throw new Error(`thread: ${res.status} ${await res.text()}`);
  const tid = ((await res.json()) as { thread: { id: string } }).thread.id;
  const sent = await fetch(`${base}/api/artifacts/${aid}/threads/${tid}/send`, { method: "POST", headers });
  if (!sent.ok) throw new Error(`send: ${sent.status} ${await sent.text()}`);
  return tid;
}

/** Session `sid`'s reply on thread `tid` of `aid`. */
export async function replyAsAgent(base: string, token: string, sid: string, aid: string, tid: string, body: string) {
  return api(base, token, `/api/artifacts/${aid}/threads/${tid}/comments`, { method: "POST", session: sid, body: JSON.stringify({ body, author_kind: "agent" }) });
}
