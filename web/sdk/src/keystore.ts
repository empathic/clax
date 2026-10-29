// Where a reporter's key and token are kept between page loads, so a
// reporter is minted a token once per token lifetime (15 minutes), not on
// every page load. The key is a non-extractable Ed25519 CryptoKey, which
// IndexedDB stores as it is: its private half never becomes bytes a page
// script can read. Everything kept is for one (instance, queue, user).

/** A reporter's key and the token last minted for it. */
export interface Kept {
  /** The private key: non-extractable. */
  privateKey: CryptoKey;
  /** Its public half in unpadded base64url. */
  publicKey: string;
  token?: { token: string; expiresAt: number };
}

/** Keeps one reporter's key and token. A store that fails (no IndexedDB,
 * a private window refusing it, a browser that cannot clone a CryptoKey)
 * is given up on, and the client keeps them for the page alone. */
export interface ReporterStore {
  load(): Promise<Kept | null>;
  save(k: Kept): Promise<void>;
  /** Forgets the token, keeping the key (a refused token, or a sign-out). */
  forgetToken(): Promise<void>;
}

const DB = "clax-reporter";
const STORE = "reporters";

/** SHA-256 of `s`, in lowercase hex: a user's ID is never stored as given. */
async function digest(s: string): Promise<string> {
  const h = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s)));
  return Array.from(h, b => b.toString(16).padStart(2, "0")).join("");
}

function done<T>(r: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error ?? new Error("IndexedDB request failed"));
  });
}

/** The reporter store in `idb` (default: the page's `indexedDB`) for
 * `user` of `queue` on `instance`, or `null` where there is no IndexedDB. */
export function indexedDbStore(instance: string, queue: string, user: string, idb: IDBFactory | undefined = globalThis.indexedDB): ReporterStore | null {
  if (!idb) return null;
  let opened: Promise<IDBDatabase> | undefined;
  const db = () => {
    opened ??= new Promise((resolve, reject) => {
      const r = idb.open(DB, 1);
      r.onupgradeneeded = () => { r.result.createObjectStore(STORE); };
      r.onsuccess = () => resolve(r.result);
      r.onerror = () => reject(r.error ?? new Error("IndexedDB would not open"));
      r.onblocked = () => reject(new Error("IndexedDB is blocked"));
    });
    return opened;
  };
  let named: Promise<string> | undefined;
  const name = () => (named ??= digest(`${instance}\n${queue}\n${user}`));
  // Each request is made in the same turn as its transaction, which
  // commits once it has no request left.
  const get = async () => {
    const [d, k] = await Promise.all([db(), name()]);
    return ((await done(d.transaction(STORE, "readonly").objectStore(STORE).get(k))) as Kept | undefined) ?? null;
  };
  const put = async (kept: Kept) => {
    const [d, k] = await Promise.all([db(), name()]);
    await done(d.transaction(STORE, "readwrite").objectStore(STORE).put(kept, k));
  };
  return {
    load: get,
    save: put,
    forgetToken: async () => {
      const kept = await get();
      if (kept) await put({ privateKey: kept.privateKey, publicKey: kept.publicKey });
    },
  };
}
