// user.d.ts in the shell. Identity is the viewer's public ID (the cookie's
// viewer, never the cookie); names come from the viewers table and need scope
// "profile". Artifax has no emails and no guests: `email` is always null and
// `guest` always false. Every read resolves; none rejects.
import type { Declared } from "./availability";
import { CapError } from "./errors";
import type { CapEnv, HandlerFactory, ViewerInfo } from "./host";

type Profile = { id: string; name: string; avatarUrl: string; color: string; email: null; isMe: boolean; guest: false };
type ApiViewer = { id: string; display_name: string | null };

const PALETTE = ["#2b5fd9", "#c2410c", "#15803d", "#7c3aed", "#be185d", "#0e7490", "#a16207", "#4d7c0f"];
const NEUTRAL = "#6b7280";
const RANK: Record<string, number> = { view: 0, interact: 1, admin: 2, owner: 3 };
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

/** A stable color per ID, readable under white initials in both themes; neutral without an ID. */
export function colorFor(id: string | null): string {
  if (!id) return NEUTRAL;
  let h = 0;
  for (const c of id) h = (Math.imul(h, 31) + c.charCodeAt(0)) >>> 0;
  return PALETTE[h % PALETTE.length];
}

/** An initials avatar on `color` as a data: URL (`?` when there is no name). */
export function avatarFor(name: string, color: string): string {
  const letters = name.trim().split(/\s+/).filter(Boolean).slice(0, 2).map(w => [...w][0].toUpperCase()).join("") || "?";
  const text = letters.replace(/[&<>"']/g, c => `&#${c.charCodeAt(0)};`);
  const fill = /^#[0-9a-f]{6}$/i.test(color) ? color : NEUTRAL;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><circle cx="32" cy="32" r="32" fill="${fill}"/><text x="32" y="41" font-family="system-ui,sans-serif" font-size="26" font-weight="600" text-anchor="middle" fill="#ffffff">${text}</text></svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

/** The write level of shared documents at the root: the declared root rule's, else `interact`. */
export function rootWrite(declared: Declared): string {
  const rules = (declared.db as { rules?: unknown } | undefined)?.rules;
  if (Array.isArray(rules)) {
    const root = rules.find(r => r && typeof r === "object" && (r as { path?: unknown }).path === "") as { write?: unknown } | undefined;
    if (typeof root?.write === "string" && root.write in RANK) return root.write;
  }
  return "interact";
}

/** The daemon's level for this view: the owner shell is `admin`, a named viewer `interact`, else `view`. */
function levelOf(env: CapEnv, name: string | null): string {
  if (env.token) return "admin";
  return name ? "interact" : "view";
}

export const userHandler: HandlerFactory = env => {
  // Without the declaration only the universal members answer (user.d.ts):
  // no id, no names, no lookups.
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
  /** The level this frame document was given at its first use of it (user.d.ts:
   * fixed for the life of a view); a new document (`reset`) takes a new one. */
  let level: Promise<string> | null = null;
  /** The newest `search()`: its number and its result; older calls resolve with it. */
  let searchGen = 0;
  let newest: Promise<Profile[]> = Promise.resolve([]);
  const timers = new Set<{ id: ReturnType<typeof setTimeout>; done(): void }>();

  /** The viewer, or none when the lookup fails. */
  const viewer = (): Promise<ViewerInfo | null> => env.viewer().catch(() => null);

  const profile = (id: string, name: string, meId: string | null): Profile => {
    const shown = profileScope ? name : "";
    const color = colorFor(id);
    return { id, name: shown, avatarUrl: avatarFor(shown, color), color, email: null, isMe: id === meId, guest: false };
  };

  async function me() {
    const v = await viewer();
    const id = declared && v ? v.publicId : null;
    const name = profileScope && id ? (v?.name ?? "") : "";
    const color = colorFor(id);
    return { id, name, avatarUrl: avatarFor(name, color), color, email: null, isOwner: owner, canEdit: owner };
  }

  async function get(url: string, headers: Record<string, string> = {}): Promise<ApiViewer[] | null> {
    if (disposed) return null;
    const ac = new AbortController();
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

  const levelNow = (): Promise<string> => (level ??= viewer().then(v => levelOf(env, v?.name ?? null)));

  /** Waits [`SEARCH_DEBOUNCE_MS`]; false when disposed meanwhile. */
  const pause = () => new Promise<boolean>(resolve => {
    const t = { id: setTimeout(() => { timers.delete(t); resolve(!disposed); }, SEARCH_DEBOUNCE_MS), done: () => resolve(false) };
    timers.add(t);
  });

  /** `search()` per user.d.ts: a call superseded by a newer one resolves with
   * the newer call's result, when that one resolves. */
  function search(q: unknown): Promise<Profile[]> {
    const gen = ++searchGen;
    const own = runSearch(q, () => gen === searchGen);
    newest = own;
    const settle = async (p: Promise<Profile[]>, g: number): Promise<Profile[]> => {
      const r = await p;
      return g === searchGen ? r : settle(newest, searchGen);
    };
    return settle(own, gen);
  }

  async function runSearch(q: unknown, current: () => boolean): Promise<Profile[]> {
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
    const vs = await get(`/api/viewers?q=${encodeURIComponent(text)}`, { authorization: `Bearer ${env.token}` });
    if (!vs) return [];
    remember(vs);
    return vs.map(x => profile(x.id, x.id === v.publicId ? (v.name ?? "") : (x.display_name ?? ""), v.publicId)).filter(p => p.name).slice(0, MAX_HITS);
  }

  return {
    async call(method, args) {
      switch (method) {
        case "isOwner":
        case "canEdit":
          return owner;
        case "can": {
          const what = args[0];
          if (what === "data.write") return RANK[await levelNow()] >= RANK[rootWrite(env.declared)];
          if (what === "files.write" || what === "assets.write") return owner;
          return false;
        }
        case "me":
          return me();
        case "id":
          return (await me()).id;
        case "name":
          return (await me()).name;
        case "avatarUrl": {
          const m = await me();
          return profileScope && m.id ? m.avatarUrl : null;
        }
        case "email":
          return null;
        case "profiles":
          return profiles(args[0]);
        case "search":
          return search(args[0]);
        default:
          throw new CapError("capability_removed", `user.${String(method)} is not part of this runtime`);
      }
    },
    reset() {
      // Resolved names and the level live for the page's lifetime (user.d.ts).
      cache = new Map();
      pending = new Map();
      level = null;
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
