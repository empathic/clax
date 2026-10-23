// The Share dialog's model (crates/clax-server/src/hosted.rs, the sharing
// routes): the words for each access, a GitHub login as the daemon checks
// it, the suggestions the add field offers, and the link request each
// general access makes. Pure: the dialog renders it.
import type { GeneralAccess, LinkAccess, Share, ShareAccess, ShareCandidate } from "../hosted/api";

export const ACCESS_LABEL: Record<ShareAccess, string> = { viewer: "Can view", commenter: "Can comment", editor: "Can edit" };
/** Listed by key: a destructuring `#each` would pull a helper into a shared chunk. */
export const ACCESSES = Object.keys(ACCESS_LABEL) as ShareAccess[];
export const LINK_ACCESSES: LinkAccess[] = ["viewer", "commenter"];
export const GENERAL_LABEL: Record<GeneralAccess, string> = { restricted: "Restricted", link: "Anyone with the link", public: "Public" };
export const GENERALS = Object.keys(GENERAL_LABEL) as GeneralAccess[];
export const GENERAL_NOTE: Record<GeneralAccess, string> = {
  restricted: "Only the people added can open it.",
  link: "Anyone with the link can open it, without signing in.",
  public: "Anyone with the link can open it, and it is listed on this Clax's public page.",
};

/** The Comments switch (spec 2026-10-21-app-push-pull §5.6), and the line under it, on and off. */
export const COMMENTS_LABEL = "Comments";
export const COMMENTS_NOTE = { on: "People who can comment may comment.", off: "Off: nobody can comment, whatever their access." } as const;

/** `s` as a GitHub login (an `@` first is dropped), lowercased; null when it is not one. */
export function parseLogin(s: string): string | null {
  const l = s.trim().replace(/^@/, "");
  return /^[A-Za-z0-9][A-Za-z0-9-]{0,38}$/.test(l) ? l.toLowerCase() : null;
}

/** How a share is named: the person's name, else `@login`. */
export function shareName(s: Share): string {
  return s.name ?? (s.github_login ? `@${s.github_login}` : "Someone");
}

/** The line under a share's name: its login, and that it waits for a first sign-in. */
export function shareDetail(s: Share): string {
  const login = s.github_login && s.name ? `@${s.github_login}` : "";
  return s.pending ? "Not signed in yet: has access once they sign in with GitHub" : login;
}

/** Whether `c` already has a share among `people`. */
const holds = (c: ShareCandidate, people: Share[]) =>
  people.some(p => (p.person && p.person === c.person) || (!!c.github_login && p.github_login === c.github_login.toLowerCase()));

/** The suggestions for what was typed: people not shared with yet whose name or login starts a word with it, at most `max`. */
export function suggest(query: string, candidates: ShareCandidate[], people: Share[], max = 6): ShareCandidate[] {
  const q = query.trim().replace(/^@/, "").toLowerCase();
  if (!q) return [];
  const hit = (s: string | null) => !!s && s.toLowerCase().split(/[\s-]+/).concat(s.toLowerCase()).some(w => w.startsWith(q));
  return candidates.filter(c => !holds(c, people) && (hit(c.name) || hit(c.github_login))).slice(0, max);
}

/** What the add field names: a chosen suggestion's person; else, for text with no `@` first that is
 * one of `candidates`' names (any case), that person; else the login typed. Null when none, or when
 * the name is more than one candidate's: a word naming someone here is never taken for a stranger's
 * GitHub login, which takes the `@`. */
export function subject(query: string, chosen: ShareCandidate | null, candidates: ShareCandidate[] = []): { person: string } | { github_login: string } | null {
  if (chosen) return { person: chosen.person };
  const typed = query.trim();
  if (!typed.startsWith("@")) {
    const named = candidates.filter(c => c.name?.trim().toLowerCase() === typed.toLowerCase());
    if (named.length === 1) return { person: named[0].person };
    if (named.length > 1) return null;
  }
  const login = parseLogin(query);
  return login ? { github_login: login } : null;
}

/** The link request a general access makes, at `access` for anyone with the link. */
export function linkRequest(general: GeneralAccess, access: LinkAccess): { access: LinkAccess | null; listed: boolean } {
  return general === "restricted" ? { access: null, listed: false } : { access, listed: general === "public" };
}
