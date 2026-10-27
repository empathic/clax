import { describe, expect, it } from "vitest";
import { base64url, canonical, encodeAgentHeaders, fromBase64url, generateSigner, importPkcs8Signer, newNonce, NONCE_LEN, signature, type SignInput } from "./sign";

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
