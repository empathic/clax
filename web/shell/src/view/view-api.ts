// The artifact view's own requests, apart from api.ts so that the gallery,
// which shares api.ts with the artifact entry, carries none of them.
import type { SampleStatus } from "../api";
import type { DraftView } from "./draft-model";

/** The artifact's live drafts (hosted spec §16.4), each with its revision's token; none on failure. */
export async function getDrafts(id: string): Promise<DraftView[]> {
  try {
    const r = await fetch(`/api/artifacts/${id}/drafts`);
    const list = r.ok ? (await r.json()).drafts : null;
    return Array.isArray(list) ? list : [];
  } catch { return []; }
}

/** Records that this viewer looked at `ids`; answers the viewer's marks on `aid`, or null on failure (the next look writes again). */
export async function putLooked(aid: string, ids: string[]): Promise<Record<string, string> | null> {
  try {
    const r = await fetch("/api/viewers/me/looked", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ artifact_id: aid, thread_ids: ids }) });
    return r.ok ? (await r.json()).looked : null;
  } catch { return null; }
}

/** Raises this viewer's version seen mark on `aid` to `n`; a failure is ignored (the next load writes again). */
export async function putSeen(aid: string, n: number): Promise<void> {
  try {
    await fetch("/api/viewers/me/seen", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ artifact_id: aid, version: n }) });
  } catch { /* the next load writes again */ }
}

/** The artifact's sample status as the owner's browser sees it, or null when it cannot be read. */
export async function getSampleStatus(id: string, token: string): Promise<SampleStatus | null> {
  try {
    const res = await fetch(`/api/artifacts/${id}/sample`, { headers: { authorization: `Bearer ${token}` } });
    return res.ok ? ((await res.json()) as SampleStatus) : null;
  } catch {
    return null;
  }
}
