// A hosted daemon's sign-in routes (crates/clax-server/src/hosted.rs): the
// first person's setup, invites redeemed in the browser, one-use sign-in
// links, sign out, agent tokens, pages shared by link or with one person,
// and the public gallery. A local daemon
// answers none of them (404). Every change is the browser's: the session
// cookie, from the public origin. A refusal throws `Refused` with the
// daemon's own message, which the page shows as it is. Names and titles
// are people's and agents' text: untrusted, rendered as text.
import { Refused } from "../people/api";

export { Refused };

/** Who the browser is signed in as. */
export type HostedPerson = { public_id: string; display_name: string | null; owner: boolean; guest: boolean };
/** `GET /api/hosted/me`. */
export type HostedMe = { origin: string; setup_open: boolean; signed_in: boolean; person: HostedPerson | null; github?: boolean };
/** An agent token, without its token. */
export type AgentToken = { id: string; label: string; status: string; created_at: string; last_used_at: string | null; ended_at: string | null };
/** A new agent token: `token` and the commands that carry it are shown this once. */
export type NewAgentToken = { id: string; label: string; created_at: string; token: string; mcp_url: string; claude_command: string; codex_config: string };
export type LinkAccess = "viewer" | "commenter";
/** A page shared by link; `listed` when the public gallery lists it. */
export type SharedLink = { artifact_id: string; title: string; access: LinkAccess; url: string; listed?: boolean };
/** What one person may do at a shared artifact. */
export type ShareAccess = "viewer" | "commenter" | "editor";
/** Who may open an artifact beyond its people: only them, anyone with its link, or the public gallery too. */
export type GeneralAccess = "restricted" | "link" | "public";
/** One person an artifact is shared with; `pending` until their GitHub login first signs in. */
export type Share = { id: string; access: ShareAccess; github_login: string | null; person: string | null; name: string | null; pending: boolean };
/** A person the owner's sharing dialog suggests. */
export type ShareCandidate = { person: string; name: string | null; github_login: string | null };
/** `GET /api/hosted/artifacts/<id>/sharing`: everything for whoever may share it, else `can_share: false` alone. */
export type Sharing =
  | { artifact_id: string; can_share: false }
  | { artifact_id: string; can_share: true; general: GeneralAccess; public_gallery?: boolean; link_access: LinkAccess | null; url: string; people: Share[]; suggestions: ShareCandidate[] };
/** An artifact in the public gallery. */
export type PublicArtifact = { artifact_id: string; title: string; url: string };

async function call<T>(path: string, method = "GET", body?: unknown): Promise<T> {
  const res = await fetch(path, method === "GET" ? { cache: "no-store" } : { method, headers: { "content-type": "application/json" }, body: JSON.stringify(body ?? {}) });
  if (res.ok) return (res.status === 204 ? {} : await res.json()) as T;
  let b: { error?: { code?: string; message?: string } } = {};
  try { b = await res.json(); } catch { /* not json */ }
  throw new Refused(res.status, b.error?.message ?? (res.statusText || `HTTP ${res.status}`), b.error?.code ?? null);
}
const e = encodeURIComponent;

/** The browser's sign-in, or null on a local daemon (which has none). */
export async function me(): Promise<HostedMe | null> {
  try { return await call<HostedMe>("/api/hosted/me"); } catch (x) { if (x instanceof Refused && x.status === 404) return null; throw x; }
}
export const setup = (secret: string, name: string) => call<{ person: HostedPerson }>("/api/hosted/setup", "POST", { secret, name });
export const peekInvite = (code: string) => call<{ name: string | null; expires_at: string }>(`/api/hosted/invites/${e(code)}`);
/** Redeems an invite; `replace` confirms ending the browser's current session (else 409 `session_exists`). */
export const acceptInvite = (code: string, replace = false) => call<{ person: HostedPerson }>(`/api/hosted/invites/${e(code)}/accept`, "POST", { replace });
export const signinLink = () => call<{ url: string; expires_at: string }>("/api/hosted/signin-links", "POST");
/** Whose account a live sign-in link signs into; spends nothing. */
export const peekSignin = (code: string) => call<{ public_id: string; name: string | null; owner: boolean; expires_at: string }>(`/api/hosted/signin-links/${e(code)}`);
/** Redeems a sign-in link; `replace` confirms ending the browser's session of someone else (else 409 `session_exists`). */
export const acceptSignin = (code: string, replace = false) => call<{ person: HostedPerson }>(`/api/hosted/signin-links/${e(code)}/accept`, "POST", { replace });
export const signOut = () => call<unknown>("/api/hosted/signout", "POST");
export const tokens = () => call<{ tokens: AgentToken[] }>("/api/hosted/agent-tokens");
export const mintToken = (label: string) => call<NewAgentToken>("/api/hosted/agent-tokens", "POST", { label });
export const revokeToken = (id: string) => call<{ token: AgentToken | null }>(`/api/hosted/agent-tokens/${e(id)}`, "DELETE");
/** A waiting device sign-in: the machine it names. */
export type DeviceRequest = { client_name: string; peer_ip: string; created_at: string; expires_at: string; same_address?: boolean };
export const peekDevice = (code: string) => call<DeviceRequest>(`/api/hosted/device/${e(code)}`);
export const approveDevice = (code: string) => call<{ token: AgentToken }>(`/api/hosted/device/${e(code)}/approve`, "POST");
export const denyDevice = (code: string) => call<unknown>(`/api/hosted/device/${e(code)}/deny`, "POST");
export const links = () => call<{ links: SharedLink[] }>("/api/hosted/links");
/** Shares `aid` by link at `access` (null stops, which unlists it too); `listed` lists it in the public gallery or not (absent keeps it). */
export const setLink = (aid: string, access: LinkAccess | null, listed?: boolean) =>
  call<{ artifact_id: string; access: LinkAccess | null; listed: boolean; url: string }>(`/api/hosted/artifacts/${e(aid)}/link`, "PUT", listed === undefined ? { access } : { access, listed });
export const sharing = (aid: string) => call<Sharing>(`/api/hosted/artifacts/${e(aid)}/sharing`);
/** Shares `aid` with a GitHub login or a person (by public ID), or changes their access. */
export const share = (aid: string, to: { github_login: string } | { person: string }, access: ShareAccess) =>
  call<{ share: Share }>(`/api/hosted/artifacts/${e(aid)}/shares`, "PUT", { ...to, access });
export const unshare = (aid: string, id: string) => call<{ share: Share }>(`/api/hosted/artifacts/${e(aid)}/shares/${e(id)}`, "DELETE");
/** The public gallery, answered to anyone. */
export const publicArtifacts = () => call<{ artifacts: PublicArtifact[] }>("/api/hosted/public");
