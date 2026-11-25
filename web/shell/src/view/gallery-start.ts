// What the gallery's pages need before their first render, and where it
// comes from: the daemon's bootstrap block (`gallery-boot.ts`) where it
// answered, else the routes asked all at once. The gallery renders each
// region once, in its final shape: the header when the header's answers
// are in, the content (the cards and, for a browser with an inbox, the
// unread summary above them) when everything that lays it out is in. Pure
// but for the requests `Io` makes, so the rules are unit-tested here.
import type { Artifact, AttentionSummary, InboxSummary, QuestionView } from "../api";
import type { HostedMe } from "../hosted/api";
import { ApiError, reasonText } from "../trouble";
import { type BotMe, type GalleryBoot, type GalleryField, type ViewerMe, GALLERY_ROUTES, answered } from "./gallery-boot";

/** Which of the gallery entry's views a path is: the gallery's list, a page
 * beside it (`/inbox`, `/tags`, `/queues…`, `/settings…`, and any other
 * one-word path, as `Gallery.svelte` reads it), or a hosted daemon's
 * sign-in and landing pages, which load their own chunks. The entry's
 * inline `clax-page` script (`shell/index.html`) decides it the same way
 * before any script loads (a test holds them to each other). */
export type PageKind = "gallery" | "page" | "auth";

/** A hosted daemon's sign-in pages (`/setup`, `/device`, `/join/<code>`, `/signin[/<code>]`). */
export const SIGN_IN_PATH = /^\/(setup$|device$|join\/|signin(\/|$))/;
/** Its landing page. */
export const LANDING_PATH = "/start/";

/** `path` with a queue's and a report's view read as the Queues page and a
 * Settings screen as Settings: the only sub-paths a page has. */
export const topOf = (path: string): string => path.replace(/^\/(queues|settings)\/.+/, "/$1");

export function pageKind(path: string): PageKind {
  if (SIGN_IN_PATH.test(path) || path === LANDING_PATH) return "auth";
  const top = topOf(path);
  return top === "/inbox" || /^\/[a-z]+$/.test(top) ? "page" : "gallery";
}

/** A route's answer: its status (0 when it did not answer) and its JSON body when that is 200. */
export type Answer<T> = { status: number; body: T | null };

/** The token route's answer: the token, null when it is refused (not the
 * owner's browser), or "failed" when it did not answer (asked again later). */
export type TokenAnswer = string | null | "failed";

/** The header's answers, as far as they are in. */
export type HeaderAnswers = { viewer?: Answer<ViewerMe>; hosted?: Answer<HostedMe>; feedback?: Answer<unknown>; bot?: Answer<BotMe>; summary?: Answer<InboxSummary>; token?: TokenAnswer };

/** What the header shows, decided once at its first render. */
export type Header = {
  /** `GET /api/viewers/me`'s body (null when it did not answer). */
  me: ViewerMe | null;
  /** The hosted daemon's answer (`GET /api/hosted/me`), null on a local daemon. */
  hosted: HostedMe | null;
  /** The browser is the owner's (the token is served to it). */
  owner: boolean;
  /** The bar names the site (a hosted daemon) rather than "local artifacts". */
  site: boolean;
  /** The browser has an inbox (the owner's, or a connected person's); null when not known yet. */
  inbox: boolean | null;
  /** The browser is a member's: Settings' gear. */
  member: boolean;
  /** Queues shows: for the owner, a member of a queue, and anyone who may make one. */
  queues: boolean;
  /** The config route's body, for the feedback menu (`feedback/boot.ts`); undefined when not asked. */
  feedback: unknown;
  /** `GET /api/me/bot`'s body (the meter), or null. */
  bot: BotMe | null;
  /** The inbox's unread summary (`GET /api/inbox/summary`), for a browser
   * with an inbox: Inbox's count in the header, and on the gallery the
   * summary above the cards. Null when there is none or it did not answer. */
  summary: InboxSummary | null;
};

const person = (v: ViewerMe | null | undefined) => !!v?.viewer?.person && !v.viewer.person_revoked_at;

