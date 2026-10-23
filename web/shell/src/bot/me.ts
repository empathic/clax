// This browser's ClaxBot (`GET /api/me/bot`), asked once per page and shared
// by the header's meter and Admin link, the usage banner, the New dialog's
// switch and the Send menu's line; `refresh` asks again after a change
// (a key set, a run started). Null where the daemon has no ClaxBot (404)
// or the request fails.
import { getToken } from "../api";
import { type MeBot, type Role, meBot } from "./api";

let asked: Promise<MeBot | null> | null = null;
/** Where this tab keeps whether ClaxBot is on for this browser, so the artifact view asks no more where it is not. */
const KEY = "clax.bot";

/** The caller's ClaxBot, asked once; the tab remembers whether it is on. */
export function myBot(): Promise<MeBot | null> {
  asked ??= meBot().catch(() => null).then(b => {
    try { sessionStorage.setItem(KEY, b?.enabled ? "1" : "0"); } catch { /* this page only */ }
    return b;
  });
  return asked;
}

/** What the artifact's agents say of ClaxBot (`live`, else why not): the
 * tab remembers it as on unless it is not turned on for this person. */
export function noteListed(live: boolean, unavailable: string | null): void {
  try { sessionStorage.setItem(KEY, live || unavailable !== "no_flag" ? "1" : "0"); } catch { /* this page only */ }
}

/** Whether ClaxBot may be on for this browser: as this tab last heard
 * (`myBot`), else as the daemon answers now. */
export async function botOn(): Promise<boolean> {
  let known: string | null = null;
  try { known = sessionStorage.getItem(KEY); } catch { /* none */ }
  if (known === "0") return false;
  if (known === "1") return true;
  return !!(await myBot())?.enabled;
}

/** Forgets the answer: the next `myBot` asks again. */
export function forget(): void {
  asked = null;
  try { sessionStorage.removeItem(KEY); } catch { /* none */ }
}

/** Asks again; the next `myBot` answers with it. */
export function refresh(): Promise<MeBot | null> {
  asked = null;
  return myBot();
}

/** The caller's role: the daemon's word for it, else the owner's in the
 * owner's shell (which holds the token), else a member's. A daemon without
 * ClaxBot has no admins to show. */
export async function myRole(me?: MeBot | null): Promise<Role> {
  const m = me === undefined ? await myBot() : me;
  if (!m) return "member";
  if (m.role) return m.role;
  return (await getToken()) ? "owner" : "member";
}

/** Whether `role` sees the admin pages. */
export const isAdmin = (role: Role | null | undefined): boolean => role === "owner" || role === "admin";

/** Whether this browser talks to a hosted daemon (`GET /api/hosted/me` answers), where Settings are not shown. */
export function hosted(): Promise<boolean> {
  return fetch("/api/hosted/me").then(r => r.ok, () => false);
}
