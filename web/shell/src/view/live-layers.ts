// The live layers on the shell's side (spec 2026-10-11-clax-dev-hosted-design
// §16.3, §16.5): what other people on the artifact are typing, drafting,
// pointing at and selecting, folded from the live socket's batches
// (`/api/artifacts/<id>/live`); the senders that put this viewer's typing,
// drafts and pointer on the socket at the rates the daemon expects; and the
// peers as the content frame draws them, with opaque keys and colours and
// never a name. Loaded after the first paint, off the artifact entry.

import type { FramePeer, LivePoint, LiveSelection } from "../../../bridge/src/protocol";
import { after } from "../clock";

/** Colours of the live palette by index, as drawn in the content frame over
 * the page's own colours (one set for both themes; the shell's chrome uses
 * `--live-<n>`, `liveVar`). */
export const LIVE_PALETTE: readonly string[] = ["#2f62d0", "#8a3fd1", "#0a757b", "#be2f7a", "#975a00", "#6b6500", "#33708f", "#a3445a"];
/** The CSS variable for live colour `c` in the shell's chrome (theme.css sets one per theme). */
export const liveVar = (c: number): string => `var(--live-${((c % LIVE_PALETTE.length) + LIVE_PALETTE.length) % LIVE_PALETTE.length})`;
/** Live colour `c` as the frame draws it. */
export const liveHex = (c: number): string => LIVE_PALETTE[((c % LIVE_PALETTE.length) + LIVE_PALETTE.length) % LIVE_PALETTE.length]!;

/** How long after a person's last keystroke they still read as typing, in
 * ms; after that their draft stays, without "is typing…". */
export const TYPING_FRESH_MS = 4000;
/** The least time between two drafts this page sends, in ms. */
export const DRAFT_EVERY_MS = 150;
/** Longest draft sent, in characters (the daemon's `MAX_DRAFT_CHARS`). */
export const DRAFT_MAX = 2000;
/** Waits before reopening a closed live socket, by attempt, in ms. */
export const RECONNECT_MS = [500, 1000, 2000, 5000, 10_000] as const;

export type LiveCursor = { version: number; file: string; anchor: LivePoint | null; selection: LiveSelection | null };
/** A sender in a batch: the parts left out are unchanged; null is none. */
export type LiveSender = {
  person_id: string;
  color: number;
  cursor?: LiveCursor | null;
  typing?: { thread_id: string | null } | null;
  draft?: { thread_id: string | null; text: string } | null;
};
/** `present`: how many other people have the artifact's live socket open. */
export type LiveBatch = { t: "live"; senders: LiveSender[]; drop: string[]; others: number; present: number };
export type LiveWelcome = { t: "welcome"; person_id: string; color: number };
/** What this page sends on the live socket. */
export type LiveOut =
  | { t: "open"; thread_id: string | null }
  | { t: "typing"; thread_id: string | null }
  | { t: "draft"; thread_id: string | null; text: string }
  | ({ t: "cursor" } & LiveCursor);

/** A sender's whole state, every part filled in. */
export type FullSender = Required<LiveSender>;
export type Peer = {
  sender: FullSender;
  /** When its typing or draft last changed here (ms, this page's clock). */
  typedAt: number;
};
/** Other people's live state as this page knows it, by person ID, how many
 * more are active but not detailed, and how many other people are present. */
export type LayerState = { peers: Record<string, Peer>; others: number; present: number };
export const emptyLayer = (): LayerState => ({ peers: {}, others: 0, present: 0 });

const isObj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const isThread = (v: unknown) => v === null || (typeof v === "string" && /^[0-9A-HJKMNP-TV-Z]{26}$/.test(v));
const isColor = (v: unknown): v is number => typeof v === "number" && Number.isInteger(v) && v >= 0 && v < 64;

