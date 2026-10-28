// The report queues' admin routes (spec 2026-10-27-clax-sdk-design §5.3,
// §6.2) for the Queues page, which alone loads this module. Every change is
// the browser's own session (`SameOrigin`), never the token. A refusal
// throws `Refused` with the daemon's message and code.
import type { Queue, QueueRole, ReportPage, ReportersSee, ServiceGrantCreated } from "../../../sdk/src/types";

/** A route's refusal: its status, the daemon's message and code. */
export class Refused extends Error {
  constructor(public status: number, message: string, public code: string | null) {
    super(message);
    this.name = "Refused";
  }
}

export type { Queue, QueueRole, ReportersSee, ServiceGrantCreated };
/** What a queue's form sets. */
export type QueueFields = { name: string; origins: string[]; reporters_see: ReportersSee };
/** A queue's member, by public ID, with the name the daemon knows them by. */
export type QueueMember = { public_id: string; display_name?: string | null; role: QueueRole };
/** A service grant as listed: never its token or key. */
export type ServiceGrant = ServiceGrantCreated["grant"];

async function call<T>(path: string, method = "GET", body?: unknown): Promise<T> {
  const res = await fetch(path, method === "GET" ? undefined : { method, ...body !== undefined && { headers: { "content-type": "application/json" }, body: JSON.stringify(body) } });
  if (res.ok) return (res.status === 204 ? {} : await res.json().catch(() => ({}))) as T;
  let b: { error?: { code?: string; message?: string } } = {};
  try { b = await res.json(); } catch { /* not json */ }
  throw new Refused(res.status, b.error?.message ?? (res.statusText || `HTTP ${res.status}`), b.error?.code ?? null);
}
const e = encodeURIComponent;
const q = (qid: string) => `/api/queues/${e(qid)}`;

export const listQueues = () => call<{ queues: Queue[] }>("/api/queues");
export const createQueue = (f: QueueFields) => call<{ queue: Queue }>("/api/queues", "POST", f);
export const updateQueue = (qid: string, f: Partial<QueueFields>) => call<{ queue: Queue }>(q(qid), "PATCH", f);
export const deleteQueue = (qid: string) => call<unknown>(q(qid), "DELETE");
/** A page of the queue's reports, every state, from `cursor`. */
export const reportPage = (qid: string, cursor: string | null) =>
  call<ReportPage>(`${q(qid)}/reports${cursor ? `?cursor=${e(cursor)}` : ""}`);
export const listMembers = (qid: string) => call<{ members: QueueMember[] }>(`${q(qid)}/members`);
/** Adds person `pid` to the queue at `role`, or changes their role. */
export const setMember = (qid: string, pid: string, role: QueueRole) => call<unknown>(`${q(qid)}/members/${e(pid)}`, "PUT", { role });
export const removeMember = (qid: string, pid: string) => call<unknown>(`${q(qid)}/members/${e(pid)}`, "DELETE");
export const listGrants = (qid: string) => call<{ grants: ServiceGrant[] }>(`${q(qid)}/service-grants`);
/** Makes a service grant; its token and private key are in this answer alone. */
export const createGrant = (qid: string, name: string) => call<ServiceGrantCreated>(`${q(qid)}/service-grants`, "POST", { name });
/** Revokes a service grant, which ends every reporter token it minted. */
export const revokeGrant = (qid: string, gid: string) => call<unknown>(`${q(qid)}/service-grants/${e(gid)}`, "DELETE");
