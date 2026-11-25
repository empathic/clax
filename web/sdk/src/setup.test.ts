import { describe, expect, it } from "vitest";
import { ENV_NAMES, setup, SetupError, withVars, type SetupDeps, type SetupOptions } from "./setup";

const ORIGIN = "https://clax.example";
const GRANT = "cxq_secret-token";
const KEY = "MFMCAQEwBQYDK2VwBCIEIPRIVATE";

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status });
const pending = { error: { code: "authorization_pending", message: "waiting" } };

/** One answer the fake instance gives, by `METHOD path`. */
type Answer = (body: any) => Response;

/** A hosted instance that answers `script`'s routes in turn (the last
 * answer of a route repeats), on a clock that only the fake's sleeps and
 * reads move; and an env file in memory. */
function world(script: Record<string, Answer[]>, files: Record<string, string> = {}) {
  let now = 1_760_000_000_000;
  const calls: { route: string; init: RequestInit }[] = [];
  const printed: string[] = [];
  const modes: Record<string, "new" | "kept"> = {};
  const deps: SetupDeps = {
    now: () => now,
    sleep: async ms => { now += ms; },
    print: line => printed.push(line),
    fetch: async (url, init) => {
      const u = new URL(url);
      const route = `${init.method ?? "GET"} ${u.pathname}`;
      calls.push({ route: `${route}${u.search}`, init });
      const answers = script[route];
      if (!answers) return json(404, { error: { code: "not_found", message: route } });
      const answer = answers.length > 1 ? answers.shift()! : answers[0];
      // A read that waits (`?wait=`) holds for its wait when it stays pending.
      const wait = Number(u.searchParams.get("wait") ?? 0);
      const r = answer(init.body ? JSON.parse(String(init.body)) : undefined);
      const text = await r.clone().text();
      if (wait && text.includes('"pending"')) now += wait * 1000;
      return r;
    },
    env: {
      read: path => files[path] ?? null,
      write: (path, text, created) => { files[path] = text; modes[path] = created ? "new" : "kept"; },
    },
  };
  return { deps, calls, printed, files, modes, advance: (ms: number) => { now += ms; } };
}

const discovery: Answer = () => json(200, {
  kind: "hosted", contract_version: "1", api: "/api", origin: ORIGIN,
  device: { code: "/api/hosted/device/code", approve: "/device", token: "/api/hosted/device/token" },
});
const code: Answer = () => json(200, {
  device_code: "dev-123", user_code: "ABCD-EFGH", verification_uri: `${ORIGIN}/device`,
  verification_uri_complete: `${ORIGIN}/device?code=ABCD-EFGH`, expires_in: 600, interval: 5,
});
const signedIn: Answer = () => json(200, {
  token: "cxa_scoped", token_type: "Bearer", grant_id: "g1", person: { public_id: "p1", display_name: "Sam" },
  origin: ORIGIN, mcp_url: `${ORIGIN}/mcp`, scope: ["queue.request"],
});
const request = (status: string, extra: object = {}) => ({ id: "01JREQ", status, name: "Invoices", origins: ["https://app.example.com"], ...extra });
const asked: Answer = () => json(201, { request: request("pending") });
const taken: Answer = () => json(200, { request: request("approved", { queue_id: "q-77" }), grant: { id: "sg1" }, token: GRANT, private_key: KEY });

const OPTS: SetupOptions = { instance: ORIGIN, name: "Invoices", origins: ["https://app.example.com", "https://app-pr-*.fly.dev"], envFile: ".env", clientName: "setup test", note: "for the app" };

/** The routes of a sign-in approved on the second poll and a queue
 * request answered `status` on its second read. */
function approvedScript(status = "approved"): Record<string, Answer[]> {
  return {
    "GET /.well-known/clax": [discovery],
    "POST /api/hosted/device/code": [code],
    "POST /api/hosted/device/token": [() => json(400, pending), signedIn],
    "POST /api/queue-requests": [asked],
    "GET /api/queue-requests/01JREQ": [() => json(200, { request: request("pending") }), () => json(200, { request: request(status, { queue_id: "q-77" }) })],
    "POST /api/queue-requests/01JREQ/take": [taken],
  };
}