/** The header from its answers; `inbox` is the block's word for it, when it has one. */
export function header(a: HeaderAnswers, inbox: boolean | null = null): Header {
  const me = a.viewer?.status === 200 ? a.viewer.body : null;
  const hosted = a.hosted?.status === 200 ? a.hosted.body : null;
  const bot = a.bot?.status === 200 ? a.bot.body : null;
  const summary = a.summary?.status === 200 ? a.summary.body : null;
  const token = typeof a.token === "string" && a.token !== "failed" ? a.token : null;
  const hostedOwner = hosted?.person?.owner === true;
  // The token's answer decides it where it was asked; without it (the
  // block's header), the viewer's `owner`, or a block that says this browser
  // has an inbox, of a viewer who is no person, says the token is served to it.
  const owner = !!token || hostedOwner || (a.token === undefined && (me?.viewer?.owner === true || (inbox === true && !!me && !person(me))));
  const member = owner || person(me);
  // The token refused and the viewer known: no inbox, unless a person's.
  const known = person(me) || owner ? true : a.token === null && !!a.viewer?.status ? false : null;
  return {
    me, hosted, owner, member, bot, summary,
    site: !!hosted || !!token?.startsWith("cxo_"),
    inbox: inbox ?? known,
    queues: member && (owner || (me?.queues?.member ?? 0) > 0 || me?.queues?.may_make === true),
    feedback: a.feedback?.body,
  };
}

/** What the content region lays out with. */
export type Content = {
  artifacts: Artifact[] | null;
  /** Why the list could not load. */
  error: string | null;
  attention: Record<string, AttentionSummary>;
  /** The inbox's open questions, for a browser with an inbox (the unread summary's cards). */
  questions: QuestionView[] | null;
};

/** The requests the gallery makes, each answering rather than rejecting (but `list`). */
export type Io = {
  /** `GET path`'s answer. */
  get(path: string): Promise<Answer<unknown>>;
  token(): Promise<TokenAnswer>;
  /** The feedback config route's body (`configOnce`, shared with the feedback boot). */
  feedback(): Promise<unknown>;
  /** The artifact list, retried while it fails for want of the daemon; rejects with what to show. */
  list(): Promise<Artifact[]>;
  /** Whether this browser holds a local owner session's proof: its inbox is asked at once. */
  proof: boolean;
};

/** The fields the page `kind` reads: the header's, and on the gallery the content's. */
export const HEADER_FIELDS = ["viewer", "hosted", "feedback", "bot", "summary"] as const satisfies readonly GalleryField[];
export const CONTENT_FIELDS = ["artifacts", "attention", "questions"] as const satisfies readonly GalleryField[];

/** The block's answer for `f`, or undefined when the block has none (ask the route). */
function fromBlock<T>(b: GalleryBoot | null, f: GalleryField): Answer<T> | undefined {
  if (!b || !answered(b, f)) return undefined;
  const status = b.status[f]!;
  return { status, body: status === 200 ? ((b as Record<string, unknown>)[f] as T) ?? null : null };
}

/** Whether this browser's inbox is asked at once, beside the rest: the
 * block says it has one, or, the block not saying, it holds the local
 * owner's proof. Any other is asked nothing of an inbox until the header
 * says it has one. */
const inboxNow = (b: GalleryBoot | null, io: Io) => b?.inbox === true || (b?.inbox == null && io.proof);

/** The header at once from the block, when it holds every answer the
 * header needs (the token is not one of them: the block's viewer and inbox
 * say whether the browser is the owner's); else null. */
export function headerFromBlock(b: GalleryBoot | null): Header | null {
  if (!b || b.inbox === null || !HEADER_FIELDS.every(f => f === "summary" ? !b.inbox || answered(b, f) : answered(b, f))) return null;
  return header({ viewer: fromBlock(b, "viewer"), hosted: fromBlock(b, "hosted"), feedback: fromBlock(b, "feedback"), bot: fromBlock(b, "bot"), summary: b.inbox ? fromBlock(b, "summary") : undefined }, b.inbox);
}

