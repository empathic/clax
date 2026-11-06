// The Queues page's rules, apart from its views: a queue's origins as typed
// one per line, its reports counted by state, the page's addresses, how a
// report's reporter, context and anchor read, and a refusal as a sentence.
import type { DiagLine, FixOutcome, Report, ReportContext, ReportPage, ReportState, ResolveReason } from "../../../sdk/src/types";
import { anchorText, browserOf, stateTone, timeIn } from "../../../bridge/src/report-text";
import type { StateFilter } from "./api";

export { anchorText, browserOf, stateTone, timeIn };

/** The origins typed in `text`, one per line: each as its origin
 * (`https://app.example`, no trailing slash), the first of each kept;
 * `bad` holds the lines that are not an http or https origin (a path, a
 * query or another scheme). Blank lines are skipped. */
export function parseOrigins(text: string): { origins: string[]; bad: string[] } {
  const origins: string[] = [];
  const bad: string[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line) continue;
    const o = originOf(line);
    if (!o) bad.push(line);
    else if (!origins.includes(o)) origins.push(o);
  }
  return { origins, bad };
}

function originOf(s: string): string | null {
  let u: URL;
  try { u = new URL(s); } catch { return null; }
  if (u.protocol !== "https:" && u.protocol !== "http:") return null;
  if (u.username || u.password || u.pathname !== "/" || u.search || u.hash || s.endsWith("?") || s.endsWith("#")) return null;
  return u.origin;
}

/** The stream topic of queue `qid`'s report changes (a `report` event,
 * IDs and state, for its members alone). */
export const queueTopic = (qid: string) => `queue:${qid}`;

/** A queue's reports counted by state; `more` when the pages ran past the
 * limit; `lapses` the soonest a claim counted ends (its lease's
 * `expires_at`, in ms), when one does: no event says a lease lapsed. */
export type Counts = Record<ReportState, number> & { more: boolean; lapses: number | null };

/** Counts the reports `page` gives, following each page's `next` for at most `limit` pages. */
export async function countReports(page: (cursor: string | null) => Promise<ReportPage>, limit = 20): Promise<Counts> {
  const c: Counts = { open: 0, claimed: 0, fix_proposed: 0, verified: 0, resolved: 0, more: false, lapses: null };
  let cursor: string | null = null;
  for (let i = 0; i < limit; i++) {
    const p = await page(cursor);
    for (const r of p.reports) {
      if (r.state in c) c[r.state]++;
      const end = r.state === "claimed" && r.claim ? Date.parse(r.claim.expires_at) : NaN;
      if (!Number.isNaN(end) && (c.lapses === null || end < c.lapses)) c.lapses = end;
    }
    cursor = p.next;
    if (!cursor) return c;
  }
  c.more = true;
  return c;
}

/** `Counts` as a line, every state in the order a report moves through
 * them: "3 open · 1 claimed · 2 fix proposed · 4 verified · 9 resolved",
 * a `+` after each when there are more. */
export function countsText(c: Counts): string {
  const plus = c.more ? "+" : "";
  return STATES.map(s => `${c[s]}${plus} ${STATE_LABEL[s].toLowerCase()}`).join(" · ");
}

/** Every state, in the order a report moves through them. */
export const STATES: readonly ReportState[] = ["open", "claimed", "fix_proposed", "verified", "resolved"];
/** A state as the views name it. */
export const STATE_LABEL: Record<ReportState, string> = {
  open: "Open", claimed: "Claimed", fix_proposed: "Fix proposed", verified: "Verified", resolved: "Resolved",
};
/** Why a report was closed, as the views say it. */
export const REASON_LABEL: Record<ResolveReason, string> = {
  fixed: "Fixed", duplicate: "Duplicate", wontfix: "Won't fix", cannot_reproduce: "Can't reproduce", other: "Other",
};
/** What became of a fix, as the views say it. */
export const OUTCOME_LABEL: Record<FixOutcome, string> = {
  pending: "Waiting for the reporter", verified: "Verified", rejected: "Still broken", superseded: "Superseded", closed: "Closed",
};

