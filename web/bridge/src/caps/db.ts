// The `db` namespace (web/contract/0.2.61/db.d.ts): pure, synchronous refs and
// query builders; terminal calls go to the shell, which reaches the daemon as
// this viewer. Snapshots arrive as `db/snapshot` pushes; this module keeps the
// previous delivery so unchanged documents stay the same frozen objects.
import { CapabilityError, type Rpc } from "../rpc";

export type WireDoc = { path: string; id: string; data: Record<string, unknown>; version: number };
type Where = [string, string, unknown];
export type DbSpec =
  | { kind: "doc"; path: string }
  | { kind: "query"; collection: string; where: Where[]; orderBy: string | null; desc: boolean; limit: number | null };

export const MAX_SUBSCRIPTIONS = 64;
/** A document body's limit as serialized JSON (db.d.ts; the daemon's `MAX_DOC_BYTES`). */
export const MAX_DOC_BYTES = 256 * 1024;
const MAX_FILTERS = 10;
const MAX_IN = 30;
const OPS = new Set(["==", "!=", "<", "<=", ">", ">=", "in", "not-in", "array-contains"]);
const SEGMENT = /^[A-Za-z0-9_\-.~:@+]{1,200}$/;
const META = Object.freeze({ fromCache: false, hasPendingWrites: false });

function segments(path: unknown): string[] {
  if (typeof path !== "string") throw new TypeError("a db path is a string");
  if (new TextEncoder().encode(path).length > 1000) throw new TypeError("a db path is at most 1000 bytes");
  const segs = path.split("/");
  if (segs.length > 16) throw new TypeError(`a db path has at most 16 segments; '${path}' has ${segs.length}`);
  const bad = segs.find(s => !SEGMENT.test(s) || s === "." || s === "..");
  if (bad !== undefined) throw new TypeError(`'${bad}' is not a valid path segment: letters, digits and _ - . ~ : @ + only, 1 to 200 bytes, not . or ..`);
  return segs;
}

export function checkDocPath(path: string): string {
  const n = segments(path).length;
  if (n % 2 !== 0) throw new TypeError(`'${path}' has ${n} segments; a document path has an even number`);
  return path;
}

export function checkCollectionPath(path: string): string {
  const n = segments(path).length;
  if (n % 2 !== 1) throw new TypeError(`'${path}' has ${n} segments; a collection path has an odd number`);
  return path;
}

const invalid = (message: string) => new CapabilityError("invalid_argument", message);

/** Whether `v` holds only plain objects (prototype `Object.prototype` or
 * null), arrays, and scalars at every level, without cycles. */
function plainJson(v: unknown, path: Set<object> = new Set()): boolean {
  if (v === null || typeof v !== "object") return true;
  if (path.has(v)) return false; // a cycle
  path.add(v);
  const proto = Object.getPrototypeOf(v);
  const ok = Array.isArray(v)
    ? v.every(x => plainJson(x, path))
    : (proto === Object.prototype || proto === null) && Object.values(v).every(x => plainJson(x, path));
  path.delete(v);
  return ok;
}

function plainObject(v: unknown): asserts v is Record<string, unknown> {
  if (v === null || typeof v !== "object" || Array.isArray(v)) throw invalid("a document body is a plain JSON object");
  if (!plainJson(v)) throw invalid("a document body holds only plain objects, arrays, strings, numbers, booleans and null");
  let json: string;
  try {
    json = JSON.stringify(v);
  } catch (e) {
    throw invalid(`a document body is plain JSON: ${e instanceof Error ? e.message : String(e)}`);
  }
  if (new TextEncoder().encode(json).length > MAX_DOC_BYTES) throw invalid(`a document is at most ${MAX_DOC_BYTES} bytes as JSON`);
}

function segment(v: unknown, what: string): string {
  if (typeof v !== "string") throw new TypeError(`${what} is a string`);
  return v;
}

function deepFreeze<T>(v: T): T {
  if (v && typeof v === "object") {
    for (const x of Object.values(v as object)) deepFreeze(x);
    Object.freeze(v);
  }
  return v;
}

function newId(): string {
  const abc = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  return Array.from(crypto.getRandomValues(new Uint8Array(20)), b => abc[b % abc.length]).join("");
}

export type DocumentSnapshot = Readonly<{ id: string; exists: boolean; data(): Record<string, unknown> | undefined; metadata: typeof META }>;

