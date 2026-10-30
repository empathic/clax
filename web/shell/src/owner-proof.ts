// A local daemon's owner browser session is two secrets (docs/contract.md,
// "The owner and other viewers"): its cookie, which the browser sends to
// every port of the daemon's host name, and its proof, which the login's
// page (`GET /login`) wrote into this origin's `localStorage`, port
// included. The daemon counts the cookie as the owner's only beside the
// proof: the shell sends it on every request to its own origin
// (`PROOF_HEADER`), and in the address of the realtime routes whose clients
// send no header of their own (`PROOF_PARAM`: a WebSocket, the stream
// worker's hand-over). A hosted daemon, the Clax app and a browser with no
// session have no proof: nothing is added.
//
// A request refused for want of any credential (401 `no_credential`: the
// session ended while the page was open, `clax logout`) shows how to open
// Clax from the terminal, as a page load without one does.
import { isColdRefusal, showColdBrowser } from "./cold-browser";

/** The `localStorage` key the login's page keeps the proof under
 * (`clax_http::local_trust::PROOF_STORAGE_KEY`). */
export const PROOF_STORAGE_KEY = "clax.owner_proof";
/** The header the proof rides in (`clax_http::local_trust::PROOF_HEADER`). */
export const PROOF_HEADER = "x-clax-owner-proof";
/** The query parameter the proof rides in on the realtime routes
 * (`clax_http::local_trust::PROOF_PARAM`). */
export const PROOF_PARAM = "owner_proof";

/** This origin's proof, or null for none (or storage that cannot be read). */
export function ownerProof(storage: Pick<Storage, "getItem"> | null = safeStorage()): string | null {
  try {
    return storage?.getItem(PROOF_STORAGE_KEY) || null;
  } catch {
    return null;
  }
}

function safeStorage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** `url` (a path or a `ws:`/`http:` address) with `proof` as its
 * `PROOF_PARAM`; unchanged without one. */
export function withProof(url: string, proof: string | null = ownerProof()): string {
  if (!proof) return url;
  return `${url}${url.includes("?") ? "&" : "?"}${PROOF_PARAM}=${encodeURIComponent(proof)}`;
}

/** Whether `url`, resolved against `origin`, is on `origin`. */
function sameOrigin(url: string, origin: string): boolean {
  try {
    return new URL(url, origin).origin === origin;
  } catch {
    return false;
  }
}

/** `f` wrapped so every request to `origin` carries the proof `proof()`
 * gives at that moment (a login in another tab takes effect at once),
 * unless it names one already, and a 401 `no_credential` from `origin`'s
 * API calls `cold`. */
export function proofFetch(f: typeof fetch, origin: string, proof: () => string | null = ownerProof, cold: () => void = showColdBrowser): typeof fetch {
  return (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = input instanceof Request ? input.url : String(input);
    if (!sameOrigin(url, origin)) return f(input, init);
    const p = proof();
    let res: Response;
    if (p) {
      const headers = new Headers(init?.headers ?? (input instanceof Request ? input.headers : undefined));
      if (!headers.has(PROOF_HEADER)) headers.set(PROOF_HEADER, p);
      res = await f(input, { ...init, headers });
    } else {
      res = await f(input, init);
    }
    if (res.status === 401 && new URL(url, origin).pathname.startsWith("/api/") && (await isColdRefusal(res))) cold();
    return res;
  }) as typeof fetch;
}

let installed = false;
/** Wraps the page's `fetch` with `proofFetch`, once. */
export function installProofFetch(win: { fetch: typeof fetch; location: { origin: string } } = globalThis as unknown as Window): void {
  if (installed) return;
  installed = true;
  win.fetch = proofFetch(win.fetch.bind(win), win.location.origin);
}
