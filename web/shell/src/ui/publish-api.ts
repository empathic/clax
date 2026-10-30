// The local daemon's remotes and pushes the Publish button uses
// (docs/contract.md "Publishing to a public Clax"). Loaded with the button,
// after the first paint. A hosted daemon answers none of them (404).
import { ApiError, getToken } from "../api";
import { HELD, heldOf } from "../held";

/** A public Clax this Clax is signed in to (never its token); while its
 * sign-in waits, the code, the approval page and the seconds between polls. */
export type Remote = {
  id: string; origin: string; name: string; status: string; person_name: string | null;
  user_code?: string | null; verification_url?: string | null; poll_interval_s?: number | null;
};

/** How a link's threads' sync stands. */
export type Sync = { unreachable_since: string | null; last_error: string | null; synced_at: string | null };

/** An artifact's public twin on one remote; its threads follow it (linked threads). */
export type Link = {
  remote_id: string; remote_name: string | null; origin: string | null; url: string;
  /** Its remote's status: `signed_out` while the remote refuses this Clax's
   * token; its pushes and comments wait for a new sign-in, which keeps the link. */
  remote_status?: string | null;
  pushed_n: number; remote_n: number; auto: boolean; behind: number;
  /** Local threads stay local; replies and resolves on linked threads still go up. */
  download_only?: boolean;
  sync?: Sync;
  /** What the last cycle could not read of this link's threads; null when it read them all. */
  read_error?: string | null;
  /** Writes waiting to go up, and those not sent. */
  pending?: number; failed?: number;
  /** Open questions whose copies there were not sent (Retry sends them). */
  failed_questions?: number;
};

/** A write not sent to a remote the artifact is no longer linked to. */
export type Unsent = { seq: number; kind: string; thread_id: string; body: string | null };

/** `GET /api/artifacts/<id>/remotes`. */
export type Remotes = { links: Link[]; remotes: Remote[]; unsent?: Unsent[] };

/** `POST /api/artifacts/<id>/push`'s answer. */
export type Pushed = { status: "pushed"; url: string; link: Link; pushed: { n: number; remote_n: number }[]; assets_not_pushed: number };

export type Mode = "update" | "new" | "fork";

async function headers(json: boolean): Promise<Record<string, string>> {
  const h: Record<string, string> = json ? { "content-type": "application/json" } : {};
  const t = await getToken();
  if (t) h.authorization = `Bearer ${t}`;
  return h;
}

async function answer<T>(res: Response): Promise<T> {
  // Held for the owner's confirmation (a push or a sign-in): where to confirm.
  const held = await heldOf(res);
  if (held) throw new ApiError(202, held.message, HELD);
  if (!res.ok) {
    let msg = res.statusText;
    let code: string | null = null;
    try {
      const e = (await res.json()).error;
      msg = e?.message ?? msg;
      code = e?.code ?? null;
    } catch { /* not JSON */ }
    throw new ApiError(res.status, msg, code);
  }
  return res.json() as Promise<T>;
}

/** Artifact `id`'s public twins and the remotes it could go to. */
export async function remotesOf(id: string): Promise<Remotes> {
  return answer<Remotes>(await fetch(`/api/artifacts/${id}/remotes`, { headers: await headers(false), cache: "no-store" }));
}

/** Pushes artifact `id` to `remote` in `mode`; `auto` lets agents push its new versions without asking. */
export async function push(id: string, remote: string, mode: Mode, auto?: boolean): Promise<Pushed> {
  return answer<Pushed>(await fetch(`/api/artifacts/${id}/push`, {
    method: "POST", headers: await headers(true), body: JSON.stringify({ remote, mode, auto }),
  }));
}

/** The public Clax the Publish button signs in to when this Clax is signed in to none. */
export const DEFAULT_ORIGIN = "https://clax.dev";

/** `POST /api/remotes`'s answer: a device sign-in waiting for the person's
 * approval there; `return_to_kept`, whether its approval page sends them back. */
export type SignIn = { remote: Remote; user_code: string; verification_url: string; expires_at: string; interval: number; return_to_kept: boolean };

/** Starts signing this Clax in to the public Clax at `url`; `returnTo`, a
 * path here, is the page its approval page sends the person back to. */
export function startSignIn(url: string, returnTo?: string): Promise<SignIn> {
  return send("POST", "/api/remotes", returnTo === undefined ? { url } : { url, return_to: returnTo });
}

/** `GET /api/remotes`: the public Clax instances this Clax is signed in to, or signing in to. */
export async function listRemotes(): Promise<Remote[]> {
  return (await answer<{ remotes: Remote[] }>(await fetch("/api/remotes", { headers: await headers(false), cache: "no-store" }))).remotes;
}

/** Asks whether remote `id`'s sign-in was approved there. */
export function pollSignIn(id: string): Promise<{ remote: Remote; status: "pending" | "connected" | "denied" | "expired" }> {
  return send("POST", `/api/remotes/${encodeURIComponent(id)}/poll`, {});
}

/** A remote's short label: its host. */
export function hostOf(origin: string | null): string {
  if (!origin) return "the public Clax";
  try { return new URL(origin).host; } catch { return origin; }
}

async function send<T>(method: string, url: string, body?: unknown): Promise<T> {
  return answer<T>(await fetch(url, {
    method, headers: await headers(body !== undefined), body: body === undefined ? undefined : JSON.stringify(body),
  }));
}

/** Makes `id`'s link on `remote` download only, or two-way again. */
export function setDownloadOnly(id: string, remote: string, on: boolean): Promise<{ download_only: boolean }> {
  return send("PATCH", `/api/artifacts/${id}/remotes/${encodeURIComponent(remote)}`, { download_only: on });
}

/** Tries `id`'s writes not sent to `remote` again now. */
export function retryLink(id: string, remote: string): Promise<{ retried: number }> {
  return send("POST", `/api/artifacts/${id}/remotes/${encodeURIComponent(remote)}/retry`, {});
}

/** Ends `id`'s link on `remote`: its threads stay here, read-only. */
export function unlink(id: string, remote: string): Promise<{ unlinked: boolean; unsent: Unsent[] }> {
  return send("DELETE", `/api/artifacts/${id}/remotes/${encodeURIComponent(remote)}`);
}

/** Drops `id`'s writes not sent to a remote it is no longer linked to. */
export function dropUnsent(id: string): Promise<{ dropped: number }> {
  return send("POST", `/api/artifacts/${id}/unsent:drop`, {});
}
