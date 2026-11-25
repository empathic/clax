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

/** The DER of an Ed25519 PKCS#8 v1 `PrivateKeyInfo` up to its 32-byte
 * seed: SEQUENCE (46), version 0, AlgorithmIdentifier id-Ed25519
 * (1.3.101.112), OCTET STRING (34) holding OCTET STRING (32). */
const V1_HEAD = [0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];
/** The v2 forms' (`OneAsymmetricKey`, RFC 5958) tails after the seed,
 * each followed by the 32-byte public key: ring's `[1]` constructed BIT
 * STRING, which a Clax grant's `private_key` is (85 bytes in all), and
 * RFC 8410's `[1]` implicit one (83 bytes). */
const V2_FORMS: { outer: number; tail: number[] }[] = [
  { outer: 0x53, tail: [0xa1, 0x23, 0x03, 0x21, 0x00] },
  { outer: 0x51, tail: [0x81, 0x21, 0x00] },
];

/** The Ed25519 private key in PKCS#8 `doc` as the 48-byte v1 form, which
 * every WebCrypto imports (Bun's reads no other): `doc` itself when it is
 * v1, its seed re-wrapped when it is v2 with the public key attached (the
 * form a Clax service grant's `private_key` has). Anything else is
 * refused: only these exact layouts are read.
 *
 * @throws Error `not an Ed25519 PKCS#8 private key` for any other bytes. */
export function ed25519Pkcs8V1(doc: Uint8Array): Uint8Array {
  const bad = () => new Error(`not an Ed25519 PKCS#8 private key (${doc.length} bytes)`);
  const at = (bytes: number[], off: number) => bytes.every((b, i) => doc[off + i] === b);
  const seedEnd = V1_HEAD.length + 32;
  if (doc.length === seedEnd) {
    if (!at(V1_HEAD, 0)) throw bad();
    return doc.slice();
  }
  const form = V2_FORMS.find(f => doc.length === 2 + f.outer && doc[1] === f.outer);
  // The v2 head is v1's with its outer length and version 1.
  if (!form || doc[0] !== 0x30 || !at([0x02, 0x01, 0x01], 2) || !at(V1_HEAD.slice(5), 5) || !at(form.tail, seedEnd) || doc.length !== seedEnd + form.tail.length + 32) {
    throw bad();
  }
  const out = new Uint8Array(seedEnd);
  out.set(V1_HEAD, 0);
  out.set(doc.subarray(V1_HEAD.length, seedEnd), V1_HEAD.length);
  return out;
}

/** A signer over the Ed25519 private key in PKCS#8 `doc` (a queue service
 * grant's `private_key`, decoded), imported as non-extractable; `doc` may
 * be v1 or v2 ([`ed25519Pkcs8V1`]). */
export async function importPkcs8Signer(doc: Uint8Array): Promise<Signer> {
  const v1 = ed25519Pkcs8V1(doc);
  try {
    const key = await crypto.subtle.importKey("pkcs8", v1 as BufferSource, { name: "Ed25519" }, false, ["sign"]);
    return cryptoKeySigner(key);
  } finally {
    v1.fill(0);
  }
}
