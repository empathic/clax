// The capabilities a page reaches on its own origin, framed or top-level:
// `db`, `assets`, `user` and `room` call the site's `/_clax/api/…`
// (origin-client.ts) as whoever the site's cookie names. What `use()`
// resolves is decided here from the artifact's current declaration
// (`/_clax/api/artifact`) and the caller (`/_clax/api/whoami`), each read
// once per page: `db` and `room` when declared, `assets` when declared and
// the caller is the owner, `user` always (user.d.ts: its universal members
// need no declaration). A Clax shell framing the page (`shell`) adds only
// what the site cannot know: which documents changed (its event stream),
// and other people's names (`user.profiles`, `user.search`).
import { type Local, ORIGIN_CAPABILITIES, buildNamespace } from "../capabilities";
import type { RoomPart } from "../parts/types";
import { CapabilityError, type Rpc, isCapabilityError } from "../rpc";
import { assetsLocals } from "./assets";
import { avatarFor, colorFor } from "./avatar";
import { makeDb } from "./db";
import { type OriginClient, type WireDoc, originClient } from "./origin-client";
import { originRoom } from "./room-socket";

export type Whoami = { kind: string; id?: string; name?: string; level: string; can_edit: boolean };
type Declared = Record<string, Record<string, unknown> | undefined>;
type Shell = Pick<Rpc, "use" | "call" | "on">;
type Transport = Pick<Rpc, "call" | "on">;

/** What a page knows of its site, each read once: the declaration (null
 * when it could not be read) and the caller (null likewise). */
export type Site = { client: OriginClient; declared(): Promise<Declared | null>; whoami(): Promise<Whoami | null> };

export function siteOf(client: OriginClient): Site {
  let declared: Promise<Declared | null> | null = null;
  let who: Promise<Whoami | null> | null = null;
  return {
    client,
    declared: () => declared ??= client.call("artifact", "get", []).then(a => ((a as { capabilities?: Declared } | null)?.capabilities ?? {}), () => null),
    whoami: () => who ??= client.call("user", "whoami", []).then(w => w as Whoami, () => null),
  };
}

let page: Site | null = null;

/** `use(name)` for a capability of the origin's: its frozen namespace, or
 * null when this view does not get it; `undefined` for any other name.
 * `shell` is the framing shell's connection (false top-level). */
export async function originUse(name: string, shell: Shell | false, room: () => Promise<RoomPart>, site?: Site): Promise<unknown> {
  if (!ORIGIN_CAPABILITIES.includes(name)) return undefined;
  const s = site ?? (page ??= siteOf(originClient(new URL(location.origin))));
  const declared = await s.declared();
  const has = (n: string) => !!declared && Object.hasOwn(declared, n);
  const direct: Transport = { call: (ns, m, a) => s.client.call(ns, m, a), on: () => () => {} };
  switch (name) {
    case "db": {
      if (!has("db")) return null;
      const t = dbTransport(s.client, shell);
      return buildNamespace("db", t, makeDb(t) as unknown as Local);
    }
    case "assets":
      return has("assets") && (await s.whoami())?.kind === "owner" ? buildNamespace("assets", direct, assetsLocals(direct)) : null;
    case "room":
      return has("room") ? room().then(p => p.roomNamespace(originRoom(s.client, shell))) : null;
    default:
      return buildNamespace("user", direct, userLocals(s, declared ?? {}, shell));
  }
}

// ---- db ----

type Spec = { kind: "doc"; path: string } | { kind: "query"; collection: string; where: unknown[]; orderBy: string | null; desc: boolean; limit: number | null };

export const SNAPSHOT_DEBOUNCE_MS = 25;
export const MAX_SUBSCRIPTIONS = 64;
/** Refresh period for every subscription while a watching shell's event
 * stream is down, and the longest the polling ever waits (the contract's
 * "about 30 seconds"). */
export const POLL_MS = 30_000;
/** Refresh periods while no shell says what changed (top-level, or a frame
 * no Clax shell answers; the site has no change stream): the first after a
 * change, this page's own write, the window's focus or the page coming back
 * into view, then each after a poll that found nothing new. */
