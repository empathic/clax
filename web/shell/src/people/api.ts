// People's routes (spec 2026-10-08-multi-agent-multi-person §6.2, §6.3,
// §6.4, §7.4, §9.3, §9.4, §9.6). They load with the People page. Every
// change is the browser's (M20): the page's cookie, never the token. A
// refusal throws `Refused` with the daemon's own message, which the page
// shows as it is.
import type { NewInvite, PeopleApproval, PeopleGrant, PeopleInvite, PeopleList, PeopleMe, PeopleRequest, Rights } from "../api";

/** A route's refusal: its status, the daemon's message and code. */
export class Refused extends Error {
  constructor(public status: number, message: string, public code: string | null) {
    super(message);
    this.name = "Refused";
  }
}

async function call<T>(path: string, method = "GET", body?: unknown): Promise<T> {
  const res = await fetch(path, method === "GET" ? undefined : { method, ...body !== undefined && { headers: { "content-type": "application/json" }, body: JSON.stringify(body) } });
  if (res.ok) return (res.status === 204 ? {} : await res.json()) as T;
  let b: { error?: { code?: string; message?: string } } = {};
  try { b = await res.json(); } catch { /* not json */ }
  throw new Refused(res.status, b.error?.message ?? (res.statusText || `HTTP ${res.status}`), b.error?.code ?? null);
}
const e = encodeURIComponent;

export const me = () => call<PeopleMe>("/api/people/me");
export const list = () => call<PeopleList>("/api/people");
export const invites = () => call<{ invites: PeopleInvite[] }>("/api/people/invites");
/** Connection requests waiting for the owner. */
export const pending = () => call<{ grants: PeopleGrant[] }>("/api/grants?status=pending");
export const approvals = () => call<{ approvals: PeopleApproval[] }>("/api/approvals");
export const requests = () => call<{ requests: PeopleRequest[]; open: number; banner: string | null }>("/api/approvals/requests");

/** Invites a new person (`name`), an existing LAN viewer (`viewer`) or the owner's own machine (`me`). */
export const invite = (who: { name: string } | { viewer: string } | { me: true }, rights?: string) =>
  call<{ invite: NewInvite }>("/api/people/invites", "POST", { ...who, ...rights && { rights } });
export const revokeInvite = (id: string) => call<unknown>(`/api/people/invites/${e(id)}`, "DELETE");
export const setRights = (pid: string, rights: string | Rights) => call<unknown>(`/api/people/${e(pid)}/rights`, "PUT", { rights });
export const revokePerson = (pid: string) => call<unknown>(`/api/people/${e(pid)}`, "DELETE");
export const narrowGrant = (gid: string, access: Rights | null) => call<unknown>(`/api/people/me/grants/${e(gid)}/narrow`, "PUT", { access });
/** Disconnects a machine: ends its grant, its tokens and its agents. */
export const revokeGrant = (gid: string) => call<unknown>(`/api/grants/${e(gid)}`, "DELETE");
export const denyGrant = (gid: string) => call<unknown>(`/api/grants/${e(gid)}/deny`, "POST", {});
export const narrowAgent = (handle: string, access: Rights | null) => call<unknown>(`/api/agents/${e(handle)}/narrow`, "PUT", { access });
/** Raises an agent with `access`, or removes its elevation (null). */
export const elevate = (handle: string, access: Rights | null) =>
  access ? call<unknown>(`/api/agents/${e(handle)}/elevate`, "PUT", { access }) : call<unknown>(`/api/agents/${e(handle)}/elevate`, "DELETE");
export const endAgent = (handle: string) => call<unknown>(`/api/agents/${e(handle)}`, "DELETE");
export const rename = (handle: string, name: string) => call<unknown>(`/api/agents/${e(handle)}`, "PUT", { name });
/** Lets `author`'s content reach the caller's agents everywhere (`*unconnected`: the switch's on). */
export const allow = (author: string) => call<unknown>("/api/approvals", "POST", { author, scope: "everywhere" });
export const revokeApproval = (id: string) => call<unknown>(`/api/approvals/${e(id)}`, "DELETE");
