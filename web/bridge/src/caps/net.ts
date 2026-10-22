// The `net` namespace (clax-extensions.d.ts `ClaxExtensions.Net`): the page's
// `fetch(url, init)` goes to the shell, which has the daemon make the request
// (`shell/src/caps/net.ts`); the answer `{url, status, headers, body}` (or
// `body_base64` for a body that is not UTF-8) is wrapped here in a response
// with `ok`, `text()`, `json()`, `arrayBuffer()` and `blob()`, which cannot
// cross `postMessage`.
import type { Local } from "../capabilities";
import { CapabilityError, type Rpc } from "../rpc";

type Answer = { url: string; status: number; headers: Record<string, string>; body?: string; body_base64?: string };

/** The bytes of a base64 string. */
function fromBase64(b64: string): Uint8Array<ArrayBuffer> {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** The page's response around the shell's answer. */
export function netResponse(a: Answer) {
  const raw = typeof a.body_base64 === "string" ? a.body_base64 : null;
  const bytes = (): Uint8Array<ArrayBuffer> => (raw !== null ? fromBase64(raw) : new TextEncoder().encode(typeof a.body === "string" ? a.body : ""));
  const text = (): string => (raw !== null ? new TextDecoder().decode(fromBase64(raw)) : typeof a.body === "string" ? a.body : "");
  const headers = Object.freeze({ ...a.headers });
  return Object.freeze({
    status: a.status,
    ok: a.status >= 200 && a.status < 300,
    url: a.url,
    headers,
    text: () => Promise.resolve(text()),
    json: () => new Promise(resolve => resolve(JSON.parse(text()))),
    arrayBuffer: () => Promise.resolve(bytes().buffer),
    blob: () => Promise.resolve(new Blob([bytes()], { type: headers["content-type"] ?? "" })),
  });
}

export function netLocals(rpc: Pick<Rpc, "call">): Local {
  return {
    fetch: async (url: unknown, init: unknown = {}) => {
      if (typeof url !== "string") throw new CapabilityError("invalid_argument", "fetch takes a URL string");
      if (init === null || typeof init !== "object") throw new CapabilityError("invalid_argument", "fetch's second argument is {method, headers, body}");
      const { method, headers, body } = init as { method?: unknown; headers?: unknown; body?: unknown };
      const answer = (await rpc.call("net", "fetch", [{ url, method, headers, body }])) as Answer;
      return netResponse(answer);
    },
  };
}
