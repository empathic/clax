import { describe, expect, it } from "vitest";
import { base64url, canonical, ed25519Pkcs8V1, encodeAgentHeaders, fromBase64url, generateSigner, importPkcs8Signer, newNonce, NONCE_LEN, signature, type SignInput } from "./sign";

// The vector of `sdk_vector_is_fixed` in crates/clax-core/src/sig.rs: the
// Rust signer's canonical string and signature for these inputs.
const PKCS8 = "MFMCAQEwBQYDK2VwBCIEIEJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCoSMDIQAhUvjRm3kdJEUyQuFfLqtst8_6e2pe0wCXlg4GmIHbEg";
const PUBLIC = "IVL40Zt5HSRFMkLhXy6rbLfP-ntqXtMAl5YOBpiB2xI";
const INPUT: SignInput = {
  method: "POST",
  pathAndQuery: "/api/queues/q1/reporter/reports?cursor=c1",
  host: "clax.example:8443",
  t: 1_760_000_000,
  nonce: "AAECAwQFBgcICQoLDA0ODw",
  body: new TextEncoder().encode('{"body":"hi"}'),
  token: "cxr_vector",
};
const CANONICAL = [
  "clax-ed25519 v2",
  "POST",
  "/api/queues/q1/reporter/reports?cursor=c1",
  "clax.example:8443",
  "1760000000",
  "AAECAwQFBgcICQoLDA0ODw",
  "8f1de4697a700dd8df293b4b51025bff262ca9f65a2af030cd50acb3818eebfd",
  "1c2cec895430450c4ba5de411b6e3fa57ec583b47af14a0901bb47d3743e9ab1",
  "a00d0d823baf03fa53cba1260dd2f71fc7e8694679ce444737a7168dfab351e8",
].join("\n");
const HEADER = "t=1760000000, n=AAECAwQFBgcICQoLDA0ODw, s=8DwcAbZkGf5Qhq47ATzagkKAuIr-T4h6vKczyevVcv1UchPiD_ba5rmjr8HyInFyYULBDia1-PZesVfaOyxtBA";

describe("clax-ed25519 v2", () => {
  it("builds the Rust signer's canonical string", async () => {
    expect(await canonical(INPUT)).toBe(CANONICAL);
  });

  it("signs the Rust signer's vector with the grant's PKCS#8 key", async () => {
    const signer = await importPkcs8Signer(fromBase64url(PKCS8));
    expect(await signature(signer, INPUT)).toBe(HEADER);
  });

  it("signs an empty token line without a token", async () => {
    const lines = (await canonical({ ...INPUT, token: undefined })).split("\n");
    expect(lines).toHaveLength(9);
    expect(lines[7]).toBe("");
  });

  it("encodes the agent headers as the daemon does", () => {
    const e = encodeAgentHeaders(["01J9ZK3M4N5P6Q7R8S9T0V1W2X", "agent", "mcp", undefined, ""]);
    expect(new TextDecoder().decode(e)).toBe("+26:01J9ZK3M4N5P6Q7R8S9T0V1W2X\n+5:agent\n+3:mcp\n-\n+0:\n");
  });

  it("generates a non-extractable key whose signatures verify", async () => {
    const s = await generateSigner();
    expect(fromBase64url(s.publicKey)).toHaveLength(32);
    const header = await signature(s, { ...INPUT, nonce: newNonce() });
    const sig = fromBase64url(header.split("s=")[1]);
    const pub = await crypto.subtle.importKey("raw", fromBase64url(s.publicKey) as BufferSource, { name: "Ed25519" }, true, ["verify"]);
    const message = new TextEncoder().encode(await canonical({ ...INPUT, nonce: header.split("n=")[1].split(",")[0] }));
    expect(await crypto.subtle.verify("Ed25519", pub, sig as BufferSource, message as BufferSource)).toBe(true);
  });

  it("refuses control characters and makes canonical nonces", async () => {
    const signer = await importPkcs8Signer(fromBase64url(PKCS8));
    await expect(signature(signer, { ...INPUT, host: "a\nb" })).rejects.toThrow(/control character/);
    const n = newNonce();
    expect(n).toHaveLength(NONCE_LEN);
    expect(base64url(fromBase64url(n))).toBe(n);
    expect(base64url(fromBase64url(PUBLIC))).toBe(PUBLIC);
  });
});

// RFC 8032 section 7.1, test 1: the seed, its public key, and the
// signature of the empty message, in the three PKCS#8 layouts.
const hex = (s: string) => Uint8Array.from((s.match(/../g) ?? []).map(b => parseInt(b, 16)));
const toHex = (b: Uint8Array) => Array.from(b, x => x.toString(16).padStart(2, "0")).join("");
const SEED = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const RFC_PUBLIC = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
const EMPTY_SIG = "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b";
const V1 = `302e020100300506032b657004220420${SEED}`;
// ring's v2 (what a Clax grant's private_key is): [1] constructed BIT STRING.
const V2_RING = `3053020101300506032b657004220420${SEED}a123032100${RFC_PUBLIC}`;
// RFC 8410's v2: [1] implicit BIT STRING.
const V2_RFC = `3051020101300506032b657004220420${SEED}812100${RFC_PUBLIC}`;

describe("ed25519Pkcs8V1", () => {
  it("keeps v1 and re-wraps either v2 layout's seed as v1", () => {
    expect(hex(V1)).toHaveLength(48);
    expect(hex(V2_RING)).toHaveLength(85);
    expect(hex(V2_RFC)).toHaveLength(83);
    for (const doc of [V1, V2_RING, V2_RFC]) expect(toHex(ed25519Pkcs8V1(hex(doc)))).toBe(V1);
    // The fixed vector above is ring's layout too.
    expect(fromBase64url(PKCS8)).toHaveLength(85);
    expect(ed25519Pkcs8V1(fromBase64url(PKCS8))).toHaveLength(48);
  });

  it("leaves the caller's bytes alone", () => {
    const doc = hex(V2_RING);
    ed25519Pkcs8V1(doc);
    expect(toHex(doc)).toBe(V2_RING);
  });

  it("signs RFC 8032's empty message from each layout", async () => {
    for (const doc of [V1, V2_RING, V2_RFC]) {
      const signer = await importPkcs8Signer(hex(doc));
      expect(toHex(await signer.sign(new Uint8Array()))).toBe(EMPTY_SIG);
    }
  });

  it("refuses anything but those exact layouts", () => {
    const cases: [string, string][] = [
      ["empty", ""],
      ["v1 one byte short", V1.slice(0, -2)],
      ["v1 one byte long", V1 + "00"],
      ["v2 one byte short", V2_RING.slice(0, -2)],
      ["v2 one byte long", V2_RING + "00"],
      ["Ed448's OID", V1.replace("2b6570", "2b6571")],
      ["v1 marked version 1", V1.replace("020100", "020101")],
      ["v2 marked version 0", V2_RING.replace("3053020101", "3053020100")],
      ["v2 with a bad public-key marker", V2_RING.replace("a123032100", "a123032101")],
      ["v2 with the other layout's outer length", V2_RING.replace("3053", "3051")],
      ["v2 RFC layout under ring's length", V2_RFC.replace("3051", "3053")],
      ["not a SEQUENCE", V1.replace(/^30/, "31")],
    ];
    for (const [what, doc] of cases) expect(() => ed25519Pkcs8V1(hex(doc)), what).toThrow(/not an Ed25519 PKCS#8 private key/);
  });
});
