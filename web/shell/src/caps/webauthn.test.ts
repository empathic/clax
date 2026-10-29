import { afterEach, describe, expect, it, vi } from "vitest";
import type { CapEnv } from "./host";
import { checkPath, declaredPaths, dialogBody, webauthnHandler } from "./webauthn";

const AID = "7q3k9mzx2b4t";
const declared = { webauthn: { paths: ["/approve/v1/"] } };
const b64u = (s: string) => btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const buf = (s: string) => new TextEncoder().encode(s).buffer;

const challenge = { ok: true, publicKey: { challenge: b64u("chal"), rpId: location.hostname, allowCredentials: [{ id: b64u("cred") }], timeout: 60000 }, intent: { to: "work-human", text: "Ship it" } };

function env(prompt: CapEnv["prompt"], token: string | null = "tk"): CapEnv {
  return { aid: AID, token, declared, prompt } as unknown as CapEnv;
}

/** fetch answering the artifact, the challenge and the assertion; the requests made. */
function routes(answers: { challenge?: unknown; assertion?: unknown } = {}) {
  const seen: { url: string; init?: RequestInit }[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    seen.push({ url, init });
    if (url === `/api/artifacts/${AID}`) return new Response(JSON.stringify({ artifact: { title: "Fleet" } }));
    if (url.startsWith("/approve/v1/challenge/")) return new Response(JSON.stringify(answers.challenge ?? challenge));
    if (url.startsWith("/approve/v1/assertion/")) return new Response(JSON.stringify(answers.assertion ?? { ok: true, message: "sent" }));
    return new Response("{}", { status: 404 });
  }));
  return seen;
}

function credentials(get: () => Promise<unknown>) {
  Object.defineProperty(navigator, "credentials", { value: { get: vi.fn(get) }, configurable: true });
  return (navigator.credentials as unknown as { get: ReturnType<typeof vi.fn> }).get;
}

const assertionCred = () => Promise.resolve({
  id: "cred", rawId: buf("cred"), type: "public-key",
  response: { clientDataJSON: buf("cd"), authenticatorData: buf("ad"), signature: buf("sig"), userHandle: null },
});

const req = { challenge: "/approve/v1/challenge/i1", assertion: "/approve/v1/assertion/i1" };

describe("webauthn in the shell", () => {
  afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

  it("reads only declared prefixes, and paths of this origin under them", () => {
    expect(declaredPaths({ webauthn: { paths: ["/approve/v1/", "nope", 7] } })).toEqual(["/approve/v1/"]);
    expect(declaredPaths({})).toEqual([]);
    expect(checkPath("/approve/v1/challenge/x?y=1", ["/approve/v1/"], "challenge")).toBe("/approve/v1/challenge/x?y=1");
    for (const [p, code] of [["/approve/v1/../../api/token", "not_declared"], ["/api/token", "not_declared"], ["//evil.example/approve/v1/", "invalid_argument"], ["https://evil.example/approve/v1/x", "invalid_argument"], [7, "invalid_argument"]] as const) {
      expect(() => checkPath(p, ["/approve/v1/"], "challenge")).toThrow(expect.objectContaining({ code }));
    }
  });

  it("names the service's intent in the dialog, never the page's words", () => {
    expect(dialogBody("Fleet", challenge)).toBe("“Fleet” asks you to approve sending this to work-human:\n\nShip it");
    expect(dialogBody("Fleet", { ok: true })).toBe(`“Fleet” asks for your passkey on ${location.host}.`);
  });

  it("asks, then runs the passkey at top level and posts the assertion", async () => {
    const seen = routes();
    const get = credentials(assertionCred);
    const prompt = vi.fn(async (_p: unknown) => "allow" as const);
    const h = webauthnHandler(env(prompt), null as never);
    await expect(h.call("approve", [req])).resolves.toEqual({ ok: true, message: "sent" });
    expect(prompt).toHaveBeenCalledTimes(1);
    expect(prompt.mock.calls[0]).toEqual([expect.objectContaining({ title: "Approve with your passkey?", allow: "Approve", body: expect.stringContaining("Ship it") })]);
    expect(prompt.mock.calls[0][0]).not.toHaveProperty("always");
    const pk = get.mock.calls[0][0].publicKey;
    expect(new TextDecoder().decode(pk.challenge)).toBe("chal");
    expect(pk.userVerification).toBe("required");
    expect(new TextDecoder().decode(pk.allowCredentials[0].id)).toBe("cred");
    const post = seen.find(s => s.url === req.assertion)!;
    expect(post.init?.method).toBe("POST");
    expect(JSON.parse(post.init!.body as string)).toEqual({ id: "cred", rawId: b64u("cred"), type: "public-key", response: { clientDataJSON: b64u("cd"), authenticatorData: b64u("ad"), signature: b64u("sig"), userHandle: null } });
  });

  it("asks no passkey on Cancel, a refused challenge, an undeclared path, or for another viewer", async () => {
    routes();
    const get = credentials(assertionCred);
    const no = webauthnHandler(env(async () => "deny"), null as never);
    await expect(no.call("approve", [req])).rejects.toMatchObject({ code: "cancelled" });
    routes({ challenge: { ok: false, reason: "expired" } });
    const yes = webauthnHandler(env(async () => "allow"), null as never);
    await expect(yes.call("approve", [req])).rejects.toMatchObject({ code: "refused", message: "expired" });
    await expect(yes.call("approve", [{ ...req, assertion: "/api/token" }])).rejects.toMatchObject({ code: "not_declared" });
    const visitor = webauthnHandler(env(async () => "allow", null), null as never);
    await expect(visitor.call("approve", [req])).rejects.toMatchObject({ code: "not_declared" });
    expect(get).not.toHaveBeenCalled();
  });

  it("says a cancelled passkey prompt is cancelled", async () => {
    routes();
    credentials(() => Promise.reject(new DOMException("no", "NotAllowedError")));
    const h = webauthnHandler(env(async () => "allow"), null as never);
    await expect(h.call("approve", [req])).rejects.toMatchObject({ code: "cancelled" });
  });
});
