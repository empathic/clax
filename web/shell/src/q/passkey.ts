// The owner's passkeys for browser confirmations (docs/contract.md,
// "Browser confirmations", "Passkeys"): opt-in Touch ID or another passkey
// (WebAuthn with user verification) on every confirmation. The routes are
// the owner's browser's (`/api/passkeys…`); the ceremonies are the
// browser's own (`navigator.credentials`), which only a secure context
// (`http://localhost`) offers.
import { type Held, heldOf } from "../held";

/** The header that presents a confirmation's ID with the call it confirms:
 * an ID the daemon handed this shell, never a page's. */
const CONFIRMATION_HEADER = "x-clax-confirmation";

/** `GET /api/passkeys`. */
export type PasskeySettings = {
  available: boolean;
  unavailable_reason: string | null;
  rp_id: string | null;
  origin: string | null;
  /** The owner asked for passkeys to confirm. */
  require_passkey: boolean;
  /** Confirming takes a passkey now (asked for, and one registered). */
  required: boolean;
  offer_dismissed: boolean;
  /** Each with how it was added: from which browser, and the agents running then. */
  passkeys: { credential_id: string; alg: number; created_at: string; added_by?: { via?: string; agents_live?: string[] } }[];
};

/** A passkey's assertion, as the daemon takes it. */
export type Assertion = { credential_id: string; authenticator_data: string; client_data_json: string; signature: string };

/** A registration, as the daemon takes it. */
export type Registration = { credential_id: string; public_key: string; alg: number; authenticator_data: string; client_data_json: string };

/** A refusal: the daemon's words, or the browser's. */
export class PasskeyRefused extends Error {
  constructor(message: string, public code: string | null = null) {
    super(message);
    this.name = "PasskeyRefused";
  }
}

/** `bytes` as unpadded base64url. */
export function b64u(bytes: ArrayBuffer | Uint8Array): string {
  const b = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let s = "";
  for (const x of b) s += String.fromCharCode(x);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** Base64url `s` as bytes. */
export function fromB64u(s: string): Uint8Array<ArrayBuffer> {
  const bin = atob(s.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(s.length / 4) * 4, "="));
  const out = new Uint8Array(new ArrayBuffer(bin.length));
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Why this browser cannot use a passkey here, or null when it can. */
export function unavailableHere(): string | null {
  if (!globalThis.isSecureContext) return "This address is not a secure context: open Clax at http://localhost to use a passkey.";
  if (typeof PublicKeyCredential === "undefined" || !navigator.credentials) return "This browser offers no passkeys.";
  return null;
}

async function call<T>(path: string, method = "GET", body?: unknown, confirm?: string): Promise<T> {
  const headers: Record<string, string> = body === undefined ? {} : { "content-type": "application/json" };
  if (confirm) headers[CONFIRMATION_HEADER] = confirm;
  const res = await fetch(path, method === "GET" ? { cache: "no-store" } : { method, headers, body: JSON.stringify(body ?? {}) });
  const held = await heldOf(res);
  if (held) throw Object.assign(new PasskeyRefused(held.message, "confirmation_pending"), { held });
  let b: { error?: { code?: string; message?: string } } & T = {} as never;
  try { b = await res.json(); } catch { /* not json */ }
  if (res.ok) return b;
  throw new PasskeyRefused(b.error?.message ?? (res.statusText || `HTTP ${res.status}`), b.error?.code ?? null);
}

let cached: Promise<PasskeySettings | null> | null = null;

/** The passkey settings (cached; `fresh` reads them again); null where the daemon has none (hosted). */
export function passkeySettings(fresh = false): Promise<PasskeySettings | null> {
  if (fresh || !cached) cached = call<PasskeySettings>("/api/passkeys").catch(() => null);
  return cached;
}

type Challenge = { challenge: string; rp_id: string; user_id: string; timeout_ms: number; allow: string[] };
type Purpose = { purpose: "register" | "disable" } | { purpose: "confirm"; confirmation_id: string } | { purpose: "remove"; credential_id: string };

const challenge = (p: Purpose) => call<Challenge>("/api/passkeys/challenge", "POST", p);
const allow = (ids: string[]) => ids.map(id => ({ type: "public-key" as const, id: fromB64u(id) }));

/** Asks the browser for a passkey's assertion, user verified, over a new challenge for `p`. */
export async function assertFor(p: Purpose): Promise<Assertion> {
  const c = await challenge(p);
  const cred = (await navigator.credentials.get({
    publicKey: { challenge: fromB64u(c.challenge), rpId: c.rp_id, allowCredentials: allow(c.allow), userVerification: "required", timeout: c.timeout_ms },
  })) as PublicKeyCredential | null;
  if (!cred) throw new PasskeyRefused("No passkey was used.");
  const r = cred.response as AuthenticatorAssertionResponse;
  return { credential_id: b64u(cred.rawId), authenticator_data: b64u(r.authenticatorData), client_data_json: b64u(r.clientDataJSON), signature: b64u(r.signature) };
}

/** Asks the browser to make a passkey, user verified, over a new registration challenge. */
export async function makePasskey(): Promise<Registration> {
  const c = await challenge({ purpose: "register" });
  const cred = (await navigator.credentials.create({
    publicKey: {
      challenge: fromB64u(c.challenge),
      rp: { id: c.rp_id, name: "Clax" },
      user: { id: fromB64u(c.user_id), name: "owner", displayName: "Clax owner" },
      pubKeyCredParams: [{ type: "public-key", alg: -7 }, { type: "public-key", alg: -8 }],
      authenticatorSelection: { userVerification: "required", residentKey: "preferred" },
      attestation: "none",
      excludeCredentials: allow(c.allow),
      timeout: c.timeout_ms,
    },
  })) as PublicKeyCredential | null;
  if (!cred) throw new PasskeyRefused("No passkey was made.");
  const r = cred.response as AuthenticatorAttestationResponse;
  const key = r.getPublicKey();
  if (!key) throw new PasskeyRefused("This passkey's algorithm is not one Clax checks.");
  return { credential_id: b64u(cred.rawId), public_key: b64u(key), alg: r.getPublicKeyAlgorithm(), authenticator_data: b64u(r.getAuthenticatorData()), client_data_json: b64u(r.clientDataJSON) };
}

/** What registering came to: the settings after, or, while an agent is live, the confirmation it waits for. */
export type Registered = { settings: PasskeySettings } | { held: Held };

/** Registers `reg`, presenting confirmation `confirm` when it was held for one. */
export async function register(reg: Registration, confirm?: string): Promise<Registered> {
  try {
    const r = await call<{ settings: PasskeySettings }>("/api/passkeys", "POST", reg, confirm);
    cached = Promise.resolve(r.settings);
    return { settings: r.settings };
  } catch (e) {
    const held = (e as { held?: Held }).held;
    if (held) return { held };
    throw e;
  }
}

/** Sets the requirement or puts the offer off; turning it off takes `assertion`. */
export async function putSettings(body: { required?: boolean; offer_dismissed?: boolean; assertion?: Assertion }): Promise<PasskeySettings> {
  const s = await call<PasskeySettings>("/api/passkeys/settings", "PUT", body);
  cached = Promise.resolve(s);
  return s;
}

/** Removes passkey `id`; while passkeys are required it takes `assertion`. */
export async function removePasskey(id: string, assertion?: Assertion): Promise<PasskeySettings> {
  const s = await call<PasskeySettings>(`/api/passkeys/${encodeURIComponent(id)}/remove`, "POST", assertion ? { assertion } : {});
  cached = Promise.resolve(s);
  return s;
}
