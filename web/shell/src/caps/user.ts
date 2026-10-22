// user.d.ts's lookups of other people in the shell: `profiles` and `search`,
// which a framed page's bridge asks here because its site serves no
// viewers' names. Everything about the viewer themselves (`me`, `isOwner`,
// `canEdit`, `can`, …) the bridge answers from its site's `whoami`
// (web/bridge/src/caps/origin.ts). Identity is the viewer's public ID (the
// cookie's viewer, never the cookie); names come from the viewers table and
// need scope "profile". `email` is always null and `guest` always false.
// Every read resolves; none rejects. A lazy chunk (registry.ts), loaded on
// the page's first lookup.
import { avatarFor, colorFor } from "../../../bridge/src/caps/avatar";
import { CapError } from "./errors";
import type { HandlerFactory, ViewerInfo } from "./host";

type Profile = { id: string; name: string; avatarUrl: string; color: string; email: null; isMe: boolean; guest: false };
type ApiViewer = { id: string; display_name: string | null };

const PUBLIC_ID = /^u_[0-9a-f]{22}$/;

/** Most public IDs one lookup request carries (the daemon's `MAX_LOOKUP_IDS`). */
export const LOOKUP_CHUNK = 64;
/** Most IDs one `profiles()` call looks up; the rest resolve unresolved. */
export const MAX_LOOKUP = 1024;
/** Longest query `search()` sends; a longer one matches no name. */
export const MAX_QUERY_CHARS = 60;
/** Most profiles `search()` resolves (user.d.ts). */
export const MAX_HITS = 8;
/** How long `search()` waits for a newer call before asking the daemon. */
export const SEARCH_DEBOUNCE_MS = 150;

