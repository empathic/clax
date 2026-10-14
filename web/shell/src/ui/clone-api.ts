// The git token API the Clone button and dialog use (hosted spec §9
// "Tokens"; docs/contract.md "Git remotes"). Loaded with the Clone button,
// after the first paint.
import { ApiError, getToken } from "../api";

export type CloneScope = "read" | "write";

/** `GET /api/artifacts/<id>/git-token`: what the viewer may ask for. */
export type CloneOptions = {
  available: boolean;
  scopes: CloneScope[];
  ttl_s?: number;
  reason?: { code: string; message: string };
};

/** The commands for a checkout that already exists (`existing_checkout`). */
export type Checkout = {
  remote_name: string;
  add_remote_command: string;
  fetch_command: string;
  push_command?: string;
  jj_add_remote_command: string;
  jj_fetch_command: string;
  jj_push_command?: string;
};

/** `POST /api/artifacts/<id>/git-token`: a token handed to the viewer. */
export type GitToken = {
  artifact_id: string;
  scope: CloneScope;
  remote: string;
  token: string;
  expires_at: string;
  clone_command: string;
  jj_clone_command: string;
  existing_checkout: Checkout;
};

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

/** What the viewer may ask for on artifact `id`. */
export async function cloneOptions(id: string): Promise<CloneOptions> {
  return answer<CloneOptions>(await fetch(`/api/artifacts/${id}/git-token`, { headers: await headers(false) }));
}

/** A token of `scope` on artifact `id`, handed to the viewer. */
export async function mintToken(id: string, scope: CloneScope): Promise<GitToken> {
  return answer<GitToken>(await fetch(`/api/artifacts/${id}/git-token`, {
    method: "POST", headers: await headers(true), body: JSON.stringify({ scope }),
  }));
}

/** "in about 59 min", "in about 2 h", or "now" once `iso` has passed, from `now`. */
export function expiresIn(iso: string, now: number = Date.now()): string {
  const min = Math.round((Date.parse(iso) - now) / 60000);
  if (!(min > 0)) return "now";
  return min < 90 ? `in about ${min} min` : `in about ${Math.round(min / 60)} h`;
}
