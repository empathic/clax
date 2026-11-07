// A queue's Zulip notifier, for its panel alone. The API key is write-only: no
// response carries it, only its `KeyInfo`.
import { type KeyInfo, call } from "../bot/api";

const route = (qid: string) => `/api/queues/${encodeURIComponent(qid)}/notifiers/zulip`;
/** What the notifier's form sets. `api_key` left out keeps the key set, on the same site. */
export type ZulipFields = {
  site: string; bot_email: string; api_key?: string; stream: string;
  /** GitHub login to Zulip email. */
  people: Record<string, string>;
  /** The `rev` this form was read at; null for no notifier. */
  if_rev: string | null;
};
/** A queue's Zulip notifier as read. `last_failure` is in fixed words. */
export type ZulipNotifier = Omit<ZulipFields, "api_key" | "if_rev"> & {
  kind: "zulip"; rev: string; api_key: KeyInfo; last_ok_at: string | null; last_failure: string | null; last_failure_at: string | null;
  /** What failed in a direct message, apart from the stream's `last_failure`. */
  direct_failure?: string | null; direct_failure_at?: string | null;
  /** Notices still being tried, and those that stopped being tried unsent. */
  pending: number; expired: number;
  set_by: { id: string; name: string | null }; set_at: string;
};
export type NotifierView = { notifier: ZulipNotifier | null };
export const getNotifier = (qid: string) => call<NotifierView>(route(qid));
export const putNotifier = (qid: string, f: ZulipFields) => call<NotifierView>(route(qid), "PUT", f);
/** Removes the notifier, its key and the notices it had not sent. */
export const deleteNotifier = (qid: string, if_rev: string) => call<unknown>(route(qid), "DELETE", { if_rev });
/** Sends a test message to the stream, by the saved settings: 429 `test_too_soon`, or 502 `notifier_failed` in fixed words when it does not go. */
export const testNotifier = (qid: string, if_rev: string) => call<NotifierView>(`${route(qid)}/test`, "POST", { if_rev });
