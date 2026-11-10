// How a local owner browser session's proof rides a request
// (`owner-proof.ts` reads it and installs it on the page's `fetch`). Kept
// apart, with no imports, so the stream worker (`stream-hub.ts`) and the
// extension's service worker that bundles it carry only these lines.

/** The header the proof rides in (`clax_http::local_trust::PROOF_HEADER`). */
export const PROOF_HEADER = "x-clax-owner-proof";
/** The query parameter the proof rides in on the realtime routes
 * (`clax_http::local_trust::PROOF_PARAM`). */
export const PROOF_PARAM = "owner_proof";

/** `url` (a path or a `ws:`/`http:` address) with `proof` as its
 * `PROOF_PARAM`; unchanged without one. */
export function withProof(url: string, proof: string | null): string {
  if (!proof) return url;
  return `${url}${url.includes("?") ? "&" : "?"}${PROOF_PARAM}=${encodeURIComponent(proof)}`;
}
