// A framed page's calls to its own site's API, relayed for it in the Clax
// app. The bridge calls its site's `/_clax/api/…` itself (web/bridge/src/caps/
// origin-client.ts), but no WebView of the app hands its scheme a request's
// body (Android's `shouldInterceptRequest` has none), so in the app a write
// cannot go by the scheme: the bridge sends it here instead (`site.fetch`),
// and the shell's transport takes it to the app (transport.ts, a `call`
// naming the site), which asks the site as the page's own frame would: with
// the site's session and origin, which the app's core keeps for that site,
// never the owner's.
//
// It goes only to the site of the artifact this view frames (`CapEnv.aid`,
// whose frame alone the shell takes messages from), only under
// `/_clax/api/`, with only the request's `Content-Type` and `Accept`: the
// page names no other site, and the shell adds nothing of its own.
import { hostEmbed } from "../transport";
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

/** The site call a page asks for. */
export type SiteFetch = { method: string; path: string; headers: [string, string][]; body: ArrayBuffer | null };
/** Its answer. */
export type SiteAnswer = { status: number; headers: [string, string][]; body: ArrayBuffer };

const METHODS = new Set(["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"]);
const KEPT = new Set(["content-type", "accept"]);
/** The site API's paths: no segment that could leave it. */
const API_PATH = /^\/_clax\/api\/[^?#]*(\?[^#]*)?$/;

/** `v` as a site call this view may relay, else `invalid_argument`. */
function siteFetch(v: unknown): SiteFetch {
  const r = (v ?? {}) as Partial<SiteFetch>;
  const method = typeof r.method === "string" ? r.method.toUpperCase() : "";
  const path = typeof r.path === "string" ? r.path : "";
  const segs = path.split(/[?#]/)[0].split("/");
  if (!METHODS.has(method)) throw new CapError("invalid_argument", `${String(r.method)} is not a method of the site's API`);
  if (!API_PATH.test(path) || segs.some(s => s === "." || s === "..") || path.split("?")[0] === "/_clax/api/room") {
    throw new CapError("invalid_argument", `'${path}' is not a path of the site's API`);
  }
  const headers = (Array.isArray(r.headers) ? r.headers : [])
    .filter((h): h is [string, string] => Array.isArray(h) && typeof h[0] === "string" && typeof h[1] === "string" && KEPT.has(h[0].toLowerCase()));
  const body = r.body instanceof ArrayBuffer && r.body.byteLength > 0 ? r.body : null;
  return { method, path, headers, body };
}

export const siteHandler: HandlerFactory = env => ({
  async call(method, args) {
    if (method !== "fetch") throw new CapError("capability_removed", `site.${String(method)} is not part of this runtime`);
    const embed = hostEmbed();
    if (!embed) throw new CapError("capability_removed", "only the Clax app relays a page's site calls");
    const r = siteFetch(args[0]);
    const url = embed.content.replace("{id}", env.aid) + r.path;
    let res: Response;
    try {
      res = await fetch(url, { method: r.method, headers: r.headers, body: r.method === "GET" || r.method === "HEAD" ? null : r.body });
    } catch (e) {
      throw new CapError("unavailable", `the site could not be reached: ${e instanceof Error ? e.message : String(e)}`);
    }
    return { status: res.status, headers: [...res.headers], body: await res.arrayBuffer() } satisfies SiteAnswer;
  },
});
