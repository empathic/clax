// The local daemon's remotes and pushes the Publish button uses
// (docs/contract.md "Publishing to a public Clax"). Loaded with the button,
// after the first paint. A hosted daemon answers none of them (404).
import { ApiError, getToken } from "../api";

/** A public Clax this Clax is signed in to (never its token). */
export type Remote = { id: string; origin: string; name: string; status: string; person_name: string | null };

/** How a link's threads' sync stands. */
export type Sync = { unreachable_since: string | null; last_error: string | null; synced_at: string | null };

/** An artifact's public twin on one remote; its threads follow it (linked threads). */
export type Link = {
  remote_id: string; remote_name: string | null; origin: string | null; url: string;
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
