// A deployer's site for the report queue tests (e2e/queues.spec.ts): a page
// that loads the overlay from the Clax instance (`<instance>/_clax/overlay.js`)
// and a backend that holds the queue's service grant and mints reporter
// tokens for its own signed-in users through `web/sdk/src/service.ts`, as a
// deployer's server does (spec 2026-10-27-clax-sdk-design §5.3).
//
// The site serves every page with a Content-Security-Policy whose
// `connect-src` allows only the site itself and the instance, and whose
// `script-src` allows no inline script. Its users sign in at
// `/login?user=<id>`, which sets a cookie; `POST /clax-token` mints a token
// for the cookie's user, bound to the key the overlay sends and to the
// site's own origin. Until `grant` hands it the service grant, the backend
// answers 503.
import { readFileSync } from "node:fs";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { ServiceClient } from "../../sdk/src/service";
import { freePort } from "../hosted-helpers";

/** The service grant the queue's admin made, as the Queues page shows it once. */
export interface Grant {
  queue: string;
  token: string;
  privateKey: string;
}

/** A running deployer site. */
export interface DeployerSite {
  /** `http://127.0.0.1:<port>`: another origin than the instance's `localhost`. */
  origin: string;
  /** Hands the backend its service grant. */
  grant(g: Grant): void;
  /** The reporter IDs the backend minted a token for, in order. */
  minted: string[];
  stop(): Promise<void>;
}

const page = readFileSync(new URL("index.html", import.meta.url), "utf8");
const app = readFileSync(new URL("app.js", import.meta.url), "utf8");

/** The page's policy: scripts from the site and the instance only (no
 * inline script), requests to the site and the instance only. The overlay's
 * own stylesheet goes into its shadow root as a `<style>`, and its clip
 * preview is a `blob:` image. */
export function policyFor(instance: string): string {
  return [
    "default-src 'self'",
    `script-src 'self' ${instance}`,
    `connect-src 'self' ${instance}`,
    "style-src 'self' 'unsafe-inline'",
    "img-src 'self' data: blob:",
    "frame-ancestors 'none'",
  ].join("; ");
}

function cookieUser(req: IncomingMessage): string | null {
  for (const part of (req.headers.cookie ?? "").split(";")) {
    const [k, v] = part.trim().split("=");
    if (k === "user" && v) return decodeURIComponent(v);
  }
  return null;
}

async function bodyOf(req: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const c of req) chunks.push(c as Buffer);
  return Buffer.concat(chunks).toString("utf8");
}

function json(res: ServerResponse, status: number, v: unknown): void {
  res.writeHead(status, { "content-type": "application/json", "cache-control": "no-store" });
  res.end(JSON.stringify(v));
}

/** Starts the site for the Clax instance at `instance` (its origin). */
export async function startDeployerSite(instance: string): Promise<DeployerSite> {
  const port = await freePort();
  const origin = `http://127.0.0.1:${port}`;
  const minted: string[] = [];
  let service: { client: ServiceClient; queue: string } | null = null;

  const server = createServer((req, res) => {
    void (async () => {
      const url = new URL(req.url ?? "/", origin);
      if (req.method === "GET" && url.pathname === "/login") {
        const user = url.searchParams.get("user") ?? "";
        if (!/^[a-z0-9-]{1,32}$/.test(user)) return json(res, 400, { error: "user" });
        res.writeHead(302, { location: "/", "set-cookie": `user=${encodeURIComponent(user)}; Path=/; HttpOnly; SameSite=Lax` });
        return res.end();
      }
      if (req.method === "GET" && url.pathname === "/") {
        if (!service) return json(res, 503, { error: "no service grant yet" });
        res.writeHead(200, { "content-type": "text/html; charset=utf-8", "content-security-policy": policyFor(instance), "cache-control": "no-store" });
        return res.end(page.replaceAll("{{INSTANCE}}", instance).replaceAll("{{QUEUE}}", service.queue));
      }
      if (req.method === "GET" && url.pathname === "/app.js") {
        res.writeHead(200, { "content-type": "text/javascript; charset=utf-8", "cache-control": "no-store" });
        return res.end(app);
      }
      if (req.method === "POST" && url.pathname === "/clax-token") {
        const user = cookieUser(req);
        if (!user) return json(res, 401, { error: "sign in first" });
        if (!service) return json(res, 503, { error: "no service grant yet" });
        let publicKey: unknown;
        try { publicKey = (JSON.parse(await bodyOf(req)) as { public_key?: unknown }).public_key; } catch { publicKey = undefined; }
        if (typeof publicKey !== "string" || !publicKey) return json(res, 400, { error: "public_key" });
        try {
          const t = await service.client.mintReporterToken({ reporter_id: user, origin, public_key: publicKey });
          minted.push(user);
          return json(res, 200, t);
        } catch (e) {
          return json(res, 502, { error: e instanceof Error ? e.message : String(e) });
        }
      }
      res.writeHead(404, { "content-type": "text/plain" });
      res.end("not found");
    })().catch(e => {
      if (!res.headersSent) json(res, 500, { error: String(e) });
      else res.end();
    });
  });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", () => resolve());
  });
  return {
    origin,
    minted,
    grant(g) {
      service = { client: new ServiceClient({ instance, queue: g.queue, token: g.token, privateKey: g.privateKey }), queue: g.queue };
    },
    stop: () => new Promise<void>(resolve => {
      server.closeAllConnections();
      server.close(() => resolve());
    }),
  };
}