export const UNWATCHED_POLL_MS = [5_000, 10_000, 20_000, POLL_MS] as const;
/** Delays before retrying a refetch that failed `unavailable`: the first, then every later one. */
export const RETRY_MS = [5_000, 30_000] as const;

const parentOf = (path: string) => path.slice(0, path.lastIndexOf("/"));
const touches = (spec: Spec, path: string) => (spec.kind === "doc" ? spec.path === path : parentOf(path) === spec.collection);
const pathOf = (args: unknown[]) => String(args[0]);

/** The `db` namespace's connection (the calls and pushes of db.ts): reads
 * and writes go to the site; subscriptions refetch, debounced, on each change
 * the framing shell reports (`db/changed`, after `db.watch`), right after
 * this page's own writes, by polling while no shell watches
 * ([`UNWATCHED_POLL_MS`], slower while nothing changes) and every
 * [`POLL_MS`] while the shell's stream is down, never while the page is
 * hidden (it refetches as it comes back into view); a refetch the site could
 * not answer is retried after [`RETRY_MS`]. A snapshot is pushed as
 * `db/snapshot` when it differs from the subscription's last, a
 * subscription's end as `db/snapshot-error`. The site's validators (an
 * unchanged read is its 304, origin-client.ts) keep a poll cheap. */
