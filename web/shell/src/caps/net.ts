// The `net` capability's shell side (spec 2026-10-12-flutter-real-apps-design
// C6), in the owner's shell only (it needs the token): the page's
// `net.fetch(url, init)` goes to `POST /api/artifacts/<aid>/net:fetch`, and
// the daemon makes the request, so the page's content security policy keeps
// `connect-src 'none'`.
//
// - A URL that is not https, or whose host the page's declaration
//   (`capabilities.net.hosts`) does not list, is refused before any request.
// - The daemon's `not_granted` (the owner has not allowed the app's hosts)
//   asks the owner once per view, for the whole host list ("Let “A” use the
//   internet?", naming every host); Allow grants exactly the hosts shown
//   (`PUT /api/artifacts/<aid>/net-grants {hosts}`) and the fetch is tried again.
//   The grant lives in the daemon, so the next view does not ask. Nobody is
//   asked in the app's inspect view (`?app=1&inspect=1`): a refusal stands.
// - The answer is `{url, status, headers, body}`; the bridge builds the
//   page's response object around it (`bridge/src/caps/net.ts`).
// This module is a lazy chunk (registry.ts), loaded on the page's first call.
import { inspectQuery } from "../route";
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

/** The codes a page may see from `net.fetch` (clax-extensions.d.ts `NetErrorCode`). */
const NET_CODES = new Set(["invalid_argument", "https_only", "not_declared", "not_granted", "redirect_refused", "blocked_address", "too_large", "too_many_redirects", "timeout", "upstream_error", "unavailable", "session_expired"]);

/** The daemon's answer: `body` when the response is UTF-8, else `body_base64`. */
export type NetResponse = { url: string; status: number; headers: Record<string, string>; body?: string; body_base64?: string };

/** The hosts the page's declaration lists. */
export function declaredHosts(declared: Record<string, Record<string, unknown> | undefined>): string[] {
  const hosts = declared.net?.hosts;
  return Array.isArray(hosts) ? hosts.filter((h): h is string => typeof h === "string") : [];
}

async function refusal(res: Response): Promise<CapError> {
  let code = "";
  let message = `HTTP ${res.status}`;
  try {
    const e = (await res.json()).error as { code?: unknown; message?: unknown };
    code = String(e?.code ?? "");
    if (typeof e?.message === "string") message = e.message;
  } catch { /* not JSON */ }
  if (res.status === 401) return new CapError("session_expired", "the page's session with Clax ended; reload the page");
  if (res.status === 404) return new CapError("not_declared", message);
  return new CapError(NET_CODES.has(code) ? code : "upstream_error", message);
}

/** `{url, method, headers, body}` as the page's bridge sent it, checked. */
function checkRequest(arg: unknown): { url: string; method: string; headers: Record<string, string>; body?: string } {
  const a = (arg !== null && typeof arg === "object" ? arg : {}) as Record<string, unknown>;
  if (typeof a.url !== "string") throw new CapError("invalid_argument", "fetch takes a URL string");
  const method = a.method === undefined ? "GET" : a.method;
  if (typeof method !== "string" || !/^[A-Za-z]+$/.test(method)) throw new CapError("invalid_argument", "method is an HTTP method name");
  const headers: Record<string, string> = {};
  if (a.headers !== undefined) {
    if (a.headers === null || typeof a.headers !== "object" || Array.isArray(a.headers)) throw new CapError("invalid_argument", "headers is an object of strings");
    for (const [k, v] of Object.entries(a.headers)) {
      if (typeof v !== "string") throw new CapError("invalid_argument", `header ${k} is not a string`);
      headers[k] = v;
    }
  }
  if (a.body !== undefined && a.body !== null && typeof a.body !== "string") throw new CapError("invalid_argument", "body is a string (JSON.stringify an object)");
  return { url: a.url, method: method.toUpperCase(), headers, ...(typeof a.body === "string" ? { body: a.body } : {}) };
}

export const netHandler: HandlerFactory = env => {
  const auth = { authorization: `Bearer ${env.token ?? ""}` };
  const hosts = declaredHosts(env.declared);
  /** The owner's answer, asked at most once per view. */
  let asked: Promise<boolean> | null = null;
  let disposed = false;

  async function title(): Promise<string> {
    try {
      const r = await fetch(`/api/artifacts/${env.aid}`, { headers: auth });
      const j = (await r.json()) as { artifact?: { title?: string } };
      return j.artifact?.title || env.aid;
    } catch {
      return env.aid;
    }
  }

  async function ask(): Promise<boolean> {
    const a = await title();
    // Every host is shown (the dialog's text scrolls when long), and exactly
    // these are granted: the daemon grants only those the live version still
    // declares.
    const list = hosts.join(", ");
    const answer = await env.prompt({
      title: `Let “${a}” use the internet?`,
      body: `“${a}” will send requests to ${list}, and read what they answer. You can take this back in the app's inspector.`,
      allow: "Allow",
      deny: "Don't allow",
    });
    if (answer !== "allow") return false;
    const res = await fetch(`/api/artifacts/${env.aid}/net-grants`, { method: "PUT", headers: { "content-type": "application/json", ...auth }, body: JSON.stringify({ hosts }) }).catch(() => null);
    if (!res?.ok) {
      asked = null;
      throw new CapError("unavailable", "the grant could not be saved");
    }
    return true;
  }

  async function send(req: ReturnType<typeof checkRequest>): Promise<NetResponse> {
    if (disposed) throw new CapError("unavailable", "this view has closed");
    let res: Response;
    try {
      res = await fetch(`/api/artifacts/${env.aid}/net:fetch`, { method: "POST", headers: { "content-type": "application/json", ...auth }, body: JSON.stringify(req) });
    } catch {
      throw new CapError("unavailable", "the Clax daemon could not be reached");
    }
    if (!res.ok) throw await refusal(res);
    return (await res.json()) as NetResponse;
  }

  async function fetchFor(arg: unknown): Promise<NetResponse> {
    const req = checkRequest(arg);
    let url: URL;
    try {
      url = new URL(req.url);
    } catch {
      throw new CapError("invalid_argument", `${req.url} is not an absolute URL`);
    }
    if (url.protocol !== "https:") throw new CapError("https_only", `${req.url} is not an https URL`);
    if (!hosts.includes(url.hostname)) throw new CapError("not_declared", `this app does not declare ${url.hostname} (capabilities.net.hosts)`);
    try {
      return await send(req);
    } catch (e) {
      if (!(e instanceof CapError && e.code === "not_granted") || env.token === null || inspectQuery(location.search)) throw e;
      asked ??= ask();
      if (!(await asked)) throw e;
      return send(req);
    }
  }

  return {
    async call(method, args) {
      if (method === "fetch") return fetchFor(args[0]);
      throw new CapError("capability_removed", `net.${method} is not part of this runtime`);
    },
    dispose() { disposed = true; },
  };
};
