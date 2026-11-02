// The shell's TriageApi (web/overlay/src/triage): the queue routes with the
// browser's own session, and the page's stream's `queues` ping (and its
// coming back) as what may have changed.
import type { TriageApi } from "../../../overlay/src/triage/triage";
import type { Report } from "../../../sdk/src/types";
import type { EventStream } from "../stream";
import * as api from "./api";

const e = encodeURIComponent;
const r = (qid: string, tid: string) => `/api/queues/${e(qid)}/reports/${e(tid)}`;

async function send<T>(path: string, body?: unknown): Promise<T> {
  const res = await fetch(path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body ?? {}) });
  const b = await res.json().catch(() => ({}));
  if (!res.ok) throw new api.Refused(res.status, b?.error?.message ?? `HTTP ${res.status}`, b?.error?.code ?? null);
  return b as T;
}

export function shellTriage(stream?: EventStream): TriageApi {
  return {
    queues: async () => (await api.listQueues()).queues,
    reports: (qid, cursor) => api.reportPage(qid, cursor),
    report: async (qid, tid) => {
      const res = await fetch(r(qid, tid));
      const b = await res.json().catch(() => ({}));
      if (!res.ok) throw new api.Refused(res.status, b?.error?.message ?? `HTTP ${res.status}`, b?.error?.code ?? null);
      return (b as { report: Report }).report;
    },
    // The clip route answers the PNG with the session; the image loads it itself.
    clip: async (qid, tid) => `${r(qid, tid)}/clip`,
    reply: async (qid, tid, body) => (await send<{ report: Report }>(`${r(qid, tid)}/comments`, { body })).report,
    resolve: async (qid, tid) => (await send<{ report: Report }>(`${r(qid, tid)}/resolve`)).report,
    watch: changed => stream?.watch(["queues"], ev => { if (ev.type === "queues" || ev.type === "ready" || ev.type === "resync") changed(); }) ?? (() => {}),
  };
}