export function dbTransport(client: OriginClient, shell: Shell | false): Transport {
  const listeners = new Map<string, Set<(d: unknown) => void>>();
  const emit = (topic: string, data: unknown) => { for (const fn of [...(listeners.get(topic) ?? [])]) fn(data); };
  const subs = new Map<string, Spec>();
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  // The latest fetch started per subscription: only it may deliver, so a slow
  // earlier fetch never overwrites a newer snapshot.
  const fetches = new Map<string, number>();
  let fetchSeq = 0;
  const retries = new Map<string, ReturnType<typeof setTimeout>>();
  const failures = new Map<string, number>();
  // The highest version delivered per document subscription: an older
  // result is never delivered after it (versions are never reused).
  const delivered = new Map<string, number>();
  // What each subscription last delivered (paths and versions), so a poll
  // that finds the same delivers nothing and counts as nothing new.
  const sigs = new Map<string, string>();
  let watched = false;
  let streamDown = false;
  let poll: ReturnType<typeof setTimeout> | null = null;
  let step = 0;
  const hidden = () => typeof document !== "undefined" && document.visibilityState === "hidden";

  const current = async (spec: Spec): Promise<WireDoc[]> => {
    if (spec.kind === "query") return (await client.call("db", "query", [spec])) as WireDoc[];
    const d = (await client.call("db", "get", [spec.path])) as WireDoc | null;
    return d ? [d] : [];
  };

  /** Refetches `sub`; true when it delivered something new. */
  async function push(sub: string): Promise<boolean> {
    const spec = subs.get(sub);
    if (!spec) return false;
    const n = ++fetchSeq;
    fetches.set(sub, n);
    const live = () => subs.get(sub) === spec && fetches.get(sub) === n;
    try {
      const docs = await current(spec);
      if (!live()) return false;
      failures.delete(sub);
      clearTimeout(retries.get(sub));
      retries.delete(sub);
      if (spec.kind === "doc" && docs.length) {
        if (docs[0].version < (delivered.get(sub) ?? 0)) return false;
        delivered.set(sub, docs[0].version);
      }
      const sig = docs.map(d => `${d.path}@${d.version}`).join("\n");
      if (sigs.get(sub) === sig) return false;
      sigs.set(sub, sig);
      emit("snapshot", { sub, docs });
      return true;
    } catch (e) {
      if (!live()) return false;
      const code = isCapabilityError(e) ? e.code : "unavailable";
      if (code === "unavailable") {
        // Retried after a backoff (and by any change before then).
        const k = (failures.get(sub) ?? 0) + 1;
        failures.set(sub, k);
        clearTimeout(retries.get(sub));
        retries.set(sub, setTimeout(() => { retries.delete(sub); void push(sub); }, RETRY_MS[Math.min(k, RETRY_MS.length) - 1]));
        return false;
      }
      drop(sub);
      emit("snapshot-error", { sub, code, message: e instanceof Error ? e.message : String(e) });
      return false;
    }
  }

  function schedule(sub: string): void {
    if (timers.has(sub)) return;
    timers.set(sub, setTimeout(() => { timers.delete(sub); void push(sub); }, SNAPSHOT_DEBOUNCE_MS));
  }

  /** Plans the next poll at the period that holds now, or none: none while
   * the page is hidden, a watching shell's stream is up, or nothing is
   * subscribed. */
  function repoll(): void {
    if (poll !== null) clearTimeout(poll);
    poll = null;
    const ms = !watched ? UNWATCHED_POLL_MS[step] : streamDown ? POLL_MS : 0;
    if (ms && subs.size && !hidden()) poll = setTimeout(() => void tick(), ms);
  }

  /** One poll: every subscription refetched; while nothing comes back new
   * the next one waits longer, up to [`POLL_MS`]. */
  async function tick(): Promise<void> {
    poll = null;
    const found = await Promise.all([...subs.keys()].map(push));
    step = found.includes(true) ? 0 : Math.min(step + 1, UNWATCHED_POLL_MS.length - 1);
    if (poll === null) repoll();
  }

  /** Polls at the shortest period again (a write, focus, the page in view). */
  function hurry(): void {
    step = 0;
    repoll();
  }

  if (typeof document !== "undefined") {
    // Hidden, nothing is polled; back in view, every subscription catches up.
    document.addEventListener("visibilitychange", () => {
      if (!hidden()) for (const sub of subs.keys()) schedule(sub);
      hurry();
    });
    addEventListener("focus", hurry);
  }

  function drop(sub: string): void {
    subs.delete(sub);
    fetches.delete(sub);
    failures.delete(sub);
    delivered.delete(sub);
    sigs.delete(sub);
    clearTimeout(timers.get(sub));
    timers.delete(sub);
    clearTimeout(retries.get(sub));
    retries.delete(sub);
    if (subs.size === 0) repoll();
  }

  /** This page wrote `path`: its matching subscriptions refetch now, and
   * the polling hurries again. */
  function echo(path: string): void {
    hurry();
    for (const [sub, spec] of subs) {
      if (!touches(spec, path)) continue;
      clearTimeout(timers.get(sub));
      timers.delete(sub);
      void push(sub);
    }
  }

  // A Clax shell around the page hears the daemon's event stream, which the
  // site has none of: once it watches, its reports replace the polling.
  if (shell) {
    void shell.use("db").then(g => {
      if (!g) return;
      shell.on("db", "changed", d => {
        const path = (d as { path?: unknown } | null)?.path;
        for (const [sub, spec] of subs) if (typeof path !== "string" || touches(spec, path)) schedule(sub);
      });
      shell.on("db", "stream", d => {
        streamDown = (d as { up?: unknown } | null)?.up !== true;
        repoll();
      });
      return shell.call("db", "watch", []).then(() => { watched = true; repoll(); });
    }).catch(() => {});
  }

  return {
    async call(_ns, method, args) {
      switch (method) {
        case "subscribe": {
          const sub = String(args[0]);
          if (!subs.has(sub) && subs.size >= MAX_SUBSCRIPTIONS) throw new CapabilityError("resource_exhausted", `at most ${MAX_SUBSCRIPTIONS} subscriptions per view`);
          drop(sub);
          subs.set(sub, args[1] as Spec);
          repoll();
          await push(sub);
          return null;
        }
        case "unsubscribe":
          drop(String(args[0]));
          return null;
        case "set": case "update": case "delete": {
          const r = await client.call("db", method, args);
          echo(pathOf(args));
          return r;
        }
        case "acquire": {
          const r = (await client.call("db", method, args)) as { acquired: boolean };
          if (r.acquired) echo(pathOf(args));
          return r;
        }
        default:
          return client.call("db", method, args);
      }
    },
    on(_ns, topic, fn) {
      let set = listeners.get(topic);
      if (!set) listeners.set(topic, (set = new Set()));
      set.add(fn);
      return () => { set!.delete(fn); };
    },
  };
}