/** The frame `raw` holds when it is one the daemon sends (a welcome or a batch), else null. */
export function checkLiveFrame(raw: unknown): LiveWelcome | LiveBatch | null {
  if (typeof raw !== "string" || raw.length > 4_000_000) return null;
  let f: unknown;
  try { f = JSON.parse(raw); } catch { return null; }
  if (!isObj(f)) return null;
  if (f.t === "welcome") return typeof f.person_id === "string" && isColor(f.color) ? { t: "welcome", person_id: f.person_id, color: f.color } : null;
  if (f.t !== "live" || !Array.isArray(f.senders) || !Array.isArray(f.drop) || typeof f.others !== "number") return null;
  const senders: LiveSender[] = [];
  for (const s of f.senders) {
    if (!isObj(s) || typeof s.person_id !== "string" || !isColor(s.color)) return null;
    if ("typing" in s && s.typing !== null && !(isObj(s.typing) && isThread(s.typing.thread_id))) return null;
    if ("draft" in s && s.draft !== null && !(isObj(s.draft) && isThread(s.draft.thread_id) && typeof s.draft.text === "string")) return null;
    if ("cursor" in s && s.cursor !== null && !(isObj(s.cursor) && typeof s.cursor.version === "number" && typeof s.cursor.file === "string")) return null;
    senders.push(s as LiveSender);
  }
  if (!f.drop.every(d => typeof d === "string")) return null;
  const present = typeof f.present === "number" && f.present >= 0 ? f.present : 0;
  return { t: "live", senders, drop: f.drop as string[], others: f.others, present };
}

/**
 * `s` with batch `b` received at `now`: each sender's parts merged over what
 * is known (a part left out is unchanged; a new sender's missing parts are
 * empty), dropped people removed.
 */
export function applyBatch(s: LayerState, b: LiveBatch, now: number): LayerState {
  const peers = { ...s.peers };
  for (const id of b.drop) delete peers[id];
  for (const x of b.senders) {
    const was = peers[x.person_id];
    const base: FullSender = was?.sender ?? { person_id: x.person_id, color: x.color, typing: null, draft: null, cursor: null };
    const wrote = "typing" in x || "draft" in x;
    peers[x.person_id] = {
      sender: {
        person_id: x.person_id,
        color: x.color,
        typing: "typing" in x ? (x.typing ?? null) : base.typing,
        draft: "draft" in x ? (x.draft ?? null) : base.draft,
        cursor: "cursor" in x ? (x.cursor ?? null) : base.cursor,
      },
      typedAt: wrote || was === undefined ? now : was.typedAt,
    };
  }
  return { peers, others: b.others, present: b.present };
}

/** Someone writing in a thread or a new comment. */
export type Writer = {
  person_id: string;
  color: number;
  /** The draft so far ("" when only typing has been seen). */
  text: string;
  /** Typed within TYPING_FRESH_MS. */
  typing: boolean;
};

/** The people writing in thread `threadId` (null: a new comment) at `now`, longest-writing first. */
export function writersIn(s: LayerState, threadId: string | null, now: number): Writer[] {
  return Object.values(s.peers)
    .flatMap(({ sender, typedAt: at }) => {
      const draft = sender.draft?.thread_id === threadId ? sender.draft.text : "";
      const typing = sender.typing !== null && sender.typing.thread_id === threadId && now - at < TYPING_FRESH_MS;
      if (!typing && draft === "") return [];
      return [{ at, w: { person_id: sender.person_id, color: sender.color, text: draft, typing } }];
    })
    .sort((a, b) => a.at - b.at || a.w.person_id.localeCompare(b.w.person_id))
    .map(x => x.w);
}

/** Whether anyone's "is typing" is still fresh at `now`, so the clock should tick. */
export const anyTyping = (s: LayerState, now: number): boolean => Object.values(s.peers).some(p => p.sender.typing !== null && now - p.typedAt < TYPING_FRESH_MS);

/** "Mia is typing…", "Mia and Ben are typing…", "Mia and 2 others are typing…". */
export function typingLine(names: string[]): string {
  if (!names.length) return "";
  if (names.length === 1) return `${names[0]} is typing…`;
  if (names.length === 2) return `${names[0]} and ${names[1]} are typing…`;
  return `${names[0]} and ${names.length - 1} others are typing…`;
}

/**
 * Opaque keys for people in one frame document: `k1`, `k2`… in the order
 * people are first drawn, never derived from an ID. `reset` starts over for
 * a new document.
 */