/** The queue view's tabs: the filter each lists by (`null`: every state). */
export const TABS: readonly { key: string; label: string; state: StateFilter | null }[] = [
  { key: "active", label: "Active", state: "active" },
  { key: "open", label: "Open", state: "open" },
  { key: "claimed", label: "Claimed", state: "claimed" },
  { key: "fix_proposed", label: "Fix proposed", state: "fix_proposed" },
  { key: "verified", label: "Verified", state: "verified" },
  { key: "resolved", label: "Resolved", state: "resolved" },
  { key: "all", label: "All", state: null },
];

/** Which view of the Queues page `pathname` is: the list (`/queues`), a
 * queue's reports (`/queues/<qid>`) or one report (`/queues/<qid>/<tid>`). */
export type QueuesRoute = { view: "list" } | { view: "queue"; qid: string } | { view: "report"; qid: string; tid: string };
export function queuesRoute(pathname: string): QueuesRoute {
  const m = /^\/queues\/([^/]+)(?:\/([^/]+))?\/?$/.exec(pathname);
  if (!m) return { view: "list" };
  const d = (s: string) => { try { return decodeURIComponent(s); } catch { return s; } };
  return m[2] ? { view: "report", qid: d(m[1]), tid: d(m[2]) } : { view: "queue", qid: d(m[1]) };
}
/** The queue view's address (on the tab of `state` when given), and a report's. */
export const queueHref = (qid: string, state?: string) => `/queues/${encodeURIComponent(qid)}${state && state !== "active" ? `?state=${encodeURIComponent(state)}` : ""}`;
export const reportHref = (qid: string, tid: string) => `${queueHref(qid)}/${encodeURIComponent(tid)}`;

/** Who filed `r`, as triage knows them: the profile's name or GitHub
 * login, else the reporter ID, else "Anonymous". */
export function reporterName(r: Pick<Report, "reporter_profile" | "reporter_id">): string {
  const p = r.reporter_profile;
  return p?.name?.trim() || (p?.github ? `@${p.github}` : "") || r.reporter_id || "Anonymous";
}
/** The note `r` was filed with: its first comment. */
export const noteOf = (r: Pick<Report, "comments">): string => r.comments[0]?.body ?? "";

/** The first line of `r`'s note with any text, trimmed. */
export const firstLine = (r: Pick<Report, "comments">): string => noteOf(r).split(/\r?\n/).find(l => l.trim())?.trim() ?? "";
/** How many comments followed `r`'s filing note. */
export const replyCount = (r: Pick<Report, "comments">): number => Math.max(0, r.comments.length - 1);

/** One queue's listing as far as it has been fetched, the latest filed first. */
export type Feed = { reports: Report[]; next: string | null };
const filed = (r: Pick<Report, "created_at">) => Date.parse(r.created_at) || 0;
/** The reports of every feed, the latest filed first, that are certainly
 * in order: none newer can come from a feed's next page, so a report
 * older than the oldest fetched of a feed with more is held back until
 * that feed is fetched further. */
export function newestAcross(feeds: readonly Feed[]): Report[] {
  const all = feeds.flatMap(f => f.reports).sort((a, b) => filed(b) - filed(a) || b.id.localeCompare(a.id));
  const ends = feeds.filter(f => f.next && f.reports.length).map(f => filed(f.reports[f.reports.length - 1]));
  if (!ends.length) return all;
  const cutoff = Math.max(...ends);
  return all.filter(r => filed(r) >= cutoff);
}

/** `s` as a link target when it is an http or https URL, else `null`. */
export function httpUrl(s: string | null | undefined): string | null {
  if (!s) return null;
  try {
    const u = new URL(s);
    return u.protocol === "https:" || u.protocol === "http:" ? u.href : null;
  } catch { return null; }
}

/** The repository `context.app.repo` names: an http(s) repository URL, or
 * `owner/name`, read as GitHub's. */
