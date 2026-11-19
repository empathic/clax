// Site analytics' routes on a hosted Clax (crates/clax-server/src/routes/analytics.rs),
// called with the browser's cookie through the shell's confirmation flow
// (`held.ts`): the read, `GET /api/artifacts/<id>/analytics`, and the
// setting, `PATCH /api/artifacts/<id> {"analytics": "on" | "off"}`, which
// needs no approval. A refusal throws `Refused` with the daemon's message.
import { browserCall } from "../held";
import type { AnalyticsAnswer, AnalyticsSetting } from "./analytics-model";

const e = encodeURIComponent;

/** The totals of the UTC days from `from` (`YYYY-MM-DD`) to the daemon's
 * today; `from` null takes the daemon's default, the last 30 days. */
export const readAnalytics = (aid: string, from: string | null) =>
  browserCall<AnalyticsAnswer>(`/api/artifacts/${e(aid)}/analytics${from ? `?from=${e(from)}` : ""}`);

/** Turns counting on or off; the answer carries what the daemon now holds. */
export const setAnalytics = (aid: string, setting: AnalyticsSetting) =>
  browserCall<{ artifact?: { analytics?: AnalyticsSetting } }>(`/api/artifacts/${e(aid)}`, "PATCH", { analytics: setting });
