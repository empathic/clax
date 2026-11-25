// A fake GitHub for the browser tests of GitHub sign-in on a local Clax
// (e2e/local-github-queues.spec.ts), as crates/clax-testkit/src/github.rs is
// for the Rust ones: an OAuth App's authorize page, which signs in as the
// account `signInAs` chose and redirects back with `code` and `state`; the
// token exchange, which turns `code-<id>` into `tok-<id>` for the app's
// client ID and secret; and the API's `GET /user`.
import { createServer, type IncomingMessage } from "node:http";

export interface FakeGitHub {
  /** `http://127.0.0.1:<port>`: the OAuth base and the API base. */
  base: string;
  account(id: number, login: string, name?: string): void;
  signInAs(id: number): void;
  stop(): Promise<void>;
}

function body(req: IncomingMessage): Promise<string> {
  return new Promise(resolve => {
    let b = "";
    req.on("data", (c: Buffer) => { b += c.toString(); });
    req.on("end", () => resolve(b));
  });
}

export async function startFakeGitHub(clientId: string, clientSecret: string): Promise<FakeGitHub> {
  const users = new Map<number, Record<string, unknown>>();
  let next: number | null = null;
  const server = createServer(async (req, res) => {
    const url = new URL(req.url ?? "/", "http://127.0.0.1");
    const json = (status: number, v: unknown) => { res.writeHead(status, { "content-type": "application/json" }); res.end(JSON.stringify(v)); };
    if (req.method === "GET" && url.pathname === "/login/oauth/authorize") {
      const to = url.searchParams.get("redirect_uri");
      const state = url.searchParams.get("state");
      if (url.searchParams.get("client_id") !== clientId || !to || !state || next === null) { res.writeHead(400); res.end("bad authorize"); return; }
      const back = new URL(to);
      back.searchParams.set("code", `code-${next}`);
      back.searchParams.set("state", state);
      res.writeHead(302, { location: back.toString() });
      res.end();
      return;
    }
    if (req.method === "POST" && url.pathname === "/login/oauth/access_token") {
      const form = new URLSearchParams(await body(req));
      if (form.get("client_id") !== clientId || form.get("client_secret") !== clientSecret) { json(200, { error: "incorrect_client_credentials" }); return; }
      const id = form.get("code")?.match(/^code-(\d+)$/)?.[1];
      json(200, id ? { access_token: `tok-${id}`, token_type: "bearer" } : { error: "bad_verification_code" });
      return;
    }
    if (req.method === "GET" && url.pathname === "/user") {
      const id = Number(req.headers.authorization?.match(/^Bearer tok-(\d+)$/)?.[1]);
      const u = users.get(id);
      if (u) json(200, u); else json(401, { message: "Bad credentials" });
      return;
    }
    res.writeHead(404);
    res.end();
  });
  await new Promise<void>(resolve => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as { port: number }).port;
  return {
    base: `http://127.0.0.1:${port}`,
    account: (id, login, name) => { users.set(id, { id, login, name: name ?? null, avatar_url: null }); },
    signInAs: id => { next = id; },
    stop: () => new Promise(resolve => server.close(() => resolve())),
  };
}
