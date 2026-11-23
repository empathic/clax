// The approval requests waiting for this viewer on an artifact's threads
// (spec 2026-10-08-multi-agent-multi-person §9.5, §17): the threads panel
// puts each above its thread, as the inbox shows it. Loaded only by a host
// that shows approval cards (the shell, not the extension's panel), once a
// thread there is withheld.
import type { InboxApproval, InboxItem } from "../api";
import { after } from "../clock";

/** The longest the thread list waits for this viewer's approval requests
 * before it shows (`approvalsDrawn`): a card that comes later goes above
 * its thread then. */
export const APPROVALS_WAIT_MS = 3000;

/** Thread ID → its open request's inbox item, among the caller's approval
 * items about artifact `aid`; none when the caller has no inbox. */
export async function approvalsFor(aid: string): Promise<Map<string, InboxItem>> {
  const out = new Map<string, InboxItem>();
  const r = await fetch(`/api/inbox?kind=approval&artifact=${encodeURIComponent(aid)}&limit=200`).catch(() => null);
  if (!r?.ok) return out;
  const b = (await r.json().catch(() => null)) as { items?: InboxItem[] } | null;
  for (const i of b?.items ?? []) {
    const a = i.approval as InboxApproval | undefined;
    if (a?.status === "open" && a.thread_id && !out.has(a.thread_id)) out.set(a.thread_id, i);
  }
  return out;
}

// One fetch per artifact while one is in flight: the threads withheld at
// once share it. The last answer for each artifact is kept, so a card
// drawn after it renders with it at once.
const inflight = new Map<string, Promise<Map<string, InboxItem>>>();
const last = new Map<string, Map<string, InboxItem>>();

/** This viewer's open approval requests on artifact `aid`'s threads, asked
 * once (a fetch in flight is shared), kept as the last answer. */
export function askApprovals(aid: string): Promise<Map<string, InboxItem>> {
  let p = inflight.get(aid);
  if (!p) {
    p = approvalsFor(aid).then(m => { last.set(aid, m); return m; }).finally(() => inflight.delete(aid));
    inflight.set(aid, p);
  }
  return p;
}

/** Thread `tid`'s open approval request for this viewer, or null. */
export function approvalFor(aid: string, tid: string): Promise<InboxItem | null> {
  return askApprovals(aid).then(m => m.get(tid) ?? null);
}

/** Thread `tid`'s open request as the last answer for `aid` holds it (null:
 * none), or undefined before any answer. */
export function approvalKnown(aid: string, tid: string): InboxItem | null | undefined {
  const m = last.get(aid);
  return m ? m.get(tid) ?? null : undefined;
}

/** Settles once this viewer's approval requests on artifact `aid` are in
 * (asked now, or failed), or after `APPROVALS_WAIT_MS`, whichever is first. */
export function approvalsDrawn(aid: string): Promise<void> {
  return new Promise(done => {
    const cap = after(APPROVALS_WAIT_MS, done);
    void askApprovals(aid).then(() => {}, () => {}).finally(() => { cap(); done(); });
  });
}