export class FrameKeys {
  private readonly byPerson = new Map<string, string>();
  private readonly byKey = new Map<string, string>();
  private next = 1;

  key(personId: string): string {
    let k = this.byPerson.get(personId);
    if (k === undefined) {
      k = `k${this.next++}`;
      this.byPerson.set(personId, k);
      this.byKey.set(k, personId);
    }
    return k;
  }

  /** The person behind key `k`, if this document was given it. */
  person(k: string): string | undefined {
    return this.byKey.get(k);
  }

  reset(): void {
    this.byPerson.clear();
    this.byKey.clear();
    this.next = 1;
  }
}

/** The pointers and selections to draw in a frame showing `file` of `version`: those made in the same file and version. */
export function framePeers(s: LayerState, version: number, file: string | null, keys: FrameKeys): FramePeer[] {
  if (file === null) return [];
  return Object.values(s.peers).flatMap(({ sender }) => {
    const c = sender.cursor;
    if (!c || c.version !== version || c.file !== file) return [];
    return [{ key: keys.key(sender.person_id), color: liveHex(sender.color), anchor: c.anchor, selection: c.selection }];
  });
}

/** A name tag over the frame: where the frame drew a peer's pointer, with the name the shell knows them by. */
export type PeerTag = { key: string; name: string; color: number; x: number; y: number };

/** The tags for the pointers the frame drew (`at`), named through `name`. */
export function peerTags(s: LayerState, at: readonly { key: string; x: number; y: number }[], keys: FrameKeys, name: (personId: string) => string): PeerTag[] {
  return at.flatMap(a => {
    const pid = keys.person(a.key);
    const p = pid === undefined ? undefined : s.peers[pid];
    return p ? [{ key: a.key, name: name(p.sender.person_id), color: p.sender.color, x: a.x, y: a.y }] : [];
  });
}

export interface Timers {
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

/** The shell's clock (`clock.ts`), which the browser tests advance. */
const realTimers: Timers = {
  setTimeout: (fn, ms) => after(ms, fn),
  clearTimeout: h => (h as () => void)(),
};

/**
 * Puts this page's typing and drafts on the socket: `typing` at the first
 * keystroke in a box, then the draft text at most every DRAFT_EVERY_MS
 * while it changes (the last text always goes), and an empty draft when the
 * box is cleared, posted or cancelled.
 */
export class DraftSender {
  /** The text last given per box (key: thread ID, or "" for a new comment). */
  private readonly texts = new Map<string, string>();
  /** The boxes whose state the daemon holds (typing or a draft was sent). */
  private readonly told = new Set<string>();
  private timer: unknown = null;
  private due = new Set<string>();

  constructor(private readonly send: (m: LiveOut) => void, private readonly timers: Timers = realTimers, private readonly every = DRAFT_EVERY_MS) {}

  private static key = (threadId: string | null): string => threadId ?? "";
  private static thread = (key: string): string | null => (key === "" ? null : key);

  /** The box of `threadId` (null: a new comment) now holds `text`. */
  input(threadId: string | null, text: string): void {
    const key = DraftSender.key(threadId);
    if (text.trim() === "") { this.clear(threadId); return; }
    const clipped = text.slice(0, DRAFT_MAX);
    if (this.texts.get(key) === clipped) return;
    this.texts.set(key, clipped);
    if (!this.told.has(key)) {
      this.told.add(key);
      this.send({ t: "typing", thread_id: threadId });
    }
    this.due.add(key);
    this.timer ??= this.timers.setTimeout(() => this.fire(), this.every);
  }

  private fire(): void {
    this.timer = null;
    const due = [...this.due];
    this.due.clear();
    for (const key of due) {
      const text = this.texts.get(key);
      if (text !== undefined) this.send({ t: "draft", thread_id: DraftSender.thread(key), text });
    }
  }

  /** The box of `threadId` was emptied, posted or cancelled. */
  clear(threadId: string | null): void {
    const key = DraftSender.key(threadId);
    this.texts.delete(key);
    this.due.delete(key);
    if (this.told.delete(key)) this.send({ t: "draft", thread_id: threadId, text: "" });
  }

