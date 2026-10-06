// A side panel's actions (spec 2026-10-05 §9.4): each message `isFromPanel`
// took acts on the tab the panel shows. Thread actions go to the daemon for
// the tab's live page, and the thread the daemon answers with is applied at
// once (the stream brings it too). A failure is told to the panel as
// `failed {code, message}`; one a new pairing can fix is also kept as the
// tab's error, so the panel's Retry pairs again for it.
import { type PanelToWorker, RETRYABLE, type WorkerToPanel } from "../messages";
import type { Api } from "./api";
import { originOf } from "./origins";
import type { Pairer } from "./pairing";
import type { Sites } from "./site";
import type { Tabs } from "./tabs";

export type PanelDeps = {
  api: Pick<Api, "sendThread" | "sendBatch" | "comment" | "resolve" | "reopen" | "looked" | "setName" | "move" | "addRule" | "deleteRule">;
  tabs: Pick<Tabs, "state" | "admits" | "route" | "applied" | "fail" | "setViewer" | "select" | "setCommentMode" | "openThread">;
  sites: Pick<Sites, "load">;
  pairer: Pick<Pairer, "pair">;
  /** Whether the extension holds `<all_urls>` (screenshots on any tab without a click). */
  allUrls(): Promise<boolean>;
  /** Loads `url` in the tab. */
  navigate(tabId: number, url: string): Promise<void>;
  /** Turns Clax off in the tab: its panel, its overlay and its record go. */
  turnOff(tabId: number): Promise<void>;
};

class PanelFailure extends Error {
  constructor(readonly code: string, message: string) { super(message); }
}
/** The panel acted on a page the tab no longer shows, or on another tab. */
const changed = () => new PanelFailure("page_changed", "The tab shows another page now.");
const failed = (e: unknown): Extract<WorkerToPanel, { t: "failed" }> => {
  const err = e as { code?: unknown; message?: unknown };
  return { t: "failed", code: typeof err?.code === "string" ? err.code : "failed", message: typeof err?.message === "string" ? err.message : String(e) };
};

export async function panelAction(d: PanelDeps, tabId: number | null, m: PanelToWorker, reply: (r: WorkerToPanel) => void): Promise<void> {
  try {
    await act(d, tabId, m, reply);
  } catch (e) {
    const f = failed(e);
    if (tabId !== null && RETRYABLE.has(f.code)) d.tabs.fail(tabId, e);
    reply("req" in m ? { ...f, req: m.req } : f);
  }
}

async function act(d: PanelDeps, tabId: number | null, m: PanelToWorker, reply: (r: WorkerToPanel) => void): Promise<void> {
  // Neither needs a tab: the name is the owner's, the ping keeps the worker up.
  if (m.t === "set-name") { d.tabs.setViewer((await d.api.setName(m.name)).viewer); return; }
  if (m.t === "ping" || m.t === "visible" || tabId === null) return;
  const s = d.tabs.state(tabId);
  switch (m.t) {
    case "watch-tab": if (s?.url) await d.tabs.route(tabId, s.url); return;
    case "retry":
      // Only a pairing or credential failure needs a new pairing; the person asked, so it is not rate-limited.
      if (s?.error && RETRYABLE.has(s.error.code)) await d.pairer.pair(true);
      if (s?.url) await d.tabs.route(tabId, s.url, true);
      return;
    case "select": d.tabs.select(tabId, m.threadId); return;
    case "comment-mode":
      if (m.on && !d.tabs.admits(tabId) && !(await d.allUrls())) {
        throw new PanelFailure("no_capture_permission", "Clax needs a click on its button to take screenshots on this tab.");
      }
      d.tabs.setCommentMode(tabId, m.on);
      return;
    case "turn-off":
      if (m.tabId !== tabId) throw changed();
      if (s?.on) await d.turnOff(tabId);
      return;
    default: break;
  }
  // The site's actions (spec §7.1): on the origin Clax is on for in the tab.
  const on = s?.on;
  if (on) {
    switch (m.t) {
      case "open-thread": {
        const url = d.tabs.openThread(tabId, m.threadId);
        if (!url) throw new PanelFailure("not_found", "That thread is not on this site any more.");
        await d.navigate(tabId, url);
        return;
      }
      case "move":
        if (originOf(m.pageUrl) !== on) throw new PanelFailure("cross_origin", "That page is on another site.");
        await d.api.move(m.threadId, m.pageUrl);
        return;
      case "rule": case "unrule": {
        const r = m.t === "rule" ? await d.api.addRule(on, m.pattern) : await d.api.deleteRule(m.ruleId);
        reply({ t: "step", req: m.req, moved: r.moved.length, remaining: r.remaining });
        // A rule changes the listing (no event says so), and once done, which page the tab's URL names.
        await d.sites.load(on);
        if (!r.remaining && s.url) await d.tabs.route(tabId, s.url, true);
        return;
      }
      default: break;
    }
  }
  const page = s?.page;
  if (!page) throw new PanelFailure("no_page", "This tab shows no live page.");
  const aid = page.artifact_id;
  switch (m.t) {
    case "navigate":
      if (m.artifactId !== aid) throw changed();
      if (m.route !== null && !/^[?#]/.test(m.route)) throw new PanelFailure("invalid_route", "Not a route of this page.");
      await d.navigate(tabId, page.page_url + (m.route ?? ""));
      return;
    case "send": d.tabs.applied(tabId, (await d.api.sendThread(aid, m.threadId, m.to)).thread); return;
    case "send-batch": for (const t of (await d.api.sendBatch(aid, m.threadIds, m.note, m.to)).threads) d.tabs.applied(tabId, t); return;
    case "reply": d.tabs.applied(tabId, (await d.api.comment(aid, m.threadId, m.body)).thread); return;
    case "resolve": d.tabs.applied(tabId, (await d.api.resolve(aid, m.threadId)).thread); return;
    case "reopen": d.tabs.applied(tabId, (await d.api.reopen(aid, m.threadId)).thread); return;
    case "looked": await d.api.looked(aid, m.threadIds); return;
    default: return;
  }
}