const asked = <T>(io: Io, f: GalleryField) => io.get(GALLERY_ROUTES[f]) as Promise<Answer<T>>;
const one = <T>(b: GalleryBoot | null, io: Io, f: GalleryField): Promise<Answer<T>> => {
  const known = fromBlock<T>(b, f);
  if (known) return Promise.resolve(known);
  if (f === "feedback") return io.feedback().then(body => ({ status: body == null ? 0 : 200, body: body as T }));
  return asked<T>(io, f);
};

/** The header's answers: the block's, and the routes it lacks, asked at once
 * (with the token, which a header without the block's word needs), and the
 * inbox's summary once the browser is known to have one. */
export async function gatherHeader(b: GalleryBoot | null, io: Io, token: Promise<TokenAnswer>): Promise<Header> {
  const now = headerFromBlock(b);
  if (now) return now;
  const early = inboxNow(b, io) ? one<InboxSummary>(b, io, "summary") : null;
  const [viewer, hosted, feedback, bot, t] = await Promise.all([one<ViewerMe>(b, io, "viewer"), one<HostedMe>(b, io, "hosted"), one<unknown>(b, io, "feedback"), one<BotMe>(b, io, "bot"), token]);
  const h = header({ viewer, hosted, feedback, bot, token: t }, b?.inbox ?? null);
  if (!h.inbox) return h;
  return { ...h, summary: (await (early ?? one<InboxSummary>(b, io, "summary"))).body };
}

/** Why the block's list was refused, in the person's words: the daemon wrote
 * the block, so the refusal is Clax's own, and no status number is shown. */
const refusal = (status: number) => reasonText(new ApiError(status, "", null, true));

/** The content's answers: the list and this viewer's attention, and for a
 * browser with an inbox its open questions; each from the block where it
 * answered, the rest asked at once (the questions, as the summary is, once
 * the header says the browser has an inbox, unless asked at once). */
export function gatherContent(b: GalleryBoot | null, io: Io, hdr: Promise<Header>): Promise<Content> {
  const list = fromBlock<{ artifacts: Artifact[] }>(b, "artifacts");
  // A refused list (the block's status) says why as the route would.
  const listed: Promise<{ artifacts: Artifact[] | null; error: string | null }> = list
    ? Promise.resolve(list.body ? { artifacts: list.body.artifacts, error: null } : { artifacts: null, error: refusal(list.status) })
    : io.list().then(artifacts => ({ artifacts, error: null }), (e: unknown) => ({ artifacts: null, error: reasonText(e) }));
  const attention = one<{ artifacts: Record<string, AttentionSummary> }>(b, io, "attention").then(a => a.body?.artifacts ?? {});
  const early = inboxNow(b, io) ? one<{ questions: QuestionView[] }>(b, io, "questions") : null;
  const open = hdr.then(h => (h.inbox ? early ?? one<{ questions: QuestionView[] }>(b, io, "questions") : null)).then(q => (q?.body ? q.body.questions ?? [] : null));
  return Promise.all([listed, attention, open]).then(([l, a, questions]) => ({ ...l, attention: a, questions }));
}

/** The content at once from the block, when it holds every answer the
 * content needs and the header is known; else null. */
export function contentFromBlock(b: GalleryBoot | null, h: Header | null): Content | null {
  if (!b || !h || h.inbox === null) return null;
  const need: GalleryField[] = h.inbox ? [...CONTENT_FIELDS] : ["artifacts", "attention"];
  if (!need.every(f => answered(b, f))) return null;
  const list = fromBlock<{ artifacts: Artifact[] }>(b, "artifacts")!;
  const q = h.inbox ? fromBlock<{ questions: QuestionView[] }>(b, "questions")! : null;
  return {
    artifacts: list.body?.artifacts ?? null,
    error: list.body ? null : refusal(list.status),
    attention: fromBlock<{ artifacts: Record<string, AttentionSummary> }>(b, "attention")!.body?.artifacts ?? {},
    questions: q?.body ? q.body.questions ?? [] : null,
  };
}
