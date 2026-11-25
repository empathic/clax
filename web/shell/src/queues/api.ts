// The report queues' admin and triage routes (spec 2026-10-27-clax-sdk-design
// §5.3, §6.2; 2026-10-29-feedback-loop-design §6, §8) for the Queues page
// and its queue and report views, which alone load this module. Every
// change is the browser's own session (`SameOrigin`), never the token. A
// refusal throws `Refused` with the daemon's message and code.
import type { QueueRequest } from "../api";
import type { DiagLine, Queue, QueueRole, Report, ReportPage, ReportState, ReportersSee, ResolveReason, ServiceGrantCreated } from "../../../sdk/src/types";

/** What a listing of reports filters by: a state, or `active` (`open` or `claimed`). */
export type StateFilter = ReportState | "active";

/** A route's refusal: its status, the daemon's message and code. */
export class Refused extends Error {
  constructor(public status: number, message: string, public code: string | null) {
    super(message);
    this.name = "Refused";
  }
}

export type { DiagLine, Queue, QueueRole, Report, ReportState, ReportersSee, ResolveReason, ServiceGrantCreated };
/** What a queue's form sets. */
export type QueueFields = { name: string; origins: string[]; reporters_see: ReportersSee };
/** A queue's member, by public ID, with the name the daemon knows them by:
 * its maker (always admin) first, and the owner marked. */
export type QueueMember = { public_id: string; name?: string | null; role: QueueRole; maker?: boolean; owner?: boolean };
/** Someone a queue's admin may add, as the people search finds them. */
export type PersonMatch = { public_id: string; name?: string | null; avatar_url?: string | null; owner?: boolean };
/** Whether members who administer nothing make report queues, and where that comes from (`default`, `db`). */
export type QueuePolicy = { members_create: boolean; source: string };
/** What this browser may do with queues (`GET /api/viewers/me`'s `queues`). */
export type QueueAccess = { member: number; may_make: boolean };
/** A service grant as listed: never its token or key. */
export type ServiceGrant = ServiceGrantCreated["grant"];

async function call<T>(path: string, method = "GET", body?: unknown): Promise<T> {
  const res = await fetch(path, method === "GET" ? undefined : { method, ...body !== undefined && { headers: { "content-type": "application/json" }, body: JSON.stringify(body) } });
  if (res.ok) return (res.status === 204 ? {} : await res.json().catch(() => ({}))) as T;
  let b: { error?: { code?: string; message?: string } } = {};
  try { b = await res.json(); } catch { /* not json */ }
  throw new Refused(res.status, b.error?.message ?? "", b.error?.code ?? null);
}
const e = encodeURIComponent;
const q = (qid: string) => `/api/queues/${e(qid)}`;

/** The pending queue requests the viewer answers (`GET /api/queue-requests?status=pending`),
 * here so the Queues page lists them without loading the request card's module. */
export const listPendingQueueRequests = () => call<{ requests: QueueRequest[] }>("/api/queue-requests?status=pending");
/** The queues the viewer made or is a member of, at their role. */
export const listQueues = () => call<{ queues: Queue[] }>("/api/queues");
/** What this browser may do with queues; none known reads as none. */
export const queueAccess = () => call<{ queues?: Partial<QueueAccess> }>("/api/viewers/me").then(b => ({ member: b.queues?.member ?? 0, may_make: b.queues?.may_make === true }));
/** The people a queue's admin may add whose name holds `q`, or whose public ID is `q`. */
export const searchPeople = (qid: string, text: string) => call<{ people: PersonMatch[] }>(`${q(qid)}/people?q=${e(text)}`);
export const getQueuePolicy = () => call<QueuePolicy>("/api/admin/queues");
/** Turns members' queue making on or off; null puts the default back. */
export const putQueuePolicy = (members_create: boolean | null) => call<QueuePolicy>("/api/admin/queues", "PUT", { members_create });
export const createQueue = (f: QueueFields) => call<{ queue: Queue }>("/api/queues", "POST", f);
export const updateQueue = (qid: string, f: Partial<QueueFields>) => call<{ queue: Queue }>(q(qid), "PATCH", f);
export const deleteQueue = (qid: string) => call<unknown>(q(qid), "DELETE");
/** A page of the queue's reports from `cursor`: those `state` passes (a
 * state or `active`; every state without it), in order of arrival or,
 * with `order` `newest`, the latest filed first. */