export function repoUrl(app: unknown): string | null {
  const repo = app && typeof app === "object" ? (app as { repo?: unknown }).repo : undefined;
  if (typeof repo !== "string") return null;
  const s = repo.trim();
  if (/^[\w.-]+\/[\w.-]+$/.test(s)) return `https://github.com/${s}`;
  const u = httpUrl(s);
  return u ? u.replace(/\/+$/, "").replace(/\.git$/, "") : null;
}

/** The release's commit as text, with a link to it and the link's host
 * when the context's `app.repo` names a repository; `null` when the release
 * names no commit. */
export function commitLink(ctx: ReportContext | undefined): { text: string; href: string | null; host: string | null } | null {
  const c = ctx?.release?.commit?.trim();
  if (!c) return null;
  const repo = repoUrl(ctx?.app);
  const href = repo && /^[0-9A-Za-z._-]+$/.test(c) ? `${repo}/commit/${c}` : null;
  return { text: c.length > 12 && /^[0-9a-f]+$/i.test(c) ? c.slice(0, 12) : c, href, host: href ? new URL(href).host : null };
}

/** Where in the code a report points (`context.app.code`, which the SDK's
 * code-line picker sends: `{file, start, end, side?, sha?}`), as text and,
 * when `app.repo` names the repository and a commit is given, a link to
 * those lines at that commit; null when the context has none or it is not
 * well formed. */
export function codeOf(ctx: ReportContext | undefined): { text: string; href: string | null } | null {
  const app = ctx?.app as { code?: unknown } | undefined;
  const c = app && typeof app === "object" ? (app.code as Record<string, unknown> | undefined) : undefined;
  if (!c || typeof c !== "object") return null;
  const { file, start, end, side, sha } = c;
  if (typeof file !== "string" || !file || !Number.isSafeInteger(start) || !Number.isSafeInteger(end) || (start as number) < 1 || (end as number) < (start as number)) return null;
  const lines = start === end ? `line ${start}` : `lines ${start}–${end}`;
  const commit = typeof sha === "string" && /^[0-9a-f]{7,64}$/i.test(sha) ? sha : null;
  const text = `${file}, ${lines}${side === "LEFT" ? " (old side)" : ""}${commit ? ` at ${commit.slice(0, 12)}` : ""}`;
  const repo = repoUrl(ctx?.app);
  const path = file.split("/").map(encodeURIComponent).join("/");
  const href = repo && commit && side !== "LEFT" ? `${repo}/blob/${commit}/${path}#L${start}${end !== start ? `-L${end}` : ""}` : null;
  return { text, href };
}

/** A diagnostics line's levels, most severe first. */
export const LEVELS: readonly DiagLine["level"][] = ["error", "warn", "info", "debug"];

/** The overlay a container's answer pins: `src` relative to the instance. */
export interface OverlayPin { src: string; integrity: string }

/** Where this instance serves queue `qid`'s container. */
export const containerPath = (qid: string) => `/_clax/containers/${encodeURIComponent(qid)}/config.json`;

const attr = (v: string) => v.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");

/** The one tag a site pastes to install queue `qid` from `instance`:
 * pinned to `pin`'s version with its integrity, or the bare URL, which
 * follows each deploy; `deployment` names the deployment that carries it,
 * which the DevTools panel's Integration view shows. */
export function installTag(instance: string, qid: string, pin: OverlayPin | null, deployment?: Deployment["id"]): string {
  const base = instance.replace(/\/+$/, "");
  const src = `${base}${pin?.src ?? "/_clax/overlay.js"}`;
  const sri = pin ? ` integrity="${attr(pin.integrity)}" crossorigin="anonymous"` : "";
  const dep = deployment ? ` data-deployment="${deployment}"` : "";
  return `<script async src="${attr(src)}"${sri} data-container="${attr(qid)}" data-auth="/clax/token"${dep}></script>`;
}

/** A way an integration reaches pages: each with the artifact its owner pastes. */
export interface Deployment { id: "render" | "edge-cloudflare" | "edge-caddy" | "edge-nginx" | "tag-manager"; label: string; where: string; artifact: string }

