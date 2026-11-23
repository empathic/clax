// What decides the artifact view's optional parts: the top bar's Inbox,
// Clone, Publish, the feedback menu and ClaxBot's runs button and meter, and
// the questions at the top of the thread list. None of them holds the bar's
// essential controls (TopbarIsland.svelte): where the page knows a part's
// answer at the bar's first render (the bootstrap block's `deciders`,
// crates/clax-server/src/shell/boot.rs, or an answer this module asked for
// as the page started, beside the view's own data, already in), the part is
// drawn in its place then; one whose answer comes later takes the bar's free
// end (`ui/topbar-gate.svelte.ts` `late`), moving nothing.
import type { InboxSummary, QuestionView } from "../api";
import { getToken } from "../api";
import { bootFeedback, readConfig } from "../feedback/boot";
import { configOnce } from "../feedback/config";
import type { MeBot, RunSummary } from "../bot/api";
import type { CloneOptions } from "../ui/clone-api";
import type { Remotes } from "../ui/publish-api";
import { type Viewer, getViewer } from "../threads";

/** The block's `deciders`: each route field the JSON body its route
 * answers this browser (`status[f]` its status; with none the field is
 * null and the shell asks), and `inbox` and `owner` as the daemon decides
 * them (null: not known). */
export type Deciders = {
  status: Partial<Record<"feedback" | "bot" | "hosted" | "summary" | "questions" | "clone" | "runs", number>>;
  feedback: unknown;
  bot: MeBot | null;
  hosted: unknown;
  summary: InboxSummary | null;
  questions: { questions: QuestionView[]; open: number } | null;
  clone: CloneOptions | null;
  runs: { runs: RunSummary[] } | null;
  inbox: boolean | null;
  owner: boolean | null;
};

/** The inbox's first read: its unread summary and the open questions (null: not read). */
export type FirstInbox = { summary: InboxSummary | null; open: QuestionView[] | null };

const known = $state({
  /** Whether this browser has an inbox (Inbox shows), null until known. */
  inbox: null as boolean | null,
  /** Inbox's unread count, null until known. */
  unread: null as number | null,
  /** Whether this browser holds the owner's token, null until known. */
  owner: null as boolean | null,
  /** Whether the thread list's questions are decided: shown, or none to show. */
  questions: false,
  /** Whether this person may report problems with Clax (the feedback menu shows), null until known. */
  feedback: null as boolean | null,
});

/** What the page knows now (read-only). */
export const decided = {
  get inbox(): boolean | null { return known.inbox; },
  get unread(): number | null { return known.unread; },
  get owner(): boolean | null { return known.owner; },
  get questions(): boolean { return known.questions; },
  get feedback(): boolean | null { return known.feedback; },
};

/** Records whether this browser has an inbox, and marks the root
 * (`data-inbox`: the threads controls keep their question dot's place). */
function setInbox(has: boolean): void {
  known.inbox = has;
  if (!has) known.questions = true;
  document.documentElement.toggleAttribute("data-inbox", has);
}

/** Sets Inbox's unread count (the question module's inbox feed, as it changes). */
export function setUnread(n: number): void { known.unread = n; }

/** Marks the thread list's questions decided (drawn, or none to draw). */
export function questionsDecided(): void { known.questions = true; }

let block: Deciders | null = null;
let first: Promise<FirstInbox> | null = null;
let cloneAsk: Promise<CloneOptions | null> | null = null;
let remotesAsk: Promise<Remotes | null> | null = null;
// Their answers once in (undefined until then), read as the bar first renders.
let cloneNow: CloneOptions | null | undefined;
let remotesNow: Remotes | null | undefined;
let inboxAsk: Promise<void> | null = null;
let runsAsk: Promise<RunSummary[]> | null = null;
let feedbackAsk: Promise<void> | null = null;
let runsNow: RunSummary[] | undefined;

const answered = (f: keyof Deciders["status"]) => typeof block?.status[f] === "number";

/** Starts what decides artifact `id`'s optional parts, once per page: from
 * the block's `deciders` (`d`) where they answer, else asked now, beside
 * the view's own data. `inspecting`: the app's inspect view, which shows
 * none of them. */
