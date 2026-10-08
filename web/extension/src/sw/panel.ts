// A side panel's actions (spec 2026-10-05 §9.4): each message `isFromPanel`
// took acts on the tab the panel shows. Thread actions go to the daemon for
// the tab's live page, or for the page of its site the thread is on (§7.1),
// and the thread the daemon answers with is applied at once (the stream
// brings it too). A thread's clip, and the agents and versions of another
// page of the site, are fetched here, with the credential, for the panel,
// which holds none. A failure is told to the panel as
// `failed {code, message}`; one a new pairing can fix is also kept as the
// tab's error, so the panel's Retry pairs again for it. The owner's
// questions and inbox act on no tab (spec 2026-10-06-agent-questions-and-inbox
// §9.6): each is a request, answered by its result.
import { MAX_SITES, type PanelToWorker, RETRYABLE, type WorkerToPanel } from "../messages";
import type { Api } from "./api";
import type { WorkerInbox } from "./inbox";
import { originOf } from "./origins";
import type { Pairer } from "./pairing";
import type { Sites } from "./site";
import type { Tabs } from "./tabs";

export type PanelDeps = {
  api: Pick<Api, "sendThread" | "sendBatch" | "comment" | "resolve" | "reopen" | "looked" | "setName" | "move" | "addRule" | "deleteRule" | "suggest" | "sites" | "join" | "split" | "answer" | "clip" | "artifact" | "closeQuestion" | "markItem" | "markItems">;
  tabs: Pick<Tabs, "ready" | "state" | "admits" | "route" | "applied" | "fail" | "setViewer" | "select" | "setCommentMode" | "commentOn" | "openThread" | "opening" | "onTabs">;
  sites: Pick<Sites, "load" | "origins" | "view" | "applied">;
  /** The owner's questions and inbox. */
  inbox: Pick<WorkerInbox, "page" | "count">;
  /** Opens tabs for inbox items: the paired daemon's origin, a new tab at a URL, and bringing a tab to the front. */
  open: {
    daemon(): Promise<string>; create(url: string): Promise<void>; focus(tabId: number): Promise<void>;
    /** A tab at `url` (any of them), among the tabs whose address the extension may read; null for none. */
    find(url: string): Promise<number | null>;
  };
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

/** The thread `id` of the listing of the site Clax is on for at `on`. */
const siteThread = (d: PanelDeps, on: string | null | undefined, id: string) =>
  d.sites.view(on ?? null)?.pages.flatMap(p => p.threads).find(t => t.id === id);

export async function panelAction(d: PanelDeps, tabId: number | null, m: PanelToWorker, reply: (r: WorkerToPanel) => void): Promise<void> {
  try {
    await act(d, tabId, m, reply);
  } catch (e) {
    const f = failed(e);
    // Only a tab's own action leaves its error on the tab (the Page view's notice).
    if (tabId !== null && RETRYABLE.has(f.code) && !INBOX_ACTIONS.has(m.t)) d.tabs.fail(tabId, e);
    reply("req" in m ? { ...f, req: m.req } : f);
  }
}

/** The question and inbox requests, which act on no tab. */
const INBOX_ACTIONS = new Set<PanelToWorker["t"]>(["q-answer", "q-decline", "q-release", "inbox-page", "inbox-mark", "inbox-mark-all", "open-url"]);
const LIVE_ITEM = /^\/a\/([0-9a-hjkmnp-tv-z]{12})(?:[/?#]|$)/;
const ULID = /^[0-9A-HJKMNP-TV-Z]{26}$/;

/** Opens daemon path `path` (an inbox item's `url`, which `isFromPanel`
 * checked is a path): a tab showing that live page comes to the front, its
 * thread (`?thread=`) selected; otherwise a new tab opens it on the paired daemon. */
export async function openPath(d: PanelDeps, path: string, pageUrl: string | null = null): Promise<void> {
  const aid = LIVE_ITEM.exec(path)?.[1];
  const tabId = aid ? d.tabs.onTabs().find(id => d.tabs.state(id)?.page?.artifact_id === aid) : undefined;
  if (tabId !== undefined) {
    await d.open.focus(tabId);
    const thread = new URLSearchParams(path.split("?")[1]?.split("#")[0] ?? "").get("thread");
    if (thread && ULID.test(thread)) d.tabs.select(tabId, thread);
    return;
  }
  // A tab showing the live page with Clax off in it comes to the front as it is.
  const other = aid && pageUrl ? await d.open.find(pageUrl) : null;
  if (other !== null) { await d.open.focus(other); return; }
  await d.open.create((await d.open.daemon()) + path);
}

/** A question or inbox message: acted on, and its result told; false for any other message. */
async function inboxAction(d: PanelDeps, m: PanelToWorker, reply: (r: WorkerToPanel) => void): Promise<boolean> {
  const api = d.api;
  switch (m.t) {
    case "q-answer": case "q-decline": case "q-release": {
      const verb = m.t === "q-answer" ? "answer" : m.t === "q-decline" ? "decline" : "release";
      const r = await api.closeQuestion(m.questionId, verb, m.t === "q-answer" ? m.body : undefined);
      reply({ t: "q-done", req: m.req, question: r.question, closed: r.closed });
      return true;
    }
    case "inbox-page": reply({ t: "inbox-page", req: m.req, page: await d.inbox.page(m.filter, m.before) }); return true;
    case "inbox-mark": {
      // One item read or unread through its own route (its view comes back); several read at once.
      const r = m.ids.length === 1 ? await api.markItem(m.ids[0], m.read) : { item: null, ...(await api.markItems({ ids: m.ids })) };
      d.inbox.count(r.unread);
      reply({ t: "marked", req: m.req, item: r.item, marked: "marked" in r ? (r.marked as number) : 1, unread: r.unread });
      return true;
    }
    case "inbox-mark-all": {
      const r = await api.markItems({ all: true, filter: m.filter, upto: m.upto });
      d.inbox.count(r.unread);
      reply({ t: "marked", req: m.req, item: null, marked: r.marked, unread: r.unread });
      return true;
    }
    case "open-url": await openPath(d, m.url, m.pageUrl); return true;
    default: return false;
  }
}

async function act(d: PanelDeps, tabId: number | null, m: PanelToWorker, reply: (r: WorkerToPanel) => void): Promise<void> {
  // A restarted worker reads its tabs back first: an action before then would find no tab.
  await d.tabs.ready();
  if (await inboxAction(d, m, reply)) return;
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
    case "far-page": {
      // The page of a thread of the site's listing: its live agents (the Send
      // button's picker there) and its versions (its comments' tags).
      const t = siteThread(d, s?.on, m.threadId);
      const art = t ? await d.api.artifact(t.artifact_id).catch(() => null) : null;
      reply({ t: "far-page", req: m.req, page: art && t ? {
        artifactId: t.artifact_id, agents: art.artifact.participants?.agents ?? [],
        // Only what a version tag reads: no files, notes or labels cross.
        versions: art.versions.map(v => ({ n: v.n, created_at: v.created_at, agent_harness: v.agent_harness ?? null })),
      } : null });
      return;
    }
    case "clip": {
      // Of the tab's page, or of another page of its site; null for a thread it knows of neither, or one with no clip.
      const t = s?.threads.find(x => x.id === m.threadId) ?? siteThread(d, s?.on, m.threadId);
      reply({ t: "clip", req: m.req, url: t?.clip_url ? await d.api.clip(t.artifact_id, t.id).catch(() => null) : null });
      return;
    }
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
  // A thread of another page of the tab's site (spec §7.1) is acted on at
  // its own page, and the answer applied to the site's listing.
  if (m.t === "looked" && on && m.threadIds.length && !m.threadIds.some(id => s.threads.some(t => t.id === id))) {
    const far = m.threadIds.map(id => siteThread(d, on, id));
    const aid = far[0]?.artifact_id;
    if (aid && far.every(t => t?.artifact_id === aid)) { await d.api.looked(aid, m.threadIds); return; }
  }
  if ((m.t === "send" || m.t === "reply" || m.t === "resolve" || m.t === "reopen") && on && !s.threads.some(t => t.id === m.threadId)) {
    const t = siteThread(d, on, m.threadId);
    if (t) {
      const aid = t.artifact_id;
      const r = m.t === "send" ? await d.api.sendThread(aid, t.id, m.to) : m.t === "reply" ? await d.api.comment(aid, t.id, m.body)
        : m.t === "resolve" ? await d.api.resolve(aid, t.id) : await d.api.reopen(aid, t.id);
      d.sites.applied(on, r.thread);
      return;
    }
  }
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
