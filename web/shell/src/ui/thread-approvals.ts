// The approval requests waiting for this viewer on an artifact's threads
// (spec 2026-10-08-multi-agent-multi-person §9.5, §17): the threads panel
// puts each above its thread, as the inbox shows it. Loaded only by a host
// that shows approval cards (the shell, not the extension's panel), once a
// thread there is withheld.
import type { InboxApproval, InboxItem } from "../api";

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
// once share it.
const inflight = new Map<string, Promise<Map<string, InboxItem>>>();

/** Thread `tid`'s open approval request for this viewer, or null. */
export function approvalFor(aid: string, tid: string): Promise<InboxItem | null> {
  let p = inflight.get(aid);
  if (!p) {
    p = approvalsFor(aid).finally(() => inflight.delete(aid));
    inflight.set(aid, p);
  }
  return p.then(m => m.get(tid) ?? null);
}
