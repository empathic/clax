// The tags' routes (docs/contract.md "Tags") for the Tags page, the tag
// editor and the inbox's Tag filter. Only their lazy chunks load it. The
// owner's shell sends its token; anyone else's request is their browser's.
import { ApiError, type TagRef, getToken } from "../api";

/** A tag as `GET /api/tags` lists it: `count` is how many artifacts the caller sees carry it. */
export type TagView = TagRef & { description: string | null; created_at: string; updated_at: string; count: number };
/** `GET /api/tags`: the tags by name, the palette keys, and whether the caller makes and changes tags. */
export type TagList = { tags: TagView[]; colours: string[]; may_manage: boolean };
/** What a tag's form sets. */
export type TagFields = { name: string; colour: string; description: string };

async function call<T>(path: string, method = "GET", body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers["content-type"] = "application/json";
  if (method !== "GET") {
    const token = await getToken();
    if (token) headers.authorization = `Bearer ${token}`;
  }
  const r = await fetch(path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  const j = await r.json().catch(() => null) as { error?: { message?: string; code?: string } } | null;
  if (!r.ok) throw new ApiError(r.status, j?.error?.message ?? r.statusText, j?.error?.code ?? null);
  return j as T;
}

export const listTags = () => call<TagList>("/api/tags");
export const createTag = (f: TagFields) => call<{ tag: TagView }>("/api/tags", "POST", f);
export const updateTag = (id: string, f: Partial<TagFields>) => call<{ tag: TagView }>(`/api/tags/${encodeURIComponent(id)}`, "PATCH", f);
/** Deletes a tag everywhere; `artifacts` is how many carried it. */
export const deleteTag = (id: string) => call<{ deleted: boolean; artifacts: number }>(`/api/tags/${encodeURIComponent(id)}`, "DELETE");
/** Adds then removes tags (by ID) on artifact `aid`; the answer's `tags` is what it carries after. */
export const changeArtifactTags = (aid: string, add: string[], remove: string[]) =>
  call<{ artifact_id: string; tags: TagRef[]; changed: boolean }>(`/api/artifacts/${encodeURIComponent(aid)}/tags`, "PATCH", { add, remove });

/** A failed call's message, as the daemon gave it. */
export const failure = (e: unknown): string => (e instanceof ApiError ? e.message.replace(/^\d+ /, "") : e instanceof Error ? e.message : String(e));