// ---- user ----

const RANK: Record<string, number> = { public: 0, view: 1, interact: 2, admin: 3, owner: 4 };

/** The write level of shared documents at the root: the declared root rule's, else `interact`. */
export function rootWrite(declared: Declared): string {
  const rules = (declared.db as { rules?: unknown } | undefined)?.rules;
  if (Array.isArray(rules)) {
    const root = rules.find(r => r && typeof r === "object" && (r as { path?: unknown }).path === "") as { write?: unknown } | undefined;
    if (typeof root?.write === "string" && root.write in RANK) return root.write;
  }
  return "interact";
}

/** The `user` members (user.d.ts), from the site's `whoami`: every read
 * resolves, none rejects. `id` needs the declaration, names the `profile`
 * scope. Other people's names come from a framing Clax shell when there is
 * one (`profiles`, `search`); without one, `profiles` names only the viewer
 * and `search` finds no one. */
export function userLocals(s: Site, declared: Declared, shell: Shell | false): Local {
  const decl = Object.hasOwn(declared, "user");
  const scopes = (declared.user as { scopes?: unknown } | undefined)?.scopes;
  const prof = decl && Array.isArray(scopes) && scopes.includes("profile");
  let viaShell: Promise<boolean> | null = null;

  const me = async () => {
    const w = await s.whoami();
    const id = decl ? (w?.id ?? null) : null;
    const name = prof && id ? (w?.name ?? "") : "";
    const color = colorFor(id);
    return { id, name, avatarUrl: avatarFor(name, color), color, email: null, isOwner: w?.kind === "owner", canEdit: w?.can_edit === true };
  };

  async function profiles(input: unknown) {
    const list: unknown[] = typeof input === "string" ? [input] : Array.isArray(input) ? input : [];
    const m = await me();
    return Object.fromEntries([...new Set(list.filter((x): x is string => typeof x === "string"))].map(id => {
      const name = id === m.id ? m.name : "";
      const color = colorFor(id);
      return [id, { id, name, avatarUrl: avatarFor(name, color), color, email: null, isMe: id === m.id, guest: false }];
    }));
  }

  /** `method` answered by the framing shell when it serves `user`, else by `alone`. */
  const ask = (method: string, alone: (x: unknown) => Promise<unknown>) => async (x: unknown) => {
    if (shell && await (viaShell ??= shell.use("user").then(g => !!g, () => false))) {
      try { return await shell.call("user", method, [x]); } catch { /* answered alone */ }
    }
    return alone(x);
  };

  /** `r` (profiles by ID) with the viewer's own entry a guest when the site
   * says so: no one else's guest flag is known here. */
  const markGuest = async (r: unknown) => {
    const w = await s.whoami();
    const mine = decl && w?.id && r && typeof r === "object" ? (r as Record<string, unknown>)[w.id] : undefined;
    if (mine && typeof mine === "object") (r as Record<string, unknown>)[w!.id!] = { ...mine, guest: w!.kind === "guest" };
    return r;
  };
  const lookUp = ask("profiles", profiles);

  return {
    isOwner: async () => (await me()).isOwner,
    canEdit: async () => (await me()).canEdit,
    /** null when the site could not say who the viewer is (user.d.ts: the
     * platform told this page nothing). */
    async can(what: unknown) {
      const w = await s.whoami();
      if (!w) return null;
      if (what === "data.write") return (RANK[w?.level ?? ""] ?? -1) >= RANK[rootWrite(declared)];
      if (what === "files.write") return w?.can_edit === true;
      if (what === "assets.write") return w?.kind === "owner";
      return false;
    },
    me,
    id: async () => (await me()).id,
    name: async () => (await me()).name,
    avatarUrl: async () => {
      const m = await me();
      return prof && m.id ? m.avatarUrl : null;
    },
    email: async () => null,
    profiles: async (x: unknown) => markGuest(await lookUp(x)),
    search: ask("search", async () => []),
  } as unknown as Local;
}