/** The copy-paste artifact of each deployment of queue `qid`, each carrying
 * the same tag (`installTag`) naming that deployment. The extension is the
 * sixth carrier and needs no artifact: it installs on the pages its owner adds. */
export function deployments(instance: string, qid: string, pin: OverlayPin | null): Deployment[] {
  const tag = (d: Deployment["id"]) => installTag(instance, qid, pin, d);
  return [
    { id: "render", label: "Page or template", where: "In <head> of each page, or the site's third-party head-snippet setting", artifact: tag("render") },
    {
      id: "edge-cloudflare", label: "Cloudflare Worker", where: "A Worker on the site's route",
      artifact: `export default {\n  async fetch(req, env) {\n    const res = await fetch(req);\n    if (!(res.headers.get("content-type") ?? "").includes("text/html")) return res;\n    return new HTMLRewriter()\n      .on("head", { element(e) { e.append(${JSON.stringify(tag("edge-cloudflare"))}, { html: true }); } })\n      .transform(res);\n  },\n};`,
    },
    { id: "edge-caddy", label: "Caddy", where: "In the site's block (needs the replace-response plugin)", artifact: `replace {\n\t"</head>" ${JSON.stringify(`${tag("edge-caddy")}</head>`)}\n}` },
    { id: "edge-nginx", label: "nginx", where: "In the site's location (ngx_http_sub_module)", artifact: `sub_filter '</head>' '${tag("edge-nginx").replace(/'/g, "\\'")}</head>';\nsub_filter_once on;` },
    { id: "tag-manager", label: "Tag manager", where: "A Custom HTML tag fired on all pages", artifact: tag("tag-manager") },
  ];
}

/** One copyable snippet of an install's "Add it to your site". */
export interface SiteSnippet { id: string; label: string; text: string }
/** One way a site adds the overlay, with its snippets. */
export interface SiteWay { id: "script" | "build" | "server"; title: string; snippets: SiteSnippet[] }

/** The three ways a site adds install `iid` of queue `qid` from `instance`
 * — a script tag, a build-time import, and the host's backend minting the
 * reporter token with the queue's service grant — each filled with these
 * values. `origin` is the page's origin the backend names (one of the
 * install's). Secrets stay placeholders: the grant's token and key are
 * shown once, when the grant is made. */
