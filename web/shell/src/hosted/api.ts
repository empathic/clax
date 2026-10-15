// A hosted daemon's sign-in routes (crates/clax-server/src/hosted.rs): the
// first person's setup, invites redeemed in the browser, one-use sign-in
// links, sign out, agent tokens and pages shared by link. A local daemon
// answers none of them (404). Every change is the browser's: the session
// cookie, from the public origin. A refusal throws `Refused` with the
// daemon's own message, which the page shows as it is. Names and titles
// are people's and agents' text: untrusted, rendered as text.
import { Refused } from "../people/api";

export { Refused };

/** Who the browser is signed in as. */
export type HostedPerson = { public_id: string; display_name: string | null; owner: boolean; guest: boolean };
/** `GET /api/hosted/me`. */
export type HostedMe = { origin: string; setup_open: boolean; signed_in: boolean; person: HostedPerson | null };
/** An agent token, without its token. */
export type AgentToken = { id: string; label: string; status: string; created_at: string; last_used_at: string | null; ended_at: string | null };
/** A new agent token: `token` and the commands that carry it are shown this once. */
export type NewAgentToken = { id: string; label: string; created_at: string; token: string; mcp_url: string; claude_command: string; codex_config: string };
export type LinkAccess = "viewer" | "commenter";
/** A page shared by link. */
export type SharedLink = { artifact_id: string; title: string; access: LinkAccess; url: string };

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
export const setLink = (aid: string, access: LinkAccess | null) => call<{ artifact_id: string; access: LinkAccess | null; url: string }>(`/api/hosted/artifacts/${e(aid)}/link`, "PUT", { access });
