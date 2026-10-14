// Spectator mode (spec 2026-10-11-clax-dev-hosted-design §16.1, §16.2),
// loaded off the artifact entry once the daemon refuses this viewer a live
// seat: the view holds no live topics, polls the shared snapshot every 5 s
// while the page is visible, swaps in what it says (threads, presence,
// working, a newer version), and offers Join live once a seat this viewer
// may take is free. A seat released for want of input is taken again at the
// viewer's next input (a press, a key, a pointer move, a scroll) or when the
// page shows again.
import type { Artifact, Attention, Version } from "../api";
import type { Thread, Viewer } from "../threads";
import type { Loaded, ViewState } from "./artifact-controller";
import type { PresenceView } from "./presence-model";
import type { ThreadSync } from "./thread-sync";
export { default as SpectatorBanner } from "../ui/SpectatorBanner.svelte";

/** How often a visible spectator's page polls the snapshot, in ms. */
export const SNAPSHOT_POLL_MS = 5000;
/** The least time between two Join live asks the viewer's input makes, in ms. */
export const INPUT_JOIN_GAP_MS = 2000;
/** The input that takes a seat released for want of input again. */
const IDLE_INPUTS = ["pointerdown", "pointermove", "keydown", "wheel", "touchstart"] as const;


/** `GET /api/artifacts/<id>/snapshot`: the artifact as any person reads it,
 * the same for every spectator. */
export type Snapshot = {
  artifact: Artifact;
  versions: Version[];
  threads: Thread[];
  presence: PresenceView[];
  viewing: number;
  seats_free: number;
  guest_seats_free: number;
  built_at: number;
};

/** This page's ID in its snapshot requests, which counts an anonymous viewer as viewing. */
const TAB = Math.random().toString(36).slice(2, 14);

/** The artifact's snapshot, or null on failure (the next poll tries again). */
export async function getSnapshot(aid: string): Promise<Snapshot | null> {
  try {
    const r = await fetch(`/api/artifacts/${aid}/snapshot?tab=${TAB}`, { cache: "no-store" });
    if (!r.ok) return null;
    return (await r.json()) as Snapshot;
  } catch {
    return null;
  }
}

/** Whether a viewer is a guest (a named viewer who is not a connected
 * person), whose seats come from the guests' share. The owner is never a
 * spectator. */
export function isGuest(me: Viewer | null): boolean {
  return !(me as { person?: boolean } | null)?.person;
}

/** Whether `snap` offers Join live: a seat this viewer may take is free. */
export function joinOffered(snap: Pick<Snapshot, "seats_free" | "guest_seats_free">, guest: boolean): boolean {
  return (guest ? snap.guest_seats_free : snap.seats_free) > 0;
}

/** What the poll drives: the artifact view's controller. */
export type SpectatorView = {
  readonly id: string;
  /** The viewer, for which seats it may take. */
  me(): Viewer | null;
  /** The code the seat was refused with, or null once the view is seated. */
  code(): string | null;
  /** Takes a snapshot in, with whether it offers Join live. */
  takeSnapshot(snap: Snapshot, joinable: boolean): void;
  joinLive(): void;
};

/** Polls `v`'s snapshot while it spectates; the returned function stops. */
export function watchSnapshots(v: SpectatorView, doc: Document = document): () => void {
  let stopped = false;
  let inFlight = false;
  const poll = async () => {
    if (stopped || inFlight || doc.visibilityState !== "visible") return;
    inFlight = true;
    try {
      const snap = await getSnapshot(v.id);
      if (!stopped && snap && snap.artifact.id === v.id) v.takeSnapshot(snap, joinOffered(snap, isGuest(v.me())));
    } finally {
      inFlight = false;
    }
  };
  void poll();
  const timer = setInterval(() => void poll(), SNAPSHOT_POLL_MS);
  // A seat released for want of input comes back with the next input.
  let lastJoin = -Infinity;
  const onInput = () => {
    if (v.code() !== "seat_idle" || Date.now() - lastJoin < INPUT_JOIN_GAP_MS) return;
    lastJoin = Date.now();
    v.joinLive();
  };
  const onVisible = () => {
    if (doc.visibilityState !== "visible") return;
    onInput();
    void poll();
  };
  doc.addEventListener("visibilitychange", onVisible);
  for (const t of IDLE_INPUTS) doc.addEventListener(t, onInput, { capture: true, passive: true });
  return () => {
    stopped = true;
    clearInterval(timer);
    doc.removeEventListener("visibilitychange", onVisible);
    for (const t of IDLE_INPUTS) doc.removeEventListener(t, onInput, { capture: true });
  };
}

/** What spectator mode needs of the controller (`ArtifactController`'s own). */
export type SpectatorHost = {
  readonly id: string;
  readonly state: { get(): ViewState };
  set(patch: Partial<ViewState>): void;
  readonly threadLoad: ThreadSync;
  readonly pinnedVersion: number | null;
  latestKnown: number;
  loaded(d: Loaded, attention: Attention | null): void;
  joinLive(): void;
};

/** Polls `c`'s snapshot while it spectates; the returned function stops. */
export function spectate(c: SpectatorHost): () => void {
  return watchSnapshots({
    id: c.id,
    me: () => c.state.get().me,
    code: () => c.state.get().spectator?.code ?? null,
    takeSnapshot: (snap, joinable) => takeSnapshot(c, snap, joinable),
    joinLive: () => c.joinLive(),
  });
}

/** A spectator's snapshot: its threads, presence and working replace the
 * view's, and a newer version is swapped in (on a view of the latest) or
 * offered (on a pinned one). */
export function takeSnapshot(c: SpectatorHost, snap: Pick<Snapshot, "artifact" | "versions" | "threads" | "presence"> & { viewing?: number }, joinable: boolean): void {
  const s = c.state.get();
  const sp = s.spectator;
  if (!sp || snap.artifact.id !== c.id) return;
  c.threadLoad.begin()(snap.threads);
  c.set({ presence: snap.presence, working: snap.artifact.working ?? [], spectator: { ...sp, joinable }, ...typeof snap.viewing === "number" && { viewing: snap.viewing } });
  const n = snap.artifact.current_version;
  const data = c.state.get().data;
  if (n <= c.latestKnown || !data) return;
  if (c.pinnedVersion === null) c.loaded({ ...data, artifact: snap.artifact, versions: snap.versions }, c.state.get().attention);
  else { c.latestKnown = n; c.set({ newer: n }); }
}