export function siteWays(instance: string, qid: string, iid: string, origin: string): SiteWay[] {
  const base = instance.replace(/\/+$/, "");
  const [b, q, i, o] = [base, qid, iid, origin].map(v => JSON.stringify(v));
  const route = "/clax/reporter-token";
  return [
    {
      id: "script", title: "A script tag loaded from Clax",
      snippets: [{
        id: "script-tag", label: "Script tag, in each page",
        text: `<script src="${attr(`${base}/_clax/overlay.js`)}"></script>
<script>
  const feedback = Clax.overlay({
    instance: ${b},
    queue: ${q},
    install: ${i}, // the install's wording and accent colour apply
    auth: async (publicKey) => {
      const r = await fetch("${route}", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ public_key: publicKey }),
      });
      if (!r.ok) throw new Error("no reporter token: " + r.status);
      return r.json(); // { token, expires_at }
    },
  });
</script>`,
      }],
    },
    {
      id: "build", title: "At build, with the SDK package or the overlay's types",
      snippets: [
        {
          id: "sdk", label: "@empathic/clax-client-sdk",
          text: `import { loadOverlay, reporterTokenFrom } from "@empathic/clax-client-sdk";

const Clax = await loadOverlay(${b});
const handle = Clax.overlay({
  instance: ${b},
  queue: ${q},
  install: ${i},
  auth: reporterTokenFrom("${route}"), // POSTs { public_key }, reads { token, expires_at }
});`,
        },
        {
          id: "dts", label: "Dynamic import, typed by overlay.d.ts",
          text: `// Save ${base}/_clax/overlay.d.ts beside your code as clax-overlay.d.ts
import type { OverlayHandle, OverlayOptions } from "./clax-overlay";
type Overlay = typeof import("./clax-overlay");

const OVERLAY_URL = ${JSON.stringify(`${base}/_clax/overlay.mjs`)};

export async function startOverlay(opts: Omit<OverlayOptions, "instance" | "queue" | "install">): Promise<OverlayHandle> {
  const { overlay } = (await import(/* @vite-ignore */ OVERLAY_URL)) as Overlay;
  return overlay({ instance: ${b}, queue: ${q}, install: ${i}, ...opts });
}`,
        },
      ],
    },
    {
      id: "server", title: "Server-side: your backend mints the reporter token",
      snippets: [
        {
          id: "env", label: "Your backend's environment",
          text: `CLAX_INSTANCE=${base}
CLAX_QUEUE=${qid}
CLAX_INSTALL=${iid}
CLAX_QUEUE_TOKEN=cxq_…          # the service grant's token, shown once when it is made
CLAX_QUEUE_KEY=<private_key>    # the service grant's private_key, as shown`,
        },
        {
          id: "route", label: `The token route, ${route}`,
          text: `import { ServiceClient } from "@empathic/clax-client-sdk/service";

const clax = new ServiceClient({
  instance: process.env.CLAX_INSTANCE!,
  queue: process.env.CLAX_QUEUE!,
  token: process.env.CLAX_QUEUE_TOKEN!,
  privateKey: process.env.CLAX_QUEUE_KEY!,
});

// POST ${route} { public_key }, behind your own sign-in.
app.post("${route}", async (req, res) => {
  const user = req.session.user;
  if (!user) return res.status(401).json({ error: "sign in first" });
  const minted = await clax.mintReporterToken({
    reporter_id: user.id,
    origin: ${o}, // the page's origin, one of the install's
    public_key: req.body.public_key,
  });
  res.set("cache-control", "no-store").json(minted); // { token, expires_at }
});`,
        },
        {
          id: "own-button", label: "Your own button (launcher: false)",
          text: `const handle = Clax.overlay({ instance: ${b}, queue: ${q}, install: ${i}, auth, launcher: false });
myButton.addEventListener("click", () => handle.report());
// The count waiting on the reporter (a fix to verify, or an unread reply):
handle.subscribe(s => { myButton.dataset.waiting = s.waiting ? String(s.waiting) : ""; });`,
        },
      ],
    },
  ];
}

/** What changed in queue `name` from counts `a` to `b`, as a sentence for
 * a screen reader: "1 report claimed in Bugs.", "2 reports filed, 1
 * resolved in Bugs."; `null` when nothing rose or either count is cut
 * short (`more`). A report back in `open` (released, or reopened by a
 * comment) is "open again". */
export function changeText(name: string, a: Counts, b: Counts): string | null {
  if (a.more || b.more) return null;
  const total = (c: Counts) => STATES.reduce((n, s) => n + c[s], 0);
  const added = Math.max(0, total(b) - total(a));
  const parts: [number, string][] = [
    [added, "filed"],
    [b.claimed - a.claimed, "claimed"],
    [b.fix_proposed - a.fix_proposed, "fix proposed"],
    [b.verified - a.verified, "verified"],
    [b.resolved - a.resolved, "resolved"],
    [b.open - a.open - added, "open again"],
  ];
  const said = parts.filter(([n]) => n > 0).map(([n, what], i) => (i === 0 ? `${n} report${n === 1 ? "" : "s"} ${what}` : `${n} ${what}`));
  if (!said.length) return null;
  return `${said.join(", ")} in ${name}.`;
}

/** What to show for a refusal `e`: the daemon's message (lower case, no
 * full stop) as a sentence. */
export function refusal(e: unknown): string {
  const s = (e instanceof Error ? e.message : String(e)).trim();
  if (!s) return "Something went wrong. Try again.";
  return `${s[0].toUpperCase()}${s.slice(1)}${/[.!?]$/.test(s) ? "" : "."}`;
}