export function snapshot(id: string, doc: WireDoc | null): DocumentSnapshot {
  const data = doc ? deepFreeze(structuredClone(doc.data)) : undefined;
  return Object.freeze({ id, exists: doc !== null, data: () => data, metadata: META });
}

type Change = { type: "added" | "modified" | "removed"; doc: DocumentSnapshot; oldIndex: number; newIndex: number };
export type SnapCache = Map<string, { snap: DocumentSnapshot; version: number; index: number }>;
export type QuerySnapshot = Readonly<{ docs: DocumentSnapshot[]; size: number; empty: boolean; docChanges(): Change[]; metadata: typeof META }>;

/** The query snapshot for `docs` after `prev` (null for the first delivery):
 * unchanged documents keep their snapshot objects; `changed` is false when
 * nothing differs from `prev`. Removals are listed first, then additions and
 * modifications (a document that only moved is `modified`). */
export function querySnapshot(docs: WireDoc[], prev: SnapCache | null): { snap: QuerySnapshot; cache: SnapCache; changed: boolean } {
  const cache: SnapCache = new Map();
  const out: DocumentSnapshot[] = [];
  const changes: Change[] = [];
  docs.forEach((d, i) => {
    const old = prev?.get(d.path);
    const snap = old && old.version === d.version ? old.snap : snapshot(d.id, d);
    cache.set(d.path, { snap, version: d.version, index: i });
    out.push(snap);
    if (!old) changes.push({ type: "added", doc: snap, oldIndex: -1, newIndex: i });
    else if (old.version !== d.version || old.index !== i) changes.push({ type: "modified", doc: snap, oldIndex: old.index, newIndex: i });
  });
  const removed: Change[] = [];
  for (const [path, old] of prev ?? []) if (!cache.has(path)) removed.push({ type: "removed", doc: old.snap, oldIndex: old.index, newIndex: -1 });
  const all = [...removed, ...changes];
  const frozen = Object.freeze(all.map(c => Object.freeze(c)));
  const snap: QuerySnapshot = Object.freeze({ docs: Object.freeze(out) as DocumentSnapshot[], size: out.length, empty: out.length === 0, docChanges: () => [...frozen], metadata: META });
  return { snap, cache, changed: prev === null || all.length > 0 };
}

type QState = { collection: string; where: Where[]; orderBy: string | null; desc: boolean; limit: number | null; orders: number };

function checkQuery(s: QState): void {
  if (s.where.length > MAX_FILTERS) throw invalid(`a query has at most ${MAX_FILTERS} filters`);
  for (const [field, op, value] of s.where) {
    if (typeof field !== "string" || !field) throw invalid("a where field is a non-empty string");
    if (!OPS.has(op)) throw invalid(`'${String(op)}' is not a query operator`);
    if ((op === "in" || op === "not-in") && !(Array.isArray(value) && value.length <= MAX_IN)) throw invalid(`in and not-in take an array of at most ${MAX_IN} values`);
  }
  if (s.orders > 1) throw invalid("a query has at most one orderBy");
  if (s.limit !== null && !(Number.isInteger(s.limit) && s.limit >= 1 && s.limit <= 1000)) throw invalid("limit is an integer from 1 to 1000");
}

const specOf = (s: QState): DbSpec => ({ kind: "query", collection: s.collection, where: s.where, orderBy: s.orderBy, desc: s.desc, limit: s.limit });

