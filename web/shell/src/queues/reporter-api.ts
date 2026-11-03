// A reporter's view of its queue (crates/clax-server/src/routes/queue_reporter_view.rs):
// the routes under /reporter/<qid>/api, read with the view session's cookie,
// which a view link from the reporter's app set. The server decides what the
// reporter sees of the queue's other reporters; this only reads it.
import type { Report, ReportPage, ReportersSee } from "../../../sdk/src/types";

const e = encodeURIComponent;

/** The queue as its reporter sees it. */
export interface ReporterQueue {
  id: string;
  name: string;
  reporters_see: ReportersSee;
}

/** A refusal: `expired` when the view session is gone (401), so the page says to open it from the app again. */
export class ViewRefused extends Error {
  constructor(readonly status: number, message: string) {
    super(message);
  }
  get expired(): boolean {
    return this.status === 401;
  }
}

/** `/reporter/<qid>[/<tid>]` as its parts; null for another path. */
export function reporterRouteOf(path: string): { qid: string; tid: string | null } | null {
  const m = /^\/reporter\/([^/]+)(?:\/([^/]+))?\/?$/.exec(path);
  if (!m || m[2] === "enter" || m[2] === "api") return null;
  return { qid: decodeURIComponent(m[1]), tid: m[2] ? decodeURIComponent(m[2]) : null };
}

export const reporterQueuePath = (qid: string) => `/reporter/${e(qid)}`;
export const reporterReportPath = (qid: string, tid: string) => `${reporterQueuePath(qid)}/${e(tid)}`;

async function read<T>(res: Response): Promise<T> {
  const b = await res.json().catch(() => ({}));
  if (!res.ok) throw new ViewRefused(res.status, b?.error?.message ?? `HTTP ${res.status}`);
  return b as T;
}

/** The view's routes for queue `qid`. */
export function reporterApi(qid: string) {
  const api = `${reporterQueuePath(qid)}/api`;
  const report = (tid: string) => `${api}/reports/${e(tid)}`;
  return {
    queue: async () => (await read<{ queue: ReporterQueue }>(await fetch(`${api}/queue`))).queue,
    reports: async (cursor: string | null) =>
      read<ReportPage>(await fetch(`${api}/reports${cursor ? `?cursor=${e(cursor)}` : ""}`)),
    report: async (tid: string) => (await read<{ report: Report }>(await fetch(report(tid)))).report,
    /** The clip loads with the cookie, so its route is the image's URL. */
    clip: (tid: string) => `${report(tid)}/clip`,
    reply: async (tid: string, body: string) =>
      (await read<{ report: Report }>(await fetch(`${report(tid)}/comments`, {
        method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ body }),
      }))).report,
  };
}

export type ReporterApi = ReturnType<typeof reporterApi>;

/** Who filed `r`, as the list and the report say it: "You" for the reader's own, the name its app gave, or "Another reporter". Null when the queue shows reporters only their own. */
export function filedBy(r: Report): string | null {
  if (!r.submitter) return null;
  if (r.submitter.you) return "You";
  return r.submitter.name ?? "Another reporter";
}
