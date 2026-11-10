// Browser confirmations (docs/contract.md, "Browser confirmations"): while
// an agent session is live, a decision that matters made with the daemon
// token waits for the owner to click Confirm in their browser. The
// owner's inbox shows each open one as a card, and every page of theirs
// as a toast (`ConfirmToasts.svelte`), listing the agent sessions live now.
// The feed follows the owner's `inbox` topic (its `confirmation` events)
// through the inbox feed's subscription (`InboxFeed.hear`), and fetches
// the open ones each time the topic goes live or resyncs. The daemon
// publishes no event when one expires, so the feed reads the open ones
// again as the earliest falls due, and one gone then has expired. A
// browser that is not the owner's is refused the list (403) and stops.
import { after, wall } from "../clock";
import { browserCall, Refused } from "../held";
import type { StreamEvent } from "../stream";
import type { Assertion } from "./passkey";

/** An agent session live as the confirmation was read. */
export type ConfirmAgent = { name: string | null; harness: string; handle: string; cwd: string; started_at: string };

/** A confirmation as `GET /api/confirmations` gives it. */
export type ConfirmationView = {
  id: string;
  status: "open" | "approved" | "declined" | "expired";
  /** The decision in words: the card reads "Confirm: <words>". */
  words: string;
  created_at: string;
  expires_at: string;
  confirm_url: string;
  agents: ConfirmAgent[];
  /** The decision as recorded: its `kind` names it. */
  decision?: { kind?: string };
};

/** Whether `c` adds a passkey: a registration the daemon holds, and adds once `c` is approved. */
export const registersPasskey = (c: ConfirmationView) => c.decision?.kind === "register_passkey";

/** The cards to show for `open`: a passkey registration is answered on
 * the setup line of the first other card (`carriesSetup`), when there is
 * one, rather than as a card of its own. */
export const cardsOf = (open: ConfirmationView[]) => open.some(c => !registersPasskey(c)) ? open.filter(c => !registersPasskey(c)) : open;

/** Whether card `c` carries the line for a passkey registration waiting in `open`. */
export const carriesSetup = (open: ConfirmationView[], c: ConfirmationView) => open.some(registersPasskey) && open.find(x => !registersPasskey(x))?.id === c.id;

/** The open confirmations, oldest first; "forbidden" for a browser that is not the owner's. */
export async function listConfirmations(): Promise<ConfirmationView[] | "forbidden"> {
  try {
    const b = await browserCall<{ confirmations?: unknown }>("/api/confirmations");
    return Array.isArray(b.confirmations) ? (b.confirmations as ConfirmationView[]) : [];
  } catch (e) {
    if (e instanceof Refused && (e.status === 403 || e.status === 404)) return "forbidden";
    throw e;
  }
}

/** Approves (`approve`) or declines confirmation `id`, with a passkey's
 * `assertion` where passkeys are required: it as it is after. A refusal
 * throws `Refused`. */
export async function answerConfirmation(id: string, approve: boolean, assertion?: Assertion): Promise<ConfirmationView> {
  const b = await browserCall<{ confirmation: ConfirmationView }>(`/api/confirmations/${encodeURIComponent(id)}`, "POST", assertion ? { approve, assertion } : { approve }, undefined, true);
  return b.confirmation;
}

/** What a person reads for an agent session: its name, else its harness and handle, and where it runs. */
export function agentLabel(a: ConfirmAgent): string {
  const who = a.name ?? `${a.harness} ${a.handle}`;
  return a.cwd ? `${who} in ${a.cwd}` : who;
}

/** What the feed follows: the inbox feed, or anything that hands on its topic's events. */
type Source = { hear(f: (e: StreamEvent) => void): () => void };

/** How long after a confirmation's time the feed asks the daemon, whose clock decides. */
const EXPIRY_SLACK_MS = 1000;
/** How long until it asks again about one this browser's clock calls due and the daemon still lists. */
const RECHECK_MS = 30_000;
const dueAt = (c: ConfirmationView) => Date.parse(c.expires_at);