export function makeDb(rpc: Pick<Rpc, "call" | "on">) {
  let seq = 0;
  const active = new Set<string>();

  function subscribe(spec: DbSpec, deliver: (docs: WireDoc[]) => void, error?: (e: CapabilityError) => void): () => void {
    let dead = false;
    const report = (e: CapabilityError) => {
      if (error) { try { error(e); } catch (x) { reportError(x); } } else reportError(e);
    };
    if (active.size >= MAX_SUBSCRIPTIONS) {
      queueMicrotask(() => report(new CapabilityError("resource_exhausted", `at most ${MAX_SUBSCRIPTIONS} subscriptions per view`)));
      return () => {};
    }
    const sub = `s${++seq}`;
    active.add(sub);
    const offSnap = rpc.on("db", "snapshot", d => {
      const x = d as { sub: string; docs: WireDoc[] };
      if (dead || x.sub !== sub) return;
      try { deliver(x.docs); } catch (e) { reportError(e); }
    });
    const offErr = rpc.on("db", "snapshot-error", d => {
      const x = d as { sub: string; code: string; message: string };
      if (x.sub === sub) fail(new CapabilityError(x.code, x.message));
    });
    const stop = () => {
      offSnap();
      offErr();
      active.delete(sub);
      void rpc.call("db", "unsubscribe", [sub]).catch(() => {});
    };
    function fail(e: unknown) {
      if (dead) return;
      dead = true;
      stop();
      report(e instanceof CapabilityError ? e : new CapabilityError("unavailable", String(e)));
    }
    try {
      if (spec.kind === "query") checkQuery({ ...spec, orders: spec.orderBy ? 1 : 0 });
    } catch (e) {
      queueMicrotask(() => fail(e));
      return () => { if (!dead) { dead = true; stop(); } };
    }
    rpc.call("db", "subscribe", [sub, spec]).catch(fail);
    return () => { if (!dead) { dead = true; stop(); } };
  }

  function query(s: QState) {
    return {
      where: (field: string, op: string, value: unknown) => query({ ...s, where: [...s.where, [field, op, value]] }),
      orderBy: (field: string, dir: "asc" | "desc" = "asc") => {
        if (dir !== "asc" && dir !== "desc") throw new TypeError(`orderBy's direction is "asc" or "desc", not '${String(dir)}'`);
        return query({ ...s, orderBy: field, desc: dir === "desc", orders: s.orders + 1 });
      },
      limit: (n: number) => query({ ...s, limit: n }),
      async get(): Promise<QuerySnapshot> {
        checkQuery(s);
        return querySnapshot((await rpc.call("db", "query", [specOf(s)])) as WireDoc[], null).snap;
      },
      onSnapshot(next: (snap: QuerySnapshot) => void, error?: (e: CapabilityError) => void): () => void {
        if (s.orders > 1) {
          let off = false;
          queueMicrotask(() => {
            if (off) return;
            const e = invalid("a query has at most one orderBy");
            if (error) error(e);
            else reportError(e);
          });
          return () => { off = true; };
        }
        let cache: SnapCache | null = null;
        return subscribe(specOf(s), docs => {
          const r = querySnapshot(docs, cache);
          cache = r.cache;
          if (r.changed) next(r.snap);
        }, error);
      },
    };
  }

  function docRef(path: string) {
    checkDocPath(path);
    const id = path.slice(path.lastIndexOf("/") + 1);
    return {
      id,
      path,
      async get(): Promise<DocumentSnapshot> {
        return snapshot(id, (await rpc.call("db", "get", [path])) as WireDoc | null);
      },
      async set(data: Record<string, unknown>): Promise<void> {
        plainObject(data);
        await rpc.call("db", "set", [path, data]);
      },
      async update(data: Record<string, unknown>): Promise<void> {
        plainObject(data);
        await rpc.call("db", "update", [path, data]);
      },
      async delete(): Promise<void> {
        await rpc.call("db", "delete", [path]);
      },
      async acquire(options: { holder: string; ttlMs?: number; data?: Record<string, unknown> }) {
        if (!options || typeof options.holder !== "string" || !options.holder) throw invalid("acquire needs {holder: string}");
        if (options.data !== undefined) plainObject(options.data);
        return rpc.call("db", "acquire", [path, { holder: options.holder, ttlMs: options.ttlMs, data: options.data }]);
      },
      onSnapshot(next: (snap: DocumentSnapshot) => void, error?: (e: CapabilityError) => void): () => void {
        let last: { version: number; snap: DocumentSnapshot } | null = null;
        return subscribe({ kind: "doc", path }, docs => {
          const d = docs[0] ?? null;
          const version = d ? d.version : 0;
          if (last && last.version === version) return;
          last = { version, snap: snapshot(id, d) };
          next(last.snap);
        }, error);
      },
      collection: (sub: string) => collectionRef(`${path}/${segment(sub, "a collection ID")}`),
    };
  }

  function collectionRef(path: string) {
    checkCollectionPath(path);
    const ref = {
      ...query({ collection: path, where: [], orderBy: null, desc: false, limit: null, orders: 0 }),
      path,
      doc: (id?: string) => docRef(`${path}/${id === undefined ? newId() : segment(id, "a document ID")}`),
      async add(data: Record<string, unknown>) {
        const d = ref.doc();
        await d.set(data);
        return d;
      },
    };
    return ref;
  }

  return { doc: (path: string) => docRef(path), collection: (path: string) => collectionRef(path) };
}
