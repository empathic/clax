// The DevTools panel's TriageApi (web/overlay/src/triage): the owner's queue
// routes through the worker (which alone holds the credential; the gateway
// admits reads, replies and resolves, never a claim) and the worker's
// `queue-ping`. What comes back is checked for the fields the views read,
// and a report's page URL kept only when http(s).
import type { Queue, Report, TriageApi } from "../../../overlay/src/triage/triage";
import type { PanelLink } from "../panel/link.svelte";

export type QueueLink = Pick<PanelLink, "queue" | "onQueuePing">;

const obj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const str = (v: unknown): v is string => typeof v === "string";
const off = () => new Error("Clax answered something else.");

export function isQueue(v: unknown): v is Queue {
  return obj(v) && str(v.id) && str(v.name) && Array.isArray(v.origins) && v.origins.every(str) && (v.role === "admin" || v.role === "triage");
}

export function isReport(v: unknown): v is Report {
  return obj(v) && str(v.id) && str(v.queue_id) && obj(v.artifact) && str(v.artifact.url) && str(v.artifact.title)
    && (v.state === "open" || v.state === "claimed" || v.state === "resolved") && (v.status === "open" || v.status === "resolved")
    && str(v.created_at) && typeof v.has_clip === "boolean" && (v.claim === null || (obj(v.claim) && str(v.claim.holder_name) && str(v.claim.expires_at)))
    && Array.isArray(v.comments) && v.comments.every(c => obj(c) && str(c.id) && str(c.body) && str(c.author_name) && str(c.author_kind) && str(c.created_at));
}

/** A daemon older than this extension refuses it the queue routes (its gateway's allowlist predates them): said as such. */
export function older(e: unknown): unknown {
  const m = e instanceof Error ? e.message : "";
  return /may not use this route/.test(m) ? new Error("This Clax is older than the extension and does not show queues here yet: update Clax.") : e;
}

const report = (d: unknown): Report => { if (obj(d) && isReport(d.report)) return d.report; throw off(); };

/** The panel's TriageApi, and `base`: the paired daemon's origin, once the queues were read (null before, or when not http(s)). */
export function panelTriage(link: QueueLink): TriageApi & { base: string | null } {
  const api: TriageApi & { base: string | null } = {
    base: null,
    queues: async () => {
      const d = await link.queue({ t: "queue-list" }).catch(e => { throw older(e); });
      if (!obj(d) || !Array.isArray(d.queues)) throw off();
      api.base = str(d.base) && /^https?:\/\/[^/]+\/?$/.test(d.base) ? d.base.replace(/\/$/, "") : null;
      return d.queues.filter(isQueue);
    },
    reports: async (qid, cursor) => {
      const d = await link.queue({ t: "queue-reports", qid, cursor });
      if (!obj(d) || !Array.isArray(d.reports) || !(d.next === null || str(d.next))) throw off();
      return { reports: d.reports.filter(isReport), next: d.next };
    },
    report: async (qid, tid) => report(await link.queue({ t: "queue-report", qid, tid })),
    clip: async (qid, tid) => {
      const d = await link.queue({ t: "queue-clip", qid, tid });
      return str(d) && d.startsWith("data:image/png;base64,") ? d : null;
    },
    reply: async (qid, tid, body) => report(await link.queue({ t: "queue-reply", qid, tid, body })),
    resolve: async (qid, tid) => report(await link.queue({ t: "queue-resolve", qid, tid })),
    watch: changed => link.onQueuePing(changed),
  };
  return api;
}

/** The expression that shows `selector`'s element in the inspected page: scrolled to, outlined for two seconds and
 * selected in Elements (`inspect`, DevTools' own command line helper); true when found. The selector is a JSON literal. */
export const showExpr = (selector: string) => `(() => {
  let el = null;
  try { el = document.querySelector(${JSON.stringify(selector)}); } catch (e) { return false; }
  if (!el) return false;
  el.scrollIntoView({ block: "center", behavior: "smooth" });
  const was = el.style.outline;
  el.style.outline = "3px solid #e0532f";
  setTimeout(() => { el.style.outline = was; }, 2000);
  try { inspect(el); } catch (e) { /* not in DevTools */ }
  return true;
})()`;
