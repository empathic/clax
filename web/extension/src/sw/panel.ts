// A side panel's actions (spec 2026-10-05 §9.4): each message `isFromPanel`
// took acts on the tab the panel shows. Thread actions go to the daemon for
// the tab's live page, and the thread the daemon answers with is applied at
// once (the stream brings it too). A failure is told to the panel as
// `failed {code, message}`; one a new pairing can fix is also kept as the
// tab's error, so the panel's Retry pairs again for it.
import { MAX_SITES, type PanelToWorker, RETRYABLE, type WorkerToPanel } from "../messages";
import type { Api } from "./api";
import { originOf } from "./origins";
import type { Pairer } from "./pairing";
import type { Sites } from "./site";
import type { Tabs } from "./tabs";

export type PanelDeps = {
  api: Pick<Api, "sendThread" | "sendBatch" | "comment" | "resolve" | "reopen" | "looked" | "setName" | "move" | "addRule" | "deleteRule" | "suggest" | "sites" | "join" | "split" | "answer">;
  tabs: Pick<Tabs, "ready" | "state" | "admits" | "route" | "applied" | "fail" | "setViewer" | "select" | "setCommentMode" | "commentOn" | "openThread" | "opening">;
  sites: Pick<Sites, "load" | "origins">;
  /** Whether `url`'s server answers a short request (decision 3, 2026-10-06: a
   * thread opens on the first origin of its site that answers). */
  probe(url: string): Promise<boolean>;
  /** The tab's title, for a suggestion's match by title. */
  title(tabId: number): Promise<string>;
  /** Whether Chrome lets Clax into `origin` (its host permission): a thread opens only where Clax can follow. */
  allowed(origin: string): Promise<boolean>;
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
/** `origin` without its scheme, as the panel names an address. */
const host = (origin: string) => origin.replace(/^\w+:\/\//, "");
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
  // A restarted worker reads its tabs back first: an action before then would find no tab.
  await d.tabs.ready();
  // Neither needs a tab: the name is the owner's, the ping keeps the worker up.
  if (m.t === "set-name") { d.tabs.setViewer((await d.api.setName(m.name)).viewer); return; }
  // A request (`req`) is always answered: a step or a failure.
  if (tabId === null && "req" in m) throw new PanelFailure("no_tab", "The panel shows no tab.");
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
      if (!m.on) { d.tabs.setCommentMode(tabId, false); return; }
      if (!d.tabs.admits(tabId) && !(await d.allUrls())) {
        throw new PanelFailure("no_capture_permission", "Clax needs the keyboard command on the page to take screenshots on this tab.");
      }
      await d.tabs.commentOn(tabId);
      return;
    case "turn-off":
      if (m.tabId !== tabId) throw changed();
      if (s?.on) await d.turnOff(tabId);
      return;
    default: break;
  }
  // The site's actions (spec §7.1, §7.2): on the origin Clax is on for in the tab, and its site.
  const on = s?.on;
  if (!on && (m.t === "suggest" || m.t === "answer" || m.t === "list-sites")) return;
  if (on) {
    const site = d.sites.origins(on);
    switch (m.t) {
      case "open-thread": {
        // The site's origins in their order now: which was used last may have changed since the listing came.
        if (site.length > 1) await d.sites.load(on);
        const where = d.tabs.openThread(tabId, m.threadId);
        if (!where) throw new PanelFailure("not_found", "That thread is not on this site any more.");
        // The most recently used address first; one that does not answer, the next (owner decision 2026-10-06).
        for (const o of where.origins) {
          if (o !== on && !(await d.allowed(o))) continue;
          const url = o + where.path;
          if (await d.probe(url)) { d.tabs.opening(tabId, m.threadId, url); await d.navigate(tabId, url); return; }
        }
        throw new PanelFailure("site_unreachable", `No address of this site answers (${where.origins.map(host).join(", ")}). Start its server, then try again.`);
      }
      case "move": {
        const o = originOf(m.pageUrl);
        if (!o || !site.includes(o)) throw new PanelFailure("cross_origin", "That page is on another site.");
        await d.api.move(m.threadId, m.pageUrl);
        return;
      }
      case "join": {
        if (m.origin !== on) throw changed();
        const r = await d.api.join(on, m.with);
        reply({ t: "step", req: m.req, moved: r.moved.length, remaining: r.remaining });
        // The site's listing changes (its `site` event says so too), and which page the tab's URL names.
        await d.sites.load(on);
        if (!r.remaining && s.url) await d.tabs.route(tabId, s.url, true);
        return;
      }
      case "split": {
        if (!site.includes(m.origin)) throw changed();
        await d.api.split(m.origin);
        reply({ t: "step", req: m.req, moved: 0, remaining: 0 });
        await d.sites.load(on);
        if (s.url) await d.tabs.route(tabId, s.url, true);
        return;
      }
      case "suggest": {
        const r = s.url ? await d.api.suggest(s.url, await d.title(tabId).catch(() => "")) : null;
        const first = r?.suggestions[0];
        reply({ t: "suggestion", origin: on, suggestion: first ? { origin: first.origin, origins: first.site.origins.map(o => o.origin), reason: first.reason, path: first.path } : null });
        return;
      }
      case "answer":
        await d.api.answer(on, m.with, m.answer);
        reply({ t: "suggestion", origin: on, suggestion: null });
        return;
      case "list-sites": {
        const r = await d.api.sites();
        reply({ t: "sites", sites: r.sites.slice(0, MAX_SITES).map(x => ({ key: x.site.key, name: x.site.name, origins: x.site.origins.map(o => o.origin) })) });
        return;
      }
      case "rule": case "unrule": {
        // The panel names the site it means: another tab now shows in it, or the tab left the site.
        if (m.origin !== on) throw changed();
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
  if ("req" in m) throw changed();
  const page = s?.page;
  if (!page) throw new PanelFailure("no_page", "This tab shows no live page.");
  const aid = page.artifact_id;
  switch (m.t) {
    case "navigate": {
      if (m.artifactId !== aid) throw changed();
      if (m.route !== null && !/^[?#]/.test(m.route)) throw new PanelFailure("invalid_route", "Not a route of this page.");
      // On the tab's own origin when the page is its site's (spec §7.2): the page's key may be another of its origins.
      const base = on && d.sites.origins(on).includes(page.origin) ? on + page.path : page.page_url;
      await d.navigate(tabId, base + (m.route ?? ""));
      return;
    }
    case "send": d.tabs.applied(tabId, (await d.api.sendThread(aid, m.threadId, m.to)).thread); return;
    case "send-batch": for (const t of (await d.api.sendBatch(aid, m.threadIds, m.note, m.to)).threads) d.tabs.applied(tabId, t); return;
    case "reply": d.tabs.applied(tabId, (await d.api.comment(aid, m.threadId, m.body)).thread); return;
    case "resolve": d.tabs.applied(tabId, (await d.api.resolve(aid, m.threadId)).thread); return;
    case "reopen": d.tabs.applied(tabId, (await d.api.reopen(aid, m.threadId)).thread); return;
    case "looked": await d.api.looked(aid, m.threadIds); return;
    default: return;
  }
}