export function reportPage(qid: string, cursor: string | null, o: { state?: StateFilter | null; order?: "newest" } = {}) {
  const query = [o.state && `state=${e(o.state)}`, o.order && `order=${o.order}`, cursor && `cursor=${e(cursor)}`].filter(Boolean).join("&");
  return call<ReportPage>(`${q(qid)}/reports${query ? `?${query}` : ""}`);
}
const r = (qid: string, tid: string) => `${q(qid)}/reports/${e(tid)}`;
/** One report of the queue, with its context, reporter profile, conversation and fixes. */
export const getReport = (qid: string, tid: string) => call<{ report: Report }>(r(qid, tid));
/** Where the filing's clip is served. */
export const clipUrl = (qid: string, tid: string) => `${r(qid, tid)}/clip`;
/** Where a reopen's clip, on its comment `cid`, is served. */
export const commentClipUrl = (qid: string, tid: string, cid: string) => `${r(qid, tid)}/comments/${e(cid)}/clip`;
/** The diagnostics of capture `capture` (0 the filing's, `n` the `n`th
 * reopen's), or without it of the latest capture that has any. */
export const diagnostics = (qid: string, tid: string, capture?: number) =>
  call<{ lines: DiagLine[] }>(`${r(qid, tid)}/diagnostics${capture === undefined ? "" : `?capture=${capture}`}`);
/** Replies on the report as the viewer, a comment its reporter sees. */
export const reply = (qid: string, tid: string, body: string) => call<{ report: Report }>(`${r(qid, tid)}/comments`, "POST", { body });
/** What a proposed fix says; `summary` is required. */
export type FixFields = { summary: string; url?: string; commit?: string; release?: string; verify_url?: string; verify_after?: string };
/** Proposes a fix: the report waits for the reporter's verdict. */
export const proposeFix = (qid: string, tid: string, f: FixFields) => call<{ report: Report }>(`${r(qid, tid)}/fix`, "POST", f);
/** Closes the report without a verdict, for `reason`. */
export const resolve = (qid: string, tid: string, reason: ResolveReason) => call<{ report: Report }>(`${r(qid, tid)}/resolve`, "POST", { reason });
/** Ends the report's claim, whoever holds it: the holder is told, and
 * `note` (when given) is a note for the queue's members alone.
 * `released` is false when no live claim was left to end (it had lapsed
 * or been released already): nothing was written then. */
export const releaseClaim = (qid: string, tid: string, note?: string) => call<{ report: Report; released?: boolean }>(`${r(qid, tid)}/release`, "POST", note ? { note } : {});
/** Says, on the reporter's behalf, that the fix waiting works. */
export const verify = (qid: string, tid: string, note?: string) => call<{ report: Report }>(`${r(qid, tid)}/verify`, "POST", note ? { note } : {});
/** Says, on the reporter's behalf, that the report is still broken, with `body` as their comment. */
export const reopen = (qid: string, tid: string, body: string) => call<{ report: Report }>(`${r(qid, tid)}/reopen`, "POST", { body });
export const listMembers = (qid: string) => call<{ members: QueueMember[] }>(`${q(qid)}/members`);
/** Adds person `pid` to the queue at `role`, or changes their role. */
export const setMember = (qid: string, pid: string, role: QueueRole) => call<unknown>(`${q(qid)}/members/${e(pid)}`, "PUT", { role });
export const removeMember = (qid: string, pid: string) => call<unknown>(`${q(qid)}/members/${e(pid)}`, "DELETE");
export const listGrants = (qid: string) => call<{ grants: ServiceGrant[] }>(`${q(qid)}/service-grants`);
/** Makes a service grant; its token and private key are in this answer
 * alone. `origins`, some of the queue's origins and patterns exactly as it
 * lists them, limits the origins it mints reporter tokens for (such as the
 * preview pattern alone); without them it mints for every one. */
export const createGrant = (qid: string, name: string, origins?: string[]) => call<ServiceGrantCreated>(`${q(qid)}/service-grants`, "POST", origins ? { name, origins } : { name });
/** Revokes a service grant, which ends every reporter token it minted. */
export const revokeGrant = (qid: string, gid: string) => call<unknown>(`${q(qid)}/service-grants/${e(gid)}`, "DELETE");