export function startDeciders(id: string, d: Deciders | null, inspecting = false): void {
  block = d && typeof d === "object" && d.status && typeof d.status === "object" ? d : null;
  if (inspecting) return;
  // Reporting a problem with Clax (`feedback/`), asked first so the
  // overlay's recorder starts as early as it can.
  bootFeedback(undefined, answered("feedback") ? { body: block!.feedback } : undefined);
  if (answered("feedback")) known.feedback = readConfig(block!.feedback) !== null;
  else feedbackAsk = configOnce().then(b => { known.feedback = readConfig(b) !== null; }, () => { known.feedback = false; });
  void import("../bot/me").then(m => (answered("bot") ? m.seedBot(block!.status.bot === 200 ? block!.bot : null) : void m.myBot()), () => {});
  if (block && typeof block.owner === "boolean") known.owner = block.owner;
  else void getToken().then(t => { known.owner = !!t; });
  if (!answered("clone")) cloneAsk = import("../ui/clone-api").then(m => m.cloneOptions(id)).catch(() => null).then(o => (cloneNow = o));
  // Publish is the local owner's shell's alone (a hosted owner's token is `cxo_`).
  remotesAsk = getToken().then(t => (t && !t.startsWith("cxo_") ? import("../ui/publish-api").then(m => m.remotesOf(id)) : null)).catch(() => null).then(r => (remotesNow = r));
  // ClaxBot's runs of the artifact (its button shows while there is one):
  // none where the daemon has no ClaxBot or refuses this browser.
  if (answered("runs")) runsNow = block!.status.runs === 200 && Array.isArray(block!.runs?.runs) ? block!.runs.runs : [];
  runsAsk = runsNow ? Promise.resolve(runsNow) : getToken().then(t => fetch(`/api/bot/runs?artifact=${encodeURIComponent(id)}`, { headers: t ? { authorization: `Bearer ${t}` } : {}, cache: "no-store" }))
    .then(r => (r.ok ? r.json() : null)).then((b: { runs?: unknown } | null) => (Array.isArray(b?.runs) ? b.runs as RunSummary[] : []), () => []).then(r => (runsNow = r));
  if (block && typeof block.inbox === "boolean") {
    setInbox(block.inbox);
    if (block.inbox && answered("summary") && answered("questions")) {
      const summary = block.status.summary === 200 ? block.summary : null;
      const open = block.status.questions === 200 ? block.questions?.questions ?? null : null;
      if (summary) known.unread = summary.unread ?? 0;
      first = Promise.resolve({ summary, open });
    }
  }
}

/** Decides whether this browser has an inbox (as `inboxBrowser`, `owner.ts`:
 * a connected person's, or the owner's), where the block did not; for one
 * with an inbox, starts its first read. Settles once known: the view waits
 * for it beside its own data, so Inbox is drawn with the bar. */
export function decideInbox(me: Promise<Viewer | null> = getViewer()): Promise<void> {
  inboxAsk ??= known.inbox !== null
    ? Promise.resolve()
    : import("../owner").then(m => m.inboxBrowser(me as Promise<{ person?: boolean; person_revoked_at?: string | null } | null>)).then(setInbox, () => setInbox(false));
  return inboxAsk.then(() => {
    if (known.inbox && !first) {
      first = import("../q/api").then(m => Promise.all([
        m.inboxSummary().then(r => (r === "forbidden" ? null : r), () => null),
        m.listQuestions().then(r => (r === "forbidden" ? null : r.questions ?? null), () => null),
      ])).then(([summary, open]) => {
        if (summary && known.unread === null) known.unread = summary.unread ?? 0;
        return { summary, open };
      }, () => ({ summary: null, open: null }));
    }
  });
}

/** The inbox's first read, as the block holds it or as asked (`decideInbox`); null where none was started. */
export function firstInbox(): Promise<FirstInbox> | null { return first; }

/** ClaxBot for this browser as the block holds it (`GET /api/me/bot`'s
 * answer; null: none here), or undefined where the block does not say. */
export function botKnown(): MeBot | null | undefined {
  if (!answered("bot")) return undefined;
  return block!.status.bot === 200 ? block!.bot : null;
}

/** Clone's options for the artifact as known now: the block's, else the
 * answer asked as the page started once it is in (null: none, or it
 * failed); undefined while that is on its way. */
export function cloneKnown(): CloneOptions | null | undefined {
  if (answered("clone")) return block!.status.clone === 200 ? block!.clone : null;
  return cloneNow;
}
/** Clone's options as asked when the page started (null where it failed, or nothing was asked). */
export function cloneAsked(): Promise<CloneOptions | null> | null { return cloneAsk; }
/** The artifact's links to public Claxes, for the local owner's shell, as
 * known now (null: not that shell, or it failed; undefined: on its way). */
export function remotesKnown(): Remotes | null | undefined { return remotesNow; }
/** The same, as asked when the page started (null where nothing was asked). */
export function remotesAsked(): Promise<Remotes | null> | null { return remotesAsk; }
/** ClaxBot's runs of the artifact as known now (empty: none, or not
 * answered); undefined while they are on their way. */
export function runsKnown(): RunSummary[] | undefined { return runsNow; }
/** The same, as asked when the page started (null where nothing was asked). */
export function runsAsked(): Promise<RunSummary[]> | null { return runsAsk; }

/** Settles once what decides the feedback menu, Publish and ClaxBot's
 * runs button is in (an answer or a failure each), where the block did not
 * say: the view waits for it beside its own data, so they are drawn with
 * the bar. */
export function barDecided(): Promise<void> {
  return Promise.all([feedbackAsk, remotesAsk, runsAsk]).then(() => {}, () => {});
}
