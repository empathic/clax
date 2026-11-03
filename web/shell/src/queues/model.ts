// The Queues page's rules, apart from its views: a queue's origins as typed
// one per line, its reports counted by state, the page's addresses, how a
// report's reporter, context and anchor read, and a refusal as a sentence.
import type { DiagLine, FixOutcome, Report, ReportContext, ReportPage, ReportState, ResolveReason } from "../../../sdk/src/types";
import type { StateFilter } from "./api";

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
/** The queue view's address, and a report's. */
export const queueHref = (qid: string) => `/queues/${encodeURIComponent(qid)}`;
export const reportHref = (qid: string, tid: string) => `${queueHref(qid)}/${encodeURIComponent(tid)}`;

/** Who filed `r`, as triage knows them: the profile's name or GitHub
 * login, else the reporter ID, else "Anonymous". */
export function reporterName(r: Pick<Report, "reporter_profile" | "reporter_id">): string {
  const p = r.reporter_profile;
  return p?.name?.trim() || (p?.github ? `@${p.github}` : "") || r.reporter_id || "Anonymous";
}
/** The note `r` was filed with: its first comment. */
export const noteOf = (r: Pick<Report, "comments">): string => r.comments[0]?.body ?? "";

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

/** A browser and its major version, read from a user agent: "Chrome 131",
 * "Safari 18", "Firefox 132", "Edge 131"; the agent itself when none is known. */
export function browserOf(ua: string | undefined): string | null {
  if (!ua) return null;
  const v = (re: RegExp) => re.exec(ua)?.[1];
  const known: [string, RegExp][] = [["Edge", /Edg(?:e|A|iOS)?\/(\d+)/], ["Firefox", /(?:Firefox|FxiOS)\/(\d+)/], ["Chrome", /(?:Chrome|CriOS)\/(\d+)/], ["Safari", /Version\/(\d+)[.\d]* .*Safari\//]];
  for (const [name, re] of known) { const n = v(re); if (n) return `${name} ${n}`; }
  return ua;
}

/** `iso` as a date and time in `zone` (the reader's when absent), with the
 * zone's short name; `null` for a zone the browser does not know or a time
 * it cannot read. */
export function timeIn(iso: string, zone?: string, locale?: string): string | null {
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return null;
  try {
    return new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short", timeZone: zone }).format(t)
      + ` ${zoneName(t, zone, locale)}`;
  } catch { return null; }
}
function zoneName(t: Date, zone: string | undefined, locale: string | undefined): string {
  return new Intl.DateTimeFormat(locale, { timeZone: zone, timeZoneName: "short" }).formatToParts(t).find(p => p.type === "timeZoneName")?.value ?? zone ?? "";
}

/** What a report's anchor points at, as a line: the quote, the element's
 * name, the custom name or the selector, after the kind. */
export function anchorText(anchor: unknown): string | null {
  if (!anchor || typeof anchor !== "object") return null;
  const a = anchor as { kind?: string; quote?: string | null; label?: string; name?: string; custom_name?: string | null; selector?: string | null };
  const kind = { element: "Element", range: "Text", area: "Area", custom: "Custom", artifact: "The whole page" }[a.kind ?? ""] ?? a.kind ?? "Anchor";
  const what = a.label || (a.quote ? `“${a.quote}”` : "") || a.name || a.custom_name || a.selector || "";
  return what ? `${kind}: ${what}` : kind;
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
 * follows each deploy. */
export function installTag(instance: string, qid: string, pin: OverlayPin | null): string {
  const base = instance.replace(/\/+$/, "");
  const src = `${base}${pin?.src ?? "/_clax/overlay.js"}`;
  const sri = pin ? ` integrity="${attr(pin.integrity)}" crossorigin="anonymous"` : "";
  return `<script async src="${attr(src)}"${sri} data-container="${attr(qid)}" data-auth="/clax/token"></script>`;
}

/** A way an integration reaches pages: each with the artifact its owner pastes. */
export interface Deployment { id: "render" | "edge-cloudflare" | "edge-caddy" | "edge-nginx" | "tag-manager"; label: string; where: string; artifact: string }

/** The copy-paste artifact of each deployment of queue `qid`, all carrying
 * the same tag (`installTag`). The extension is the fifth carrier and needs
 * no artifact: it installs on the pages its owner adds. */
export function deployments(instance: string, qid: string, pin: OverlayPin | null): Deployment[] {
  const tag = installTag(instance, qid, pin);
  const js = JSON.stringify(tag);
  return [
    { id: "render", label: "Page or template", where: "In <head> of each page, or the site's third-party head-snippet setting", artifact: tag },
    {
      id: "edge-cloudflare", label: "Cloudflare Worker", where: "A Worker on the site's route",
      artifact: `export default {\n  async fetch(req, env) {\n    const res = await fetch(req);\n    if (!(res.headers.get("content-type") ?? "").includes("text/html")) return res;\n    return new HTMLRewriter()\n      .on("head", { element(e) { e.append(${js}, { html: true }); } })\n      .transform(res);\n  },\n};`,
    },
    { id: "edge-caddy", label: "Caddy", where: "In the site's block (needs the replace-response plugin)", artifact: `replace {\n\t"</head>" ${JSON.stringify(`${tag}</head>`)}\n}` },
    { id: "edge-nginx", label: "nginx", where: "In the site's location (ngx_http_sub_module)", artifact: `sub_filter '</head>' '${tag.replace(/'/g, "\\'")}</head>';\nsub_filter_once on;` },
    { id: "tag-manager", label: "Tag manager", where: "A Custom HTML tag fired on all pages", artifact: tag },
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
  const filed = Math.max(0, total(b) - total(a));
  const parts: [number, string][] = [
    [filed, "filed"],
    [b.claimed - a.claimed, "claimed"],
    [b.fix_proposed - a.fix_proposed, "fix proposed"],
    [b.verified - a.verified, "verified"],
    [b.resolved - a.resolved, "resolved"],
    [b.open - a.open - filed, "open again"],
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
