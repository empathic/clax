import type { Artifact, AttentionSummary, InboxSummary, QuestionView } from "../api";
import type { HostedMe } from "../hosted/api";

/** A field of the gallery's bootstrap that one route answers: its name in
 * the block and the route (each `GET`), as the gallery fetches it. */
export const GALLERY_ROUTES = {
  viewer: "/api/viewers/me",
  hosted: "/api/hosted/me",
  feedback: "/_app/feedback/config",
  bot: "/api/me/bot",
  artifacts: "/api/artifacts",
  attention: "/api/viewers/me/attention",
  summary: "/api/inbox/summary",
  questions: "/api/questions?status=open&limit=200",
} as const;
export type GalleryField = keyof typeof GALLERY_ROUTES;

/** `GET /api/viewers/me`'s body. */
export type ViewerMe = {
  viewer: { public_id?: string; display_name?: string | null; person?: boolean; person_revoked_at?: string | null; owner?: boolean } | null;
  may_create?: boolean;
};

/** `GET /api/me/bot`'s body, as far as the gallery's header reads it: the
 * caller's role (Queues for an admin), and whether ClaxBot is on (its meter). */
export type BotMe = { enabled?: boolean; role?: "owner" | "admin" | "member" } & Record<string, unknown>;

/** The daemon's first-load data for a gallery page (`/`, `/inbox`, `/tags`,
 * `/queues…`, `/settings…`, `/people`, `/admin`, `/me`, `/audit`), from
 * `crates/clax-server/src/shell/gallery.rs`, which documents the contract.
 *
 * Each route field is exactly the JSON body its route (`GALLERY_ROUTES`)
 * answers this browser's own `fetch`, so it goes to the parser the shell
 * already has for that route:
 * - `status[f]` is the HTTP status that route answered; `f` is its body
 *   when that is 200, else null (`hosted` on a local daemon: 404, null).
 *   A status of 200 or a refusal is the answer: the shell need not ask.
 * - No `status[f]` (the route failed (5xx, 408, 429) or was slow, or the
 *   field was left out, as on back/forward): `f` is null and the shell asks
 *   the route itself.
 * - `artifacts`, `attention` and `questions` are there on `/` alone
 *   (absent elsewhere); `summary` (Inbox's count) on every page.
 *   `summary` and `questions` are the inbox's (refused, 401 or 403, to a
 *   browser without one).
 * - `inbox`: whether this browser has an inbox, as `inboxBrowser` in
 *   `owner.ts` decides it (a live connected person's, or the owner's);
 *   null when not known, and the shell asks.
 * The shell token (`GET /api/token`) is never in it. */
export type GalleryBoot = {
  v: 1;
  kind: "gallery";
  status: Partial<Record<GalleryField, number>>;
  viewer: ViewerMe | null;
  hosted: HostedMe | null;
  /** `GET /_app/feedback/config`'s body, for `readConfig` (`feedback/boot.ts`). */
  feedback: unknown;
  bot: BotMe | null;
  inbox: boolean | null;
  artifacts?: { artifacts: Artifact[] } | null;
  attention?: { artifacts: Record<string, AttentionSummary> } | null;
  summary?: InboxSummary | null;
  questions?: { questions: QuestionView[]; open: number } | null;
};

/** The fields that depend on who this browser is, which a page from the
 * browser's cache may hold for someone else's cookie: all but `artifacts`,
 * which the gallery's stream refreshes. */
const VIEWER_FIELDS = ["viewer", "hosted", "feedback", "bot", "attention", "summary", "questions"] as const;

/** How this document was reached (`back_forward` for history), from
 * Navigation Timing, as `readBoot` (`boot.ts`) reads it. */
function navigationType(): string {
  try {
    return (performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming | undefined)?.type ?? "";
  } catch {
    return "";
  }
}

/** Whether `f` was answered by its route: its status is known (the body
 * is `b[f]` when that status is 200, else the route refused). */
export function answered(b: GalleryBoot, f: GalleryField): boolean {
  return typeof b.status[f] === "number";
}

/** The gallery's bootstrap block in `doc`, or null when there is none, it
 * does not parse, or it is not a version 1 gallery block. A page reached
 * through history may come from the browser's cache without asking the
 * daemon, so what depends on the viewer may be stale: those fields are
 * left out (null, with no status) and `inbox` is null, and the shell asks
 * their routes itself, as `readBoot` does with the artifact page's viewer. */
export function readGalleryBoot(doc: Document = document, navType: string = navigationType()): GalleryBoot | null {
  const text = doc.getElementById("clax-boot")?.textContent;
  if (!text) return null;
  let b: GalleryBoot;
  try {
    b = JSON.parse(text) as GalleryBoot;
  } catch {
    return null;
  }
  if (!(b && b.v === 1 && b.kind === "gallery" && b.status && typeof b.status === "object")) return null;
  if (navType !== "back_forward") return b;
  const status = { ...b.status };
  const out: GalleryBoot = { ...b, status, inbox: null };
  for (const f of VIEWER_FIELDS) {
    delete status[f];
    if (f in out) (out as Record<string, unknown>)[f] = null;
  }
  return out;
}
