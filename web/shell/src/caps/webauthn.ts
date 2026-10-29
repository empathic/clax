// The `webauthn` capability's shell side, in the owner's shell only: a
// page's `webauthn.approve({challenge, assertion})` asks the owner's passkey
// for a challenge a service on the shell's own origin issues, without the
// framed page ever touching a credential. The frame stays sandboxed (its
// origin is opaque, where WebAuthn cannot run); the shell runs the ceremony
// at top level, on its own origin, which is the passkeys' relying party.
//
// - `challenge` and `assertion` are paths on the shell's origin, each under
//   a prefix the page declares (`capabilities.webauthn.paths`); anything else
//   is refused before any request.
// - The shell reads the challenge (`GET`, `{ok, publicKey, intent?}`), shows
//   the owner its own dialog, naming what is approved (the challenge's
//   `intent`, `{to, text}`, when it names one), and only on Approve runs
//   `navigator.credentials.get` (user verification required), then posts the
//   assertion (`POST`, JSON) and answers the page with the service's answer.
// - Nothing is kept: every call asks. The page's own text never reaches the
//   dialog; only the service's.
// This module is a lazy chunk (registry.ts), loaded on the page's first call.
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

/** The codes a page may see from `webauthn.approve` (clax-extensions.d.ts `WebAuthn.ErrorCode`). */
export type WebAuthnCode = "invalid_argument" | "not_declared" | "refused" | "cancelled" | "unsupported" | "unavailable";

/** The path prefixes the page's declaration lists (each starts and ends with `/`). */
export function declaredPaths(declared: Record<string, Record<string, unknown> | undefined>): string[] {
  const paths = declared.webauthn?.paths;
  return Array.isArray(paths) ? paths.filter((p): p is string => typeof p === "string" && p.startsWith("/") && p.endsWith("/")) : [];
}

/** `p` as a path of this origin under one of `prefixes`, or why not. */
export function checkPath(p: unknown, prefixes: string[], what: string): string {
  if (typeof p !== "string" || !p.startsWith("/") || p.startsWith("//")) throw new CapError("invalid_argument", `${what} is a path on this origin, starting with /`);
  let u: URL;
  try {
    u = new URL(p, location.origin);
  } catch {
    throw new CapError("invalid_argument", `${what} is not a path`);
  }
  // The resolved path, so `..` cannot climb out of a declared prefix.
  if (u.origin !== location.origin || !prefixes.some(x => u.pathname.startsWith(x))) {
    throw new CapError("not_declared", `this app does not declare ${u.pathname} (capabilities.webauthn.paths)`);
  }
  return u.pathname + u.search;
}

const dec = (s: string): ArrayBuffer => {
  const b = atob(s.replace(/-/g, "+").replace(/_/g, "/") + "===".slice((s.length + 3) % 4));
  const u = new Uint8Array(b.length);
  for (let i = 0; i < b.length; i++) u[i] = b.charCodeAt(i);
  return u.buffer;
};
const enc = (buf: ArrayBuffer): string => {
  let s = "";
  for (const c of new Uint8Array(buf)) s += String.fromCharCode(c);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
};

type Challenge = {
  ok?: unknown; reason?: unknown;
  publicKey?: { challenge?: unknown; rpId?: unknown; timeout?: unknown; allowCredentials?: { id?: unknown }[] };
  intent?: { to?: unknown; text?: unknown };
};

async function json(res: Response): Promise<unknown> {
  try {
    return await res.json();
  } catch {
    throw new CapError("unavailable", `the service answered ${res.status}, not JSON`);
  }
}

/** What the dialog says is approved: the service's words, never the page's. */
export function dialogBody(title: string, c: Challenge): string {
  const to = typeof c.intent?.to === "string" ? c.intent.to : null;
  const text = typeof c.intent?.text === "string" ? c.intent.text : null;
  if (to !== null && text !== null) return `“${title}” asks you to approve sending this to ${to}:\n\n${text}`;
  return `“${title}” asks for your passkey on ${location.host}.`;
}

export const webauthnHandler: HandlerFactory = env => {
  const paths = declaredPaths(env.declared);
  let disposed = false;

  async function title(): Promise<string> {
    try {
      const r = await fetch(`/api/artifacts/${env.aid}`, { headers: { authorization: `Bearer ${env.token ?? ""}` } });
      const j = (await r.json()) as { artifact?: { title?: string } };
      return j.artifact?.title || env.aid;
    } catch {
      return env.aid;
    }
  }

  async function approve(arg: unknown): Promise<unknown> {
    if (env.token === null) throw new CapError("not_declared", "webauthn is the owner's only");
    const a = (arg !== null && typeof arg === "object" ? arg : {}) as Record<string, unknown>;
    const challenge = checkPath(a.challenge, paths, "challenge");
    const assertion = checkPath(a.assertion, paths, "assertion");
    if (typeof navigator.credentials?.get !== "function") throw new CapError("unsupported", "this browser has no passkeys");
    let c: Challenge;
    try {
      c = (await json(await fetch(challenge, { cache: "no-store" }))) as Challenge;
    } catch (e) {
      throw e instanceof CapError ? e : new CapError("unavailable", "the challenge could not be read");
    }
    const pk = c.publicKey;
    if (c.ok !== true || !pk || typeof pk.challenge !== "string") {
      throw new CapError("refused", typeof c.reason === "string" ? c.reason : "the service issued no challenge");
    }
    if (disposed) throw new CapError("unavailable", "this view has closed");
    const answer = await env.prompt({ kind: "capability", title: "Approve with your passkey?", body: dialogBody(await title(), c), allow: "Approve", deny: "Cancel" });
    if (answer !== "allow") throw new CapError("cancelled", "the owner did not approve");
    let cred: PublicKeyCredential;
    try {
      cred = (await navigator.credentials.get({
        publicKey: {
          challenge: dec(pk.challenge),
          ...(typeof pk.rpId === "string" ? { rpId: pk.rpId } : {}),
          allowCredentials: (pk.allowCredentials ?? []).filter(x => typeof x.id === "string").map(x => ({ type: "public-key" as const, id: dec(x.id as string) })),
          userVerification: "required",
          ...(typeof pk.timeout === "number" ? { timeout: pk.timeout } : {}),
        },
      })) as PublicKeyCredential;
    } catch (e) {
      if (e instanceof DOMException && e.name === "NotAllowedError") throw new CapError("cancelled", "the passkey prompt was cancelled or timed out");
      throw new CapError("refused", e instanceof Error ? e.message : String(e));
    }
    const r = cred.response as AuthenticatorAssertionResponse;
    const body = {
      id: cred.id, rawId: enc(cred.rawId), type: cred.type,
      response: { clientDataJSON: enc(r.clientDataJSON), authenticatorData: enc(r.authenticatorData), signature: enc(r.signature), userHandle: r.userHandle ? enc(r.userHandle) : null },
    };
    let res: Response;
    try {
      res = await fetch(assertion, { method: "POST", cache: "no-store", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
    } catch {
      throw new CapError("unavailable", "the assertion could not be sent");
    }
    return json(res);
  }

  return {
    async call(method, args) {
      if (method === "approve") return approve(args[0]);
      throw new CapError("capability_removed", `webauthn.${method} is not part of this runtime`);
    },
    dispose() { disposed = true; },
  };
};
