// `clax-ed25519 v2` request signing (crates/clax-core/src/sig.rs) on
// WebCrypto alone, so it runs in a browser, in Node and in a Worker.
//
// A request signs the canonical lines of `SignInput::canonical`: the
// version line, the method, the path and query as sent, the `Host`, the
// time `t` in Unix seconds, a nonce, the lowercase hex SHA-256 of the body,
// of the token (or an empty line) and of the agent headers' encoding,
// joined by `\n` with no trailing newline. The signature travels as
// `Clax-Signature: t=<t>, n=<nonce>, s=<base64url signature>`.

/** First line of the canonical string (`clax_core::sig::VERSION_LINE`). */
export const VERSION_LINE = "clax-ed25519 v2";
/** The agent-side headers a signature covers, in order (`clax_core::sig::AGENT_HEADERS`). */
export const AGENT_HEADERS = ["x-clax-session", "x-clax-channel", "x-clax-via", "x-clax-git", "x-clax-call"] as const;
/** Length of a nonce: 16 random bytes in unpadded base64url. */
export const NONCE_LEN = 22;

/** What a request signs (`clax_core::sig::SignInput`). */
export interface SignInput {
  method: string;
  /** The path and query exactly as sent on the request line. */
  pathAndQuery: string;
  /** The `Host` the request is sent with: the instance URL's host and port. */
  host: string;
  /** Unix seconds. */
  t: number;
  nonce: string;
  body: Uint8Array;
  /** The token the request carries; `undefined` signs an empty token line. */
  token: string | undefined;
  /** The values of [`AGENT_HEADERS`] the request sends, in their order;
   * absent means none of them. */
  agent?: readonly (string | undefined)[];
}

/** Something that signs with an Ed25519 private key. */
export interface Signer {
  sign(message: Uint8Array): Promise<Uint8Array>;
}

/** A signer whose public key is known: a reporter's, sent to the deployer's
 * backend for a reporter token. */
export interface KeyedSigner extends Signer {
  /** The 32 public bytes in unpadded base64url (`clax_core::sig::encode_public`). */
  publicKey: string;
}

const utf8 = new TextEncoder();

/** `bytes` in unpadded base64url. */
export function base64url(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** The bytes of unpadded (or padded) base64url `s`. */
export function fromBase64url(s: string): Uint8Array {
  const b = atob(s.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(b, c => c.charCodeAt(0));
}

/** The lowercase hex SHA-256 of `bytes`. */
export async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const d = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes as BufferSource));
  return Array.from(d, b => b.toString(16).padStart(2, "0")).join("");
}

/** The fixed serialization of the agent headers' values
 * (`clax_core::sig::encode_agent_headers`): per header in [`AGENT_HEADERS`]
 * order, `-\n` when absent, else `+<byte length>:<value>\n`. */
export function encodeAgentHeaders(values: readonly (string | undefined)[] = []): Uint8Array {
  let out = "";
  for (let i = 0; i < AGENT_HEADERS.length; i++) {
    const v = values[i];
    out += v === undefined ? "-\n" : `+${utf8.encode(v).length}:${v}\n`;
  }
  return utf8.encode(out);
}

/** The signed message (`clax_core::sig::SignInput::canonical`). */
export async function canonical(i: SignInput): Promise<string> {
  const token = i.token === undefined ? "" : await sha256Hex(utf8.encode(i.token));
  return [
    VERSION_LINE, i.method, i.pathAndQuery, i.host, String(i.t), i.nonce,
    await sha256Hex(i.body), token, await sha256Hex(encodeAgentHeaders(i.agent)),
  ].join("\n");
}

/** A fresh nonce: 16 random bytes in unpadded base64url ([`NONCE_LEN`] characters). */
export function newNonce(): string {
  return base64url(crypto.getRandomValues(new Uint8Array(16)));
}

/** The `Clax-Signature` value for `i`, signed by `signer`.
 *
 * Throws for a control character in the method, path, host or nonce, which
 * the daemon refuses whatever the signature. */
export async function signature(signer: Signer, i: SignInput): Promise<string> {
  // eslint-disable-next-line no-control-regex
  if (/[\x00-\x1f\x7f]/.test(i.method + i.pathAndQuery + i.host + i.nonce)) throw new Error("a request field holds a control character");
  const s = await signer.sign(utf8.encode(await canonical(i)));
  return `t=${i.t}, n=${i.nonce}, s=${base64url(s)}`;
}

function cryptoKeySigner(key: CryptoKey): Signer {
  return { sign: async m => new Uint8Array(await crypto.subtle.sign("Ed25519", key, m as BufferSource)) };
}

/** A new Ed25519 key whose private half cannot be exported: the
 * CryptoKey (which IndexedDB keeps as it is) and the public half in
 * unpadded base64url. */
export async function generateKeyPair(): Promise<{ privateKey: CryptoKey; publicKey: string }> {
  const pair = await crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"]) as CryptoKeyPair;
  const raw = new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey));
  return { privateKey: pair.privateKey, publicKey: base64url(raw) };
}

/** The signer of `privateKey`, whose public half is `publicKey`. */
export function keyedSigner(privateKey: CryptoKey, publicKey: string): KeyedSigner {
  return { ...cryptoKeySigner(privateKey), publicKey };
}

/** A new Ed25519 key whose private half cannot be exported. */
export async function generateSigner(): Promise<KeyedSigner> {
  const k = await generateKeyPair();
  return keyedSigner(k.privateKey, k.publicKey);
}

/** A signer over the Ed25519 private key in PKCS#8 `doc` (a queue service
 * grant's `private_key`, decoded), imported as non-extractable. */
export async function importPkcs8Signer(doc: Uint8Array): Promise<Signer> {
  const key = await crypto.subtle.importKey("pkcs8", doc as BufferSource, { name: "Ed25519" }, false, ["sign"]);
  return cryptoKeySigner(key);
}
