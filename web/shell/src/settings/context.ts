// What this browser is to this Clax, asked once per page: it decides which
// of Settings' screens the navigation, the overview and the search show.
import { getToken } from "../api";
import { isAdmin, myBot, myRole } from "../bot/me";
import type { Ctx } from "./screens";

let asked: Promise<Ctx> | null = null;

const json = <T>(url: string): Promise<T | null> => fetch(url).then(r => (r.ok ? r.json() as Promise<T> : null), () => null);

/** This browser's context: whether the Clax is hosted, and whether the
 * browser is the owner's, a member's, an admin's, with ClaxBot or without. */
export function context(): Promise<Ctx> {
  asked ??= Promise.all([
    getToken().catch(() => null),
    json<{ person?: { owner?: boolean } | null }>("/api/hosted/me"),
    json<{ viewer?: { person?: boolean; person_revoked_at?: string } | null }>("/api/viewers/me"),
    myBot(),
  ]).then(async ([token, hosted, me, bot]) => {
    const owner = !!token || !!hosted?.person?.owner;
    const v = me?.viewer;
    return {
      hosted: !!hosted,
      owner,
      member: owner || !!hosted?.person || (!!v?.person && !v.person_revoked_at),
      admin: isAdmin(await myRole(bot)),
      bot: !!bot,
    };
  });
  return asked;
}

/** Forgets the answer: the next `context` asks again. */
export function forget(): void { asked = null; }
