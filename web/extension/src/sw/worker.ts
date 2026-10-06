// The worker's parts wired together: the pairer, the API client over it,
// the shell's stream hub with requests through the API client (so the
// stream carries the credential and pairs again like any request), and the
// tabs, one hub client each. A new pairing reconnects the hub: the old
// credential's stream cannot be changed or resumed by the new one.
import { Hub } from "../../../shell/src/stream-hub";
import type { WorkerToOverlay } from "../messages";
import { Api } from "./api";
import { type PairEnv, Pairer } from "./pairing";
import { Tabs } from "./tabs";

export type WorkerDeps = {
  pair: PairEnv;
  fetch?: typeof fetch;
  toOverlay(tabId: number, m: WorkerToOverlay): void;
  inject(tabId: number): Promise<void>;
  /** Where the tabs are kept across worker restarts (chrome.storage.session). */
  store?: { get(k: string): Promise<Record<string, unknown>>; set(v: Record<string, unknown>): Promise<void> };
};
export type Worker = { pairer: Pairer; api: Api; hub: Hub; tabs: Tabs };

export function createWorker(d: WorkerDeps): Worker {
  const pairer = new Pairer(d.pair);
  const api = new Api(pairer, d.fetch);
  let tabs: Tabs | null = null;
  const hub = new Hub({
    send: (ids, msg) => tabs?.fromHub(ids, msg),
    // The hub renews the shell's events cookie at `/api/token` after a
    // failure; the extension has no cookie, and the gateway refuses that route.
    fetch: (input, init) => (String(input) === "/api/token" ? Promise.resolve(new Response(null, { status: 204 })) : api.request(String(input), init)),
    base: "",
  });
  tabs = new Tabs({ api, hub, toOverlay: d.toOverlay, inject: d.inject, store: d.store });
  const t = tabs;
  api.onRepair = () => t.repaired();
  return { pairer, api, hub, tabs: t };
}
