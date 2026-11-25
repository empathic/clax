// Queue requests' routes: an agent's request for a report queue, which its
// approver approves or denies from the inbox or the Queues page (both load
// this module, so it stays apart from `queues/api.ts`). Every decision is
// the browser's own session (its cookie, never the token). A refusal
// throws `Refused` with the daemon's message; a decision held for the
// owner's confirmation throws it with `held`, and is made again with
// that confirmation's ID; a request no longer pending throws `Closed`,
// carrying the request as it stands.
import type { QueueRequest } from "../api";
import type { Queue } from "../../../sdk/src/types";
import { CONFIRMATION_HEADER, HELD, Refused, browserCall, heldFetch, heldOf } from "../held";
import { ApiError, readJson } from "../trouble";

export type { QueueRequest };
export { queueRequestLine } from "./queue-request-line";

/** A decision on a request that was decided, cancelled or expired before
 * it (409 `request_closed`): `request` is how it stands. */
export class Closed extends Refused {
  constructor(public request: QueueRequest, message: string) {
    super(409, message, "request_closed");
    this.name = "Closed";
  }
}

/** The approver's decision on a request. */
export type QueueDecision = "approve" | "deny";

/** What a decision answers: the request after it and, for an approval, the queue it made. */
export type Decided = { request: QueueRequest; queue?: Queue };

const e = encodeURIComponent;

/** The requests addressed to the caller, newest first: those in `status`, or all of them. */
export const listQueueRequests = (status?: QueueRequest["status"]) =>
  browserCall<{ requests: QueueRequest[] }>(`/api/queue-requests${status ? `?status=${e(status)}` : ""}`);

/** Approves or denies request `id`, presenting confirmation `confirm` when
 * the decision was held for one. */
export async function decideQueueRequest(id: string, d: QueueDecision, confirm?: string): Promise<Decided> {
  const headers: Record<string, string> = { "content-type": "application/json" };
  if (confirm) headers[CONFIRMATION_HEADER] = confirm;
  const res = await heldFetch(`/api/queue-requests/${e(id)}/${d}`, { method: "POST", headers, cache: "no-store", body: "{}" }, !!confirm);
  if (res.status === 409) {
    const b = await res.clone().json().catch(() => null) as { error?: { code?: unknown; message?: unknown; request?: QueueRequest } } | null;
    if (b?.error?.code === "request_closed" && b.error.request) throw new Closed(b.error.request, typeof b.error.message === "string" ? b.error.message : "");
  }
  const held = await heldOf(res);
  if (held) throw new Refused(202, held.message, HELD, held);
  try { return await readJson<Decided>(res); } catch (x) { throw x instanceof ApiError ? Refused.of(x) : x; }
}