  /** Sends every box's draft again (a reopened socket's daemon holds none). */
  resend(): void {
    this.told.clear();
    for (const [key, text] of this.texts) {
      this.told.add(key);
      this.send({ t: "draft", thread_id: DraftSender.thread(key), text });
    }
  }

  /** Stops the pending send. */
  stop(): void {
    if (this.timer !== null) this.timers.clearTimeout(this.timer);
    this.timer = null;
    this.due.clear();
  }
}

/** A WebSocket as the live socket uses it (a test gives a fake). */
export interface SocketLike {
  readyState: number;
  onopen: ((e: unknown) => void) | null;
  onmessage: ((e: { data: unknown }) => void) | null;
  onclose: ((e: { code: number; reason?: string }) => void) | null;
  send(data: string): void;
  close(code?: number, reason?: string): void;
}

export type LiveHandlers = {
  welcome(w: LiveWelcome): void;
  batch(b: LiveBatch): void;
  /** The socket opened (again): what the daemon held of this page is gone. */
  opened(): void;
  /** The socket closed; `final` when it will not reopen (no seat, the artifact deleted). */
  closed(final: boolean): void;
  /** The daemon closed it for want of a live seat (4429): `code` is why,
   * `no_live_seat` or `seat_idle`. It reopens on `resume`. */
  unseated?(code: string): void;
};

/**
 * The live socket of one artifact view: opened on `start`, reopened after a
 * close with the waits in RECONNECT_MS, except after a close the daemon
 * means for good (4429 no seat, 4410 deleted, 1008 overrun). What is sent
 * while it is not open is dropped; `opened` lets the owner send its state again.
 * `pause` closes it until `resume` (a spectator holds no live socket), and
 * `resume` opens it again after a 4429.
 */
export class LiveSocket {
  private ws: SocketLike | null = null;
  private attempt = 0;
  private stopped = false;
  private paused = false;
  private open = false;

  constructor(
    private readonly url: string,
    private readonly on: LiveHandlers,
    private readonly make: (url: string) => SocketLike = u => new WebSocket(u) as unknown as SocketLike,
    private readonly timers: Timers = realTimers,
  ) {}

  start(): void {
    if (this.ws || this.stopped || this.paused) return;
    const s = this.make(this.url);
    this.ws = s;
    s.onopen = () => {
      if (this.ws !== s) return;
      this.open = true;
      this.attempt = 0;
      this.on.opened();
    };
    s.onmessage = e => {
      if (this.ws !== s) return;
      const f = checkLiveFrame(e.data);
      if (f?.t === "welcome") this.on.welcome(f);
      else if (f) this.on.batch(f);
    };
    s.onclose = e => {
      if (this.ws !== s) return;
      this.ws = null;
      this.open = false;
      const final = this.stopped || e.code === 4429 || e.code === 4410 || e.code === 1008;
      this.on.closed(final);
      if (e.code === 4429 && !this.stopped) this.on.unseated?.(e.reason === "seat_idle" ? "seat_idle" : "no_live_seat");
      if (final) return;
      const wait = RECONNECT_MS[Math.min(this.attempt++, RECONNECT_MS.length - 1)];
      this.timers.setTimeout(() => this.start(), wait);
    };
  }

  send(m: LiveOut): void {
    if (this.open && this.ws) this.ws.send(JSON.stringify(m));
  }

  /** Closes the socket until `resume`. */
  pause(): void {
    this.paused = true;
    const s = this.ws;
    this.ws = null;
    this.open = false;
    s?.close(1000, "spectating");
  }

  /** Opens the socket again (after `pause` or a 4429), unless stopped. */
  resume(): void {
    this.paused = false;
    this.attempt = 0;
    this.start();
  }

  stop(): void {
    this.stopped = true;
    this.ws?.close(1000, "left");
    this.ws = null;
    this.open = false;
  }
}

/** The live socket's URL for artifact `id` on this shell's origin. */
export const liveUrl = (id: string, loc: { protocol: string; host: string } = location): string =>
  `${loc.protocol === "https:" ? "wss" : "ws"}://${loc.host}/api/artifacts/${encodeURIComponent(id)}/live`;