/** The page's open confirmations, following the owner's `inbox` topic. */
export class ConfirmationFeed {
  declare private s: { open: ConfirmationView[]; listed: number };
  declare private unwatch: (() => void) | null;
  declare private unexpire: (() => void) | null;
  declare private changed: Set<() => void>;
  declare private ended: Set<(c: ConfirmationView) => void>;
  declare private readonly list: typeof listConfirmations;

  constructor(opts: { list?: typeof listConfirmations } = {}) {
    const s = $state({ open: [] as ConfirmationView[], listed: 0 });
    this.s = s;
    this.unwatch = null;
    this.unexpire = null;
    this.changed = new Set();
    this.ended = new Set();
    this.list = opts.list ?? listConfirmations;
  }

  /** The open confirmations, oldest first. */
  get open(): ConfirmationView[] { return this.s.open; }

  /** Whether the page lists the open ones itself (the inbox does), so the toasts need not. */
  get listed(): boolean { return this.s.listed > 0; }

  /** Marks the open ones listed on the page until the returned function runs. */
  listHere(): () => void {
    this.s.listed++;
    return () => { this.s.listed--; };
  }

  /** Calls `f` with each confirmation heard answered or expired, until the returned function runs. */
  onEnded(f: (c: ConfirmationView) => void): () => void {
    this.ended.add(f);
    return () => { this.ended.delete(f); };
  }
  private set open(v: ConfirmationView[]) {
    this.s.open = v;
    this.awaitExpiry();
    for (const f of [...this.changed]) f();
  }

  /** Reads the open ones again once the earliest falls due. */
  private awaitExpiry(): void {
    this.unexpire?.();
    this.unexpire = null;
    const due = Math.min(...this.s.open.map(dueAt).filter(Number.isFinite));
    if (!Number.isFinite(due)) return;
    const wait = due - wall() + EXPIRY_SLACK_MS;
    this.unexpire = after(wait > 0 ? wait : RECHECK_MS, () => { this.unexpire = null; void this.refresh(); }, "confirmation-expiry");
  }

  /** Calls `f` after each change of `open`, until the returned function runs. */
  listen(f: () => void): () => void {
    this.changed.add(f);
    return () => { this.changed.delete(f); };
  }

  /** Follows the inbox topic through `source` (once), fetching when it goes live. */
  start(source: Source): void {
    this.unwatch ??= source.hear(e => this.on(e));
  }

  /** Stops following the topic. */
  stop(): void {
    this.unwatch?.();
    this.unwatch = null;
    this.unexpire?.();
    this.unexpire = null;
  }

  /** Fetches the open ones; one listed before, due by now and gone has
   * expired, and is heard ended so. A browser that is not the owner's
   * stops following. */
  async refresh(): Promise<void> {
    const r = await this.list().catch(() => null);
    if (r === "forbidden") { this.stop(); return; }
    if (!Array.isArray(r)) return;
    const gone = this.s.open.filter(x => dueAt(x) <= wall() && !r.some(y => y.id === x.id));
    this.open = r;
    for (const c of gone) for (const f of [...this.ended]) f({ ...c, status: "expired" });
  }

  /** Takes a confirmation as it is now: an open one is listed (or
   * replaced), an answered or expired one leaves the list. */
  take(c: ConfirmationView): void {
    const rest = this.s.open.filter(x => x.id !== c.id);
    this.open = c.status === "open" ? [...rest, c].sort((a, b) => a.created_at.localeCompare(b.created_at)) : rest;
    if (c.status !== "open") for (const f of [...this.ended]) f(c);
  }

  private on(e: StreamEvent): void {
    if (e.type === "confirmation") {
      const c = e.confirmation as ConfirmationView | undefined;
      if (c?.id) this.take(c);
    } else if (e.type === "ready" || e.type === "resync") void this.refresh();
    else if (e.type === "refused") this.stop();
  }
}

let feed: ConfirmationFeed | null = null;

/** The page's one confirmation feed, which the inbox's cards and the toasts share. */
export const confirmations = () => (feed ??= new ConfirmationFeed());