describe("setup", () => {
  it("signs in for one queue request, waits for the approval and writes the grant to a new env file", async () => {
    const w = world(approvedScript());
    const result = await setup(OPTS, w.deps);
    expect(result).toEqual({ queue: "q-77", envFile: ".env", mcpUrl: `${ORIGIN}/mcp` });
    expect(w.files[".env"]).toBe(`CLAX_QUEUE=q-77\nCLAX_QUEUE_TOKEN=${GRANT}\nCLAX_QUEUE_KEY=${KEY}\n`);
    expect(w.modes[".env"]).toBe("new");

    // The scoped sign-in, polled no sooner than its interval.
    const routes = w.calls.map(c => c.route);
    expect(routes).toEqual([
      "GET /.well-known/clax",
      "POST /api/hosted/device/code",
      "POST /api/hosted/device/token",
      "POST /api/hosted/device/token",
      "POST /api/queue-requests",
      "GET /api/queue-requests/01JREQ?wait=60",
      "GET /api/queue-requests/01JREQ?wait=60",
      "POST /api/queue-requests/01JREQ/take",
    ]);
    expect(JSON.parse(String(w.calls[1].init.body))).toEqual({ client_name: "setup test", scope: ["queue.request"] });
    expect(JSON.parse(String(w.calls[4].init.body))).toEqual({ name: "Invoices", origins: OPTS.origins, note: "for the app" });
    for (const c of w.calls.slice(4)) expect((c.init.headers as Record<string, string>).authorization).toBe("Bearer cxa_scoped");

    // The person is shown where to approve each step, and never a secret.
    const out = w.printed.join("\n");
    expect(out).toContain(`${ORIGIN}/device?code=ABCD-EFGH`);
    expect(out).toContain(`${ORIGIN}/inbox?queue_request=01JREQ`);
    expect(out).toContain(`Wrote ${ENV_NAMES.join(", ")} to .env`);
    for (const secret of [GRANT, KEY, "cxa_scoped", "dev-123"]) expect(out).not.toContain(secret);
  });

  it("replaces the variables an existing env file has, keeping its other lines and its mode", async () => {
    const w = world(approvedScript(), { "app/.env": "PORT=3000\nexport CLAX_QUEUE=old\nCLAX_QUEUE_TOKEN=cxq_old\nCLAX_QUEUE_TOKEN=cxq_older\n# note" });
    await setup({ ...OPTS, envFile: "app/.env" }, w.deps);
    expect(w.files["app/.env"]).toBe(`PORT=3000\nexport CLAX_QUEUE=q-77\nCLAX_QUEUE_TOKEN=${GRANT}\n# note\nCLAX_QUEUE_KEY=${KEY}\n`);
    expect(w.modes["app/.env"]).toBe("kept");
  });

  it("stops on a denied queue request, writing nothing and taking nothing", async () => {
    const w = world(approvedScript("denied"));
    await expect(setup(OPTS, w.deps)).rejects.toMatchObject({ code: "denied" });
    expect(w.files).toEqual({});
    expect(w.calls.some(c => c.route.endsWith("/take"))).toBe(false);
  });

  it("stops on a denied sign-in", async () => {
    const w = world({ ...approvedScript(), "POST /api/hosted/device/token": [() => json(400, { error: { code: "access_denied", message: "denied" } })] });
    await expect(setup(OPTS, w.deps)).rejects.toMatchObject({ code: "sign_in_denied" });
    expect(w.calls.some(c => c.route.startsWith("POST /api/queue-requests"))).toBe(false);
  });

  it("stops when the sign-in expires unapproved, after polling no longer than its code lives", async () => {
    const w = world({ ...approvedScript(), "POST /api/hosted/device/token": [() => json(400, pending)] });
    await expect(setup(OPTS, w.deps)).rejects.toMatchObject({ code: "sign_in_expired" });
    const polls = w.calls.filter(c => c.route === "POST /api/hosted/device/token").length;
    expect(polls).toBe(600 / 5);
  });

  it("stops when the queue request expires, or when its wait runs out", async () => {
    const expired = world(approvedScript("expired"));
    await expect(setup(OPTS, expired.deps)).rejects.toMatchObject({ code: "expired" });
    expect(expired.files).toEqual({});

    const waiting = world({ ...approvedScript(), "GET /api/queue-requests/01JREQ": [() => json(200, { request: request("pending") })] });
    await expect(setup({ ...OPTS, timeoutS: 150 }, waiting.deps)).rejects.toMatchObject({ code: "timed_out" });
    const reads = waiting.calls.filter(c => c.route.startsWith("GET /api/queue-requests/")).map(c => c.route.split("?")[1]);
    expect(reads).toEqual(["wait=60", "wait=60", "wait=30"]);
  });

  it("slows its polls when told to", async () => {
    const w = world({ ...approvedScript(), "POST /api/hosted/device/token": [() => json(400, { error: { code: "slow_down", message: "" } }), signedIn] });
    const start = w.deps.now();
    await setup(OPTS, w.deps);
    expect(w.calls.filter(c => c.route === "POST /api/hosted/device/token")).toHaveLength(2);
    // 5 s before the first poll, 10 s before the second, then the reads' wait.
    expect(w.deps.now() - start).toBeGreaterThanOrEqual(15_000);
  });

  it("refuses a Clax with no device sign-in, naming the local command", async () => {
    const w = world({ "GET /.well-known/clax": [() => json(200, { kind: "local", contract_version: "1", api: "/api" })] });
    const e = await setup(OPTS, w.deps).catch(x => x);
    expect(e).toBeInstanceOf(SetupError);
    expect(e.code).toBe("no_device_sign_in");
    expect(e.message).toContain("clax queue create");
  });

  it("says so when an earlier take took the grant", async () => {
    const w = world({ ...approvedScript(), "POST /api/queue-requests/01JREQ/take": [() => json(200, { request: request("approved", { queue_id: "q-77" }), grant_taken: true })] });
    await expect(setup(OPTS, w.deps)).rejects.toMatchObject({ code: "grant_taken" });
    expect(w.files).toEqual({});
  });
});

describe("withVars", () => {
  it("appends to a file without a final line break, and makes a missing one", () => {
    expect(withVars("A=1", [["B", "2"]])).toBe("A=1\nB=2\n");
    expect(withVars(null, [["B", "2"]])).toBe("B=2\n");
    expect(withVars("", [["B", "2"]])).toBe("B=2\n");
  });
});