export const userHandler: HandlerFactory = env => {
  // Without the declaration (user.d.ts): no names, no lookups.
  const declared = Object.prototype.hasOwnProperty.call(env.declared, "user");
  const scopes = (env.declared.user as { scopes?: unknown } | undefined)?.scopes;
  const profileScope = declared && Array.isArray(scopes) && scopes.includes("profile");
  const owner = env.token !== null;
  /** Names this page has resolved (public ID → name, "" when unnamed). */
  let cache = new Map<string, string>();
  /** Lookups in flight, per ID, so concurrent calls share one request. */
  let pending = new Map<string, Promise<void>>();
  const inflight = new Set<AbortController>();
  let disposed = false;
  let warned = false;
  /** The newest `search()`'s number; an older call's own result is dropped. */
  let searchGen = 0;
  /** The promise every call since the last settled search returns: the newest
   * call resolves it, so superseded calls settle with it, at the same moment. */
  let burst: { promise: Promise<Profile[]>; resolve(r: Profile[]): void } | null = null;
  /** The newest search's request, aborted when a newer call supersedes it. */
  let searchAc: AbortController | null = null;
  /** The frame document's number (`reset` starts a new one); a search started
   * in an older document never adds names to the current one. */
  let docEpoch = 0;
  const timers = new Set<{ id: ReturnType<typeof setTimeout>; done(): void }>();

  /** The viewer, or none when the lookup fails. */
  const viewer = (): Promise<ViewerInfo | null> => env.viewer().catch(() => null);

  const profile = (id: string, name: string, meId: string | null): Profile => {
    const shown = profileScope ? name : "";
    const color = colorFor(id);
    return { id, name: shown, avatarUrl: avatarFor(shown, color), color, email: null, isMe: id === meId, guest: false };
  };

  async function get(url: string, headers: Record<string, string> = {}, ac = new AbortController()): Promise<ApiViewer[] | null> {
    if (disposed || ac.signal.aborted) return null;
    inflight.add(ac);
    try {
      const r = await fetch(url, { headers, signal: ac.signal });
      if (!r.ok || disposed) return null;
      const body = (await r.json()) as { viewers?: unknown };
      if (disposed || !Array.isArray(body.viewers)) return null;
      return body.viewers.filter((v): v is ApiViewer => !!v && typeof v === "object" && typeof (v as ApiViewer).id === "string" && PUBLIC_ID.test((v as ApiViewer).id));
    } catch {
      return null;
    } finally {
      inflight.delete(ac);
    }
  }

  function remember(vs: ApiViewer[]): void {
    for (const v of vs) cache.set(v.id, typeof v.display_name === "string" ? v.display_name : "");
  }

  /** Looks up `ids` (valid public IDs) not yet known, sharing lookups in flight. */
  async function lookup(ids: string[]): Promise<void> {
    const waits: Promise<void>[] = [];
    const fresh: string[] = [];
    for (const id of ids) {
      if (cache.has(id)) continue;
      const p = pending.get(id);
      if (p) waits.push(p);
      else fresh.push(id);
    }
    const mine = pending;
    for (let i = 0; i < fresh.length; i += LOOKUP_CHUNK) {
      const chunk = fresh.slice(i, i + LOOKUP_CHUNK);
      const p = get(`/api/viewers?ids=${chunk.join(",")}`).then(vs => { if (vs && mine === pending) remember(vs); });
      for (const id of chunk) mine.set(id, p);
      waits.push(p.finally(() => { for (const id of chunk) if (mine.get(id) === p) mine.delete(id); }));
    }
    await Promise.all(waits);
  }

  async function profiles(input: unknown): Promise<Record<string, Profile>> {
    const list: unknown[] = typeof input === "string" ? [input] : Array.isArray(input) ? input : [];
    const ids = [...new Set(list.filter((x): x is string => typeof x === "string"))];
    const v = await viewer();
    const meId = declared && v ? v.publicId : null;
    if (profileScope && !disposed) {
      const wanted = ids.filter(id => PUBLIC_ID.test(id) && id !== meId);
      if (wanted.length > MAX_LOOKUP && !warned) {
        warned = true;
        console.warn(`user.profiles: ${wanted.length} IDs in one call; only the first ${MAX_LOOKUP} are looked up`);
      }
      await lookup(wanted.slice(0, MAX_LOOKUP));
    }
    // The viewer's own entry always carries their current name.
    const nameOf = (id: string) => (id === meId ? (v?.name ?? "") : (cache.get(id) ?? ""));
    return Object.fromEntries(ids.map(id => [id, profile(id, nameOf(id), meId)]));
  }


  /** Waits [`SEARCH_DEBOUNCE_MS`]; false when disposed meanwhile. */
  const pause = () => new Promise<boolean>(resolve => {
    const t = { id: setTimeout(() => { timers.delete(t); resolve(!disposed); }, SEARCH_DEBOUNCE_MS), done: () => resolve(false) };
    timers.add(t);
  });

  /** `search()` per user.d.ts: a call superseded by a newer one resolves with
   * the newer call's result, at the same moment; its own request is aborted
   * and never waited on. */
  function search(q: unknown): Promise<Profile[]> {
    const gen = ++searchGen;
    searchAc?.abort();
    const ac = new AbortController();
    searchAc = ac;
    let b = burst;
    if (!b) {
      let resolve!: (r: Profile[]) => void;
      const promise = new Promise<Profile[]>(r => { resolve = r; });
      b = burst = { promise, resolve };
    }
    const mine = b;
    void runSearch(q, () => gen === searchGen, ac, docEpoch).catch((): Profile[] => []).then(r => {
      if (gen !== searchGen) return;
      if (burst === mine) burst = null;
      if (searchAc === ac) searchAc = null;
      mine.resolve(r);
    });
    return mine.promise;
  }

  async function runSearch(q: unknown, current: () => boolean, ac: AbortController, epoch: number): Promise<Profile[]> {
    if (typeof q !== "string" || !owner || !profileScope || disposed) return [];
    const v = await viewer();
    if (!v) return [];
    const text = q.trim();
    if (!text) {
      const others = [...cache].filter(([id]) => id !== v.publicId).map(([id, name]) => profile(id, name, v.publicId));
      return [profile(v.publicId, v.name ?? "", v.publicId), ...others].filter(p => p.name).slice(0, MAX_HITS);
    }
    if ([...text].length > MAX_QUERY_CHARS) return [];
    // A newer call within the pause takes over; this one asks nothing.
    if (!(await pause()) || !current()) return [];
    const vs = await get(`/api/viewers?q=${encodeURIComponent(text)}`, { authorization: `Bearer ${env.token}` }, ac);
    if (!vs || epoch !== docEpoch) return [];
    remember(vs);
    return vs.map(x => profile(x.id, x.id === v.publicId ? (v.name ?? "") : (x.display_name ?? ""), v.publicId)).filter(p => p.name).slice(0, MAX_HITS);
  }

  return {
    async call(method, args) {
      switch (method) {
        case "profiles":
          return profiles(args[0]);
        case "search":
          return search(args[0]);
        default:
          throw new CapError("capability_removed", `user.${String(method)} is not part of this runtime`);
      }
    },
    reset() {
      // Resolved names live for the page's lifetime (user.d.ts).
      cache = new Map();
      pending = new Map();
      // The old document's searches end: nothing of theirs reaches the new one.
      docEpoch++;
      searchGen++;
      searchAc?.abort();
      searchAc = null;
      burst?.resolve([]);
      burst = null;
    },
    dispose() {
      disposed = true;
      for (const t of timers) { clearTimeout(t.id); t.done(); }
      timers.clear();
      for (const ac of inflight) ac.abort();
      inflight.clear();
      cache = new Map();
      pending = new Map();
    },
  };
};
