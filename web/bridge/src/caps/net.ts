// The `net` namespace (clax-extensions.d.ts `ClaxExtensions.Net`): the page's
// `fetch(url, init)` goes to the shell, which has the daemon make the request
// (`shell/src/caps/net.ts`); the answer `{url, status, headers, body}` is
// wrapped here in a response with `ok`, `text()` and `json()`, which cannot
// cross `postMessage`.
import type { Local } from "../capabilities";
import { CapabilityError, type Rpc } from "../rpc";

type Answer = { url: string; status: number; headers: Record<string, string>; body: string };

/** The page's response around the shell's answer. */
export function netResponse(a: Answer) {
  const body = typeof a.body === "string" ? a.body : "";
  return Object.freeze({
    status: a.status,
    ok: a.status >= 200 && a.status < 300,
    url: a.url,
    headers: Object.freeze({ ...a.headers }),
    text: () => Promise.resolve(body),
    json: () => new Promise(resolve => resolve(JSON.parse(body))),
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
