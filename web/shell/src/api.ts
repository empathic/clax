export type Artifact = {
  id: string; title: string; description: string | null; icon: string | null;
  created_at?: string; updated_at: string; current_version: number; pinned: boolean;
  capabilities?: Record<string, unknown>; contract_version?: string; owner_session_id?: string | null;
};
export type FileMeta = { content_type: string; size: number };
export type Version = { artifact_id: string; n: number; label: string | null; created_at: string; files: Record<string, FileMeta> };

/** A non-OK API response; `status` is the HTTP status code. */
export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(`${status} ${message}`);
    this.name = "ApiError";
  }
}

async function json<T>(res: Response): Promise<T> {
  if (!res.ok) {
    let msg = res.statusText;
    try { msg = (await res.json()).error?.message ?? msg; } catch { /* not json */ }
    throw new ApiError(res.status, msg);
  }
  return res.json() as Promise<T>;
}

export async function listArtifacts(): Promise<Artifact[]> {
  return (await json<{ artifacts: Artifact[] }>(await fetch("/api/artifacts"))).artifacts;
}
export async function getArtifact(id: string): Promise<{ artifact: Artifact; versions: Version[] }> {
  return json(await fetch(`/api/artifacts/${id}`));
}

let tokenPromise: Promise<string | null> | null = null;
/** The write token, only served to loopback browsers; null on a LAN viewer. */
export function getToken(): Promise<string | null> {
  if (!tokenPromise) {
    const p: Promise<string | null> = fetch("/api/token").then(async r => {
      if (r.ok) return (await r.json()).token as string;
      if (r.status === 403) return null;
      throw new Error(`token request failed: ${r.status}`);
    }).catch(() => {
      if (tokenPromise === p) tokenPromise = null;
      return null;
    });
    tokenPromise = p;
  }
  return tokenPromise;
}
export async function patchArtifact(id: string, patch: Partial<Pick<Artifact, "title" | "description" | "icon" | "pinned">>, token: string): Promise<Artifact> {
  return (await json<{ artifact: Artifact }>(await fetch(`/api/artifacts/${id}`, { method: "PATCH", headers: { "content-type": "application/json", authorization: `Bearer ${token}` }, body: JSON.stringify(patch) }))).artifact;
}
export async function deleteArtifact(id: string, token: string): Promise<void> {
  await json<unknown>(await fetch(`/api/artifacts/${id}`, { method: "DELETE", headers: { authorization: `Bearer ${token}` } }).then(r => (r.status === 204 ? new Response("{}") : r)));
}
