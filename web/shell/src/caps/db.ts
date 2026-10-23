// db.d.ts's live updates for a framed page. The page's bridge reads and
// writes its documents on its own origin (web/bridge/src/caps/origin.ts),
// whose site has no change stream; the shell hears the daemon's event
// stream, so once the bridge asks (`watch`, a call per frame document) it
// tells it what changed: `db/changed` with `{path}` for a document event,
// `{}` (every subscription) for `resync`, `version` (a republish may change
// the rules) and `ready`, and `db/stream` with `{up}` as the stream drops
// and comes back (`stream_up` or `ready`), so the bridge polls only while it
// is down. It sends nothing it was not asked for, and reads or writes
// nothing of the page's. In the Clax app's migration view the bridge hands
// it each document's value before the migration's first write to it
// (`journal`, `{path, before}`), which goes to the app's write journal
// while one runs (`CapEnv.journal`, app/inspect.ts) and is answered once
// the journal has it; `replayJournal` puts a journal's values back. A
// `db.artifact(id)` read its site refused `not_granted` asks the owner here
// (`grant`, cross-read.ts).
import type { ArtifactEvent } from "../events";
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

const SEGMENT = /^[A-Za-z0-9_\-.~:@+]{1,200}$/;

/** `path` when it is a document path in the contract's grammar, else
 * `invalid_argument`: no `.` or `..` segment, so it cannot leave the Docs
 * routes. */
function docPath(path: unknown): string {
  const segs = typeof path === "string" ? path.split("/") : [];
  const ok = typeof path === "string" && path.length <= 1000 && segs.length <= 16 && segs.length % 2 === 0
    && segs.every(s => SEGMENT.test(s) && s !== "." && s !== "..");
  if (!ok) throw new CapError("invalid_argument", `'${String(path)}' is not a document path`);
  return path as string;
}

/** `v` when it is a journal entry's `before`: a plain object, or null. */
function before(v: unknown): Record<string, unknown> | null {
  if (v === null) return null;
  if (typeof v !== "object" || Array.isArray(v)) throw new CapError("invalid_argument", "before is a document's data, or null");
  return v as Record<string, unknown>;
}

/** Puts back what a migration's write journal holds, as this viewer (the
 * cookie, and `token` when given): each entry in reverse order, `set` to its
 * `before`, or deleted when `before` is null (one already gone is fine).
 * Throws the first refusal. */
export async function replayJournal(aid: string, token: string | null, entries: { path: string; before: Record<string, unknown> | null }[]): Promise<void> {
  const base = `/api/artifacts/${aid}/docs`;
  const headers: Record<string, string> = { "content-type": "application/json" };
  if (token) headers.authorization = `Bearer ${token}`;
  for (const e of [...entries].reverse()) {
    const url = `${base}/${docPath(e.path).split("/").map(encodeURIComponent).join("/")}`;
    const res = e.before === null
      ? await fetch(`${url}?lww=true`, { method: "DELETE", headers })
      : await fetch(url, { method: "PUT", headers, body: JSON.stringify({ data: e.before, lww: true }) });
    if (res.ok || (e.before === null && res.status === 404)) continue;
    const err = ((await res.json().catch(() => ({}))) as { error?: { code?: string; message?: string } }).error ?? {};
    throw new CapError(res.status === 429 ? "resource_exhausted" : [400, 403, 404, 409].includes(res.status) ? "invalid_argument" : "unavailable", err.message ?? `HTTP ${res.status}`);
  }
}

export const dbHandler: HandlerFactory = env => {
  let watching = false;
  const post = (topic: string, data: unknown) => env.post({ type: "clax:event", ns: "db", topic, data });
  return {
    async call(method, args) {
      // `db.artifact(id)`'s consent, from a chunk of its own (cross-read.ts).
      if (method === "grant") return (await import("./cross-read")).grantRead(env, args[0]);
      if (method === "journal") {
        const e = (args[0] ?? {}) as { path?: unknown; before?: unknown };
        const entry = { path: docPath(e.path), before: before(e.before) };
        const j = env.journal?.();
        if (j) await j(entry);
        return null;
      }
      if (method !== "watch") throw new CapError("capability_removed", `db.${String(method)} is not part of this runtime`);
      watching = true;
      return null;
    },
    onEvent(e: ArtifactEvent) {
      if (!watching) return;
      if (e.type === "doc") post("changed", { path: e.path });
      else if (e.type === "resync" || e.type === "version") post("changed", {});
      else if (e.type === "stream_up") post("stream", { up: true });
      else if (e.type === "stream_down") post("stream", { up: false });
      else if (e.type === "ready") {
        // A fresh stream: whatever happened before it is refetched.
        post("stream", { up: true });
        post("changed", {});
      }
    },
    reset() { watching = false; },
    dispose() { watching = false; },
  };
};
