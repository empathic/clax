// `db.artifact(id)` in the shell:
// app A, the artifact this view serves, reads app B's documents, read-only.
// Loaded by the `db` handler on the page's first `read` call.
//
// The shell is the consent boundary, so:
// - B must be listed in A's `capabilities.db.read` (else `not_declared`,
//   before any request);
// - every request names A in the X-Clax-Reader header from the view (env.aid),
//   never from anything the page sends, and goes only to B's document read
//   routes, with paths checked by `checkPath` (no `.` or `..` segment);
// - a refusal (`not_granted`) in the owner shell asks the owner once per
//   source and view ("Let “A” read “B”'s data?"); Allow makes the grant
//   (`PUT /api/artifacts/<A>/db-grants/<B>`) and the read is tried again.
//   A dismissed prompt refuses the reads waiting on it and is not kept: the
//   next read asks again.
//   A share in B's declaration, or an earlier grant, is served without
//   asking. A viewer who is not the owner is never asked, nor is anyone in
//   the app's inspect view (`?app=1&inspect=1`, `app/host.ts`), which
//   renders a draft for the agent: there a refusal stands;
// - subscriptions follow the stream topic `docs:<B>?reader=<A>` only (never
//   `docs:<B>`, whose events reach the stream at its own level), and refetch
//   on B's `doc` events there; nothing else of B (threads, files, versions)
//   reaches the page. A refetch the daemon refuses (the grant was revoked)
//   ends the subscription with `not_granted`, without asking.
import { inspectQuery } from "../route";
import { type StreamEvent, pageStream } from "../stream";
import { type ApiDoc, MAX_SUBSCRIPTIONS, SNAPSHOT_DEBOUNCE_MS, type Spec, type WireDoc, checkPath, checkSpec, checkSubId, dbError, docUrl, runQuery, touches, wire } from "./db";
import { CapError } from "./errors";
import { ALWAYS } from "./grants";
import type { CapEnv } from "./host";

/** Follows stream topics until the returned function runs (`EventStream.watch`). */
export type Watch = (topics: string[], on: (e: StreamEvent) => void) => () => void;

export type CrossRead = {
  /** One `db.read` op: `{op: "get", artifact, path}`, `{op: "query", artifact, spec}`,
   * `{op: "subscribe", artifact, sub, spec}` or `{op: "unsubscribe", sub}`. */
  call(arg: unknown): Promise<unknown>;
  /** The frame loaded a new document: every subscription ends. */
  reset(): void;
  leave(): void;
  dispose(): void;
};

/** The source app does not declare db (the daemon's `not_declared`). */
const notDeclared = (src: string) => new CapError("not_declared", `${src} does not keep documents to share (it does not declare db)`);
const ARTIFACT_ID = /^[0-9a-hjkmnp-tv-z]{12}$/;
const STREAM_EVENTS = new Set(["ready", "resync", "refused", "stream_up"]);

export function crossRead(env: CapEnv, watch: Watch = (t, on) => pageStream().watch(t, on)): CrossRead {
  const reader = env.aid;
  /** Per source: whether the owner allowed it, asked at most once per view. */
  const asked = new Map<string, Promise<boolean>>();
  /** Every subscription, from its subscribe call on: `live` once its first
   * read is delivered and its topic followed. A pending one holds its slot,
   * and a reset, leave or unsubscribe meanwhile removes it, which its
   * subscribe call sees as another entry (or none) under its ID. */
  const subs = new Map<string, { src: string; spec: Spec; live: boolean }>();
  const watches = new Map<string, () => void>();
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  const fetches = new Map<string, number>();
  const delivered = new Map<string, number>();
  let seq = 0;
  let disposed = false;
  const topicOf = (src: string) => `docs:${src}?reader=${reader}`;
  const headers = (h: Record<string, string>) => (env.token ? { ...h, authorization: `Bearer ${env.token}` } : h);

  /** `v` when it is an artifact ID A's declaration lists in `db.read`. */
  function source(v: unknown): string {
    if (typeof v !== "string" || !ARTIFACT_ID.test(v)) throw new CapError("invalid_argument", `'${String(v)}' is not an artifact ID`);
    const reads = env.declared.db?.read;
    if (v === reader || !Array.isArray(reads) || !reads.includes(v)) throw new CapError("not_declared", `this app does not declare reading ${v}'s documents (capabilities.db.read)`);
    return v;
  }

  /** One GET of B's documents as reader A; a missing document is null. */
  async function get(url: string): Promise<unknown> {
    if (disposed) throw new CapError("unavailable", "this view has closed");
    let res: Response;
    try {
      res = await fetch(url, { headers: headers({ "x-clax-reader": reader }) });
    } catch {
      throw new CapError("unavailable", "the Clax daemon could not be reached");
    }
    if (res.ok) return res.json();
    if (res.status === 404) return null;
    const err = ((await res.json().catch(() => ({}))) as { error?: { code?: string; message?: string } }).error ?? {};
    if (err.code === "not_granted") throw new CapError("not_granted", `this app may not read ${url.split("/")[3]}'s documents`);
    if (err.code === "not_declared") throw notDeclared(url.split("/")[3]);
    throw dbError(res.status, err, false);
  }

  /** Artifact `id`'s title and declaration, as far as the daemon tells the owner. */
  const info = (id: string) => fetch(`/api/artifacts/${id}`, { headers: headers({}) })
    .then(r => r.json()).then(j => (j as { artifact: { title: string; capabilities?: Record<string, unknown> } }).artifact, () => ({ title: "", capabilities: undefined }));

  async function ask(src: string): Promise<boolean> {
    const [a0, b0] = await Promise.all([info(reader), info(src)]);
    // A source without db has nothing a grant could open: no dialog.
    if (b0.capabilities && !Object.hasOwn(b0.capabilities, "db")) {
      asked.delete(src);
      throw notDeclared(src);
    }
    const [a, b] = [a0.title || reader, b0.title || src];
    const answer = await env.prompt({
      kind: "crossRead",
      title: `Let “${a}” read “${b}”'s data?`,
      body: `“${a}” will see the documents “${b}” keeps, and cannot change them. Documents private to one viewer stay private.`,
      allow: "Allow",
      deny: "Don't allow",
      always: ALWAYS,
    });
    // A dismissal (Escape, a tap outside) refuses this read only: the next one asks again.
    if (answer === "dismiss") asked.delete(src);
    if (answer !== "allow") return false;
    const res = await fetch(`/api/artifacts/${reader}/db-grants/${src}`, { method: "PUT", headers: headers({}) }).catch(() => null);
    if (!res?.ok) {
      asked.delete(src);
      throw new CapError("unavailable", "the grant could not be saved");
    }
    return true;
  }

  /** `url` read as A; on a refusal in the owner shell, when `mayAsk` (a
   * call of the page, not a refetch), asked once per source, except in the
   * inspect view. */
  async function read(src: string, url: string, mayAsk: boolean): Promise<unknown> {
    try {
      return await get(url);
    } catch (e) {
      if (!(e instanceof CapError && e.code === "not_granted") || env.token === null || !mayAsk || inspectQuery(location.search)) throw e;
      let p = asked.get(src);
      if (!p) asked.set(src, p = ask(src));
      if (!(await p)) throw e;
      return get(url);
    }
  }

  async function current(src: string, spec: Spec, mayAsk: boolean): Promise<WireDoc[]> {
    const base = `/api/artifacts/${src}/docs`;
    if (spec.kind === "query") return runQuery(url => read(src, url, mayAsk), base, spec);
    const r = (await read(src, docUrl(base, spec.path), mayAsk)) as { doc: ApiDoc } | null;
    return r ? [wire(r.doc)] : [];
  }

  async function push(sub: string): Promise<void> {
    const s = subs.get(sub);
    if (!s?.live) return;
    const n = ++seq;
    fetches.set(sub, n);
    const live = () => !disposed && subs.get(sub) === s && fetches.get(sub) === n;
    try {
      const docs = await current(s.src, s.spec, false);
      if (!live()) return;
      if (s.spec.kind === "doc" && docs.length) {
        if (docs[0].version < (delivered.get(sub) ?? 0)) return;
        delivered.set(sub, docs[0].version);
      }
      env.post({ type: "clax:event", ns: "db", topic: "snapshot", data: { sub, docs } });
    } catch (e) {
      // A transient failure waits for the next event or `ready`.
      if (!live() || (e instanceof CapError && e.code === "unavailable")) return;
      drop(sub);
      env.post({ type: "clax:event", ns: "db", topic: "snapshot-error", data: { sub, code: e instanceof CapError ? e.code : "unavailable", message: e instanceof Error ? e.message : String(e) } });
    }
  }

  function schedule(sub: string): void {
    if (timers.has(sub)) return;
    timers.set(sub, setTimeout(() => { timers.delete(sub); void push(sub); }, SNAPSHOT_DEBOUNCE_MS));
  }

  function drop(sub: string): void {
    const s = subs.get(sub);
    subs.delete(sub);
    fetches.delete(sub);
    delivered.delete(sub);
    clearTimeout(timers.get(sub));
    timers.delete(sub);
    if (s && ![...subs.values()].some(x => x.src === s.src)) {
      watches.get(s.src)?.();
      watches.delete(s.src);
    }
  }

  /** An event of `src`'s reader topic: only B's `doc` events, and the
   * stream's own signals, cause a refetch; nothing is forwarded as is. */
  function onStream(src: string, e: StreamEvent): void {
    if (disposed) return;
    const doc = e.type === "doc" && e.topic === topicOf(src) && (e.artifact_id === undefined || e.artifact_id === src) && typeof e.path === "string";
    if (!doc && !STREAM_EVENTS.has(e.type)) return;
    for (const [sub, s] of subs) if (s.live && s.src === src && (!doc || touches(s.spec, e.path as string))) schedule(sub);
  }

  const endAll = () => { for (const sub of [...subs.keys()]) drop(sub); };

  return {
    async call(arg) {
      const a = (arg !== null && typeof arg === "object" ? arg : {}) as Record<string, unknown>;
      switch (a.op) {
        case "get": {
          const src = source(a.artifact);
          const r = (await read(src, docUrl(`/api/artifacts/${src}/docs`, checkPath(a.path, 0)), true)) as { doc: ApiDoc } | null;
          return r ? wire(r.doc) : null;
        }
        case "query": {
          const src = source(a.artifact);
          const spec = checkSpec(a.spec);
          if (spec.kind !== "query") throw new CapError("invalid_argument", "query takes a query");
          return current(src, spec, true);
        }
        case "subscribe": {
          const src = source(a.artifact);
          const sub = checkSubId(a.sub);
          const spec = checkSpec(a.spec);
          if (!subs.has(sub) && subs.size >= MAX_SUBSCRIPTIONS) throw new CapError("resource_exhausted", `at most ${MAX_SUBSCRIPTIONS} subscriptions of other apps per view`);
          drop(sub);
          // The slot is taken now, before any await, so a burst cannot pass the cap.
          const entry = { src, spec, live: false };
          subs.set(sub, entry);
          const gone = () => new CapError("unavailable", "the page's document changed before this subscription started");
          let docs: WireDoc[];
          try {
            // The first read asks for consent; only then is the topic followed.
            docs = await current(src, spec, true);
          } catch (e) {
            if (subs.get(sub) !== entry) throw gone();
            drop(sub);
            throw e;
          }
          if (disposed || subs.get(sub) !== entry) throw gone();
          entry.live = true;
          if (!watches.has(src)) watches.set(src, watch([topicOf(src)], e => onStream(src, e)));
          if (spec.kind === "doc" && docs.length) delivered.set(sub, docs[0].version);
          env.post({ type: "clax:event", ns: "db", topic: "snapshot", data: { sub, docs } });
          return null;
        }
        case "unsubscribe":
          drop(checkSubId(a.sub));
          return null;
        default:
          throw new CapError("capability_removed", `db.artifact(…) has no ${String(a.op)}`);
      }
    },
    reset: endAll,
    leave: endAll,
    dispose() {
      disposed = true;
      endAll();
    },
  };
}
