// `npx @empathic/clax-client-sdk setup` (cli.ts): an app's report queue
// asked of a person on a hosted Clax and its service grant written to the
// app's env file, with nothing but the instance's public routes
// (docs/contract.md, "The public side's device sign-in" and "Queue
// requests"; docs/setup-feedback.md is the same as curl calls). Every
// effect is injected (`SetupDeps`), so the tests drive it with a fake
// fetch, clock and env file.
import { checkInstance } from "./client";
import { base64url } from "./sign";

/** The variables written to the env file, in order: the queue's ID, the
 * service grant's token and its private key (the names `queue_request`'s
 * `write_env` and docs/feedback.md's recipes use). The private key is made
 * here and never sent: the take sends its public half. */
export const ENV_NAMES = ["CLAX_QUEUE", "CLAX_QUEUE_TOKEN", "CLAX_QUEUE_KEY"] as const;

/** The longest wait one read of a queue request asks for (the server's
 * cap). */
const MAX_WAIT_S = 60;

/** What `setup` asks for. */
export interface SetupOptions {
  /** The instance's origin, such as `https://clax.dev`. */
  instance: string;
  /** The queue's name, as the person sees it. */
  name: string;
  /** The origins the queue takes reports from (exact, or with one `*` label). */
  origins: string[];
  reportersSee?: "own" | "all";
  /** Why the queue is asked for, shown to the person. */
  note?: string;
  /** The env file to write (default `.env`). */
  envFile: string;
  /** What the approval page calls this machine. */
  clientName: string;
  /** How long to wait for the person's answer to the queue request, in
   * seconds (default an hour, the request's own life). */
  timeoutS?: number;
}

/** The env file `setup` writes. */
export interface EnvFile {
  /** Its text, or `null` when there is no such file. */
  read(path: string): string | null;
  /** Writes `text` to it; `created` says the file did not exist, so it is
   * made readable by its owner alone. */
  write(path: string, text: string, created: boolean): void;
}

/** Everything `setup` does to the world. */
export interface SetupDeps {
  fetch: (input: string, init: RequestInit) => Promise<Response>;
  /** Milliseconds since the Unix epoch. */
  now: () => number;
  sleep: (ms: number) => Promise<void>;
  /** One line for the person (stdout). Never handed a secret. */
  print: (line: string) => void;
  env: EnvFile;
}

/** Why `setup` stopped: `code` is the instance's error code, or one of
 * `no_device_sign_in`, `sign_in_denied`, `sign_in_expired`, `denied`,
 * `cancelled`, `expired`, `timed_out`, `grant_taken`, `bad_response`. */
export class SetupError extends Error {
  constructor(readonly code: string, message: string) {
    super(message);
    this.name = "SetupError";
  }
}

/** What `setup` did: the queue's ID, the env file, and the instance's MCP
 * endpoint for a fixer. No secret. */
export interface SetupResult {
  queue: string;
  envFile: string;
  mcpUrl?: string;
}

type Json = Record<string, any>;

/** A new Ed25519 pair for the service grant: its private key as PKCS#8 in
 * unpadded base64url (what `ServiceClient`'s `privateKey` reads) and its
 * public key's 32 bytes in unpadded base64url (what the take's
 * `public_key` takes, `clax_core::sig::encode_public`). */
export async function grantKeyPair(): Promise<{ privateKey: string; publicKey: string }> {
  const pair = await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"]) as CryptoKeyPair;
  const pkcs8 = new Uint8Array(await crypto.subtle.exportKey("pkcs8", pair.privateKey));
  const raw = new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey));
  return { privateKey: base64url(pkcs8), publicKey: base64url(raw) };
}

/** One JSON call: `[status, body]`; a body that is not JSON is
 * `bad_response`, and no answer at all is `network`. */
async function call(deps: SetupDeps, url: string, init: RequestInit): Promise<[number, Json]> {
  let r: Response;
  try {
    r = await deps.fetch(url, init);
  } catch (e) {
    throw new SetupError("network", `${url}: ${e instanceof Error ? e.message : String(e)}`);
  }
  const text = await r.text();
  try {
    return [r.status, JSON.parse(text) as Json];
  } catch {
    throw new SetupError("bad_response", `${url} answered ${r.status} with no JSON`);
  }
}

const errorCode = (v: Json): string => (typeof v?.error?.code === "string" ? v.error.code : "");
const failed = (what: string, status: number, v: Json) =>
  new SetupError(errorCode(v) || "bad_response", `${what}: ${status} ${errorCode(v)} ${v?.error?.message ?? ""}`.trim());

const post = (body: unknown, token?: string): RequestInit => ({
  method: "POST",
  headers: { "content-type": "application/json", ...(token ? { authorization: `Bearer ${token}` } : {}) },
  body: JSON.stringify(body),
});

/** `text` with each `name=value` line of `vars` replaced in place (an
 * `export ` prefix kept), and those it lacks appended, each once: a second
 * line of a name already replaced is dropped. */
export function withVars(text: string | null, vars: [string, string][]): string {
  const lines = text ? text.split("\n") : [];
  if (lines.length && lines[lines.length - 1] === "") lines.pop();
  const seen = new Set<string>();
  const out: string[] = [];
  for (const line of lines) {
    const m = /^(\s*(?:export\s+)?)([A-Za-z_][A-Za-z0-9_]*)\s*=/.exec(line);
    const v = m && vars.find(([k]) => k === m[2]);
    if (!m || !v) {
      out.push(line);
      continue;
    }
    if (seen.has(v[0])) continue;
    seen.add(v[0]);
    out.push(`${m[1]}${v[0]}=${v[1]}`);
  }
  for (const [k, value] of vars) if (!seen.has(k)) out.push(`${k}=${value}`);
  return `${out.join("\n")}\n`;
}

/** Signs in with the scoped device flow, asks the person for the queue,
 * waits for their answer, and writes the queue's ID and service grant to
 * `opts.envFile`. Throws `SetupError` when the person denies a step, a
 * step expires, or the instance refuses; nothing is written then. */
export async function setup(opts: SetupOptions, deps: SetupDeps): Promise<SetupResult> {
  checkInstance(opts.instance, "setup");
  const origin = opts.instance.replace(/\/$/, "");
  if (!opts.name) throw new SetupError("invalid_args", "--name is required");
  if (!opts.origins.length) throw new SetupError("invalid_args", "at least one --origin is required");

  // What the instance is, and the paths of its device sign-in.
  const [ds, d] = await call(deps, `${origin}/.well-known/clax`, { method: "GET" });
  if (ds !== 200 || !["hosted", "local"].includes(d.kind)) throw new SetupError("bad_response", `${origin} does not describe itself as a Clax (GET /.well-known/clax answered ${ds})`);
  if (!d.device?.code || !d.device?.token) {
    throw new SetupError("no_device_sign_in", `${origin} is a ${d.kind} Clax with no device sign-in: run \`clax queue create "${opts.name}" --origin … --write-env <path>\` on its machine instead`);
  }

  // The sign-in, scoped to asking for one queue.
  const [cs, code] = await call(deps, `${origin}${d.device.code}`, post({ client_name: opts.clientName, scope: ["queue.request"] }));
  if (cs !== 200) throw failed("starting the sign-in", cs, code);
  deps.print(`Approve this sign-in in your browser: ${code.verification_uri_complete} (code ${code.user_code})`);
  let interval = Math.max(1, Number(code.interval) || 5) * 1000;
  const signInEnds = deps.now() + (Number(code.expires_in) || 600) * 1000;
  let signedIn: Json | undefined;
  while (!signedIn) {
    if (deps.now() >= signInEnds) throw new SetupError("sign_in_expired", "the sign-in was not approved in time: run setup again");
    await deps.sleep(interval);
    const [ts, t] = await call(deps, `${origin}${d.device.token}`, post({ device_code: code.device_code }));
    if (ts === 200 && typeof t.token === "string") {
      signedIn = t;
      break;
    }
    switch (errorCode(t)) {
      case "authorization_pending":
        continue;
      case "slow_down":
        interval += 5000;
        continue;
      case "access_denied":
        throw new SetupError("sign_in_denied", "the sign-in was denied");
      case "expired_token":
        throw new SetupError("sign_in_expired", "the sign-in expired before it was approved: run setup again");
      default:
        throw failed("waiting for the sign-in", ts, t);
    }
  }
  const token = signedIn.token as string;
  const person = signedIn.person?.display_name;
  deps.print(`Signed in${person ? ` as ${person}'s agent` : ""}, to ask for one queue.`);

  // The queue request.
  const body: Json = { name: opts.name, origins: opts.origins };
  if (opts.reportersSee) body.reporters_see = opts.reportersSee;
  if (opts.note) body.note = opts.note;
  const [rs, asked] = await call(deps, `${origin}/api/queue-requests`, post(body, token));
  if (rs !== 201) throw failed("asking for the queue", rs, asked);
  const id = String(asked.request?.id ?? "");
  deps.print(`Asked for the queue "${opts.name}": approve it in Clax at ${origin}/inbox?queue_request=${encodeURIComponent(id)}`);

  // Its answer, in reads of up to a minute each, until the time is up.
  const ends = deps.now() + (opts.timeoutS ?? 3600) * 1000;
  const auth = { headers: { authorization: `Bearer ${token}` } };
  let request: Json = asked.request;
  while (request?.status === "pending") {
    const left = Math.ceil((ends - deps.now()) / 1000);
    if (left <= 0) throw new SetupError("timed_out", `no answer to the queue request in time; it waits in ${origin}/inbox until it expires`);
    const wait = Math.min(left, MAX_WAIT_S);
    const started = deps.now();
    const [gs, got] = await call(deps, `${origin}/api/queue-requests/${encodeURIComponent(id)}?wait=${wait}`, { method: "GET", ...auth });
    if (gs !== 200) throw failed("waiting for the answer", gs, got);
    request = got.request;
    // A read that came back pending at once (a proxy that does not hold
    // it) waits a second before the next.
    if (request?.status === "pending" && deps.now() - started < 1000) await deps.sleep(1000);
  }
  switch (request?.status) {
    case "approved":
      break;
    case "denied":
      throw new SetupError("denied", `the queue request was denied`);
    case "cancelled":
      throw new SetupError("cancelled", "the queue request was cancelled (its sign-in was revoked)");
    case "expired":
      throw new SetupError("expired", "the queue request expired before it was answered: run setup again");
    default:
      throw new SetupError("bad_response", `the queue request is ${request?.status}`);
  }

  // The grant, taken once with the public half of a key made here, and
  // its token and this key's private half straight into the env file. An
  // instance that made the pair itself (one older than the take's
  // `public_key`) hands over its private key instead, and that is written.
  const key = await grantKeyPair();
  const [ks, taken] = await call(deps, `${origin}/api/queue-requests/${encodeURIComponent(id)}/take`, post({ public_key: key.publicKey }, token));
  if (ks !== 200) throw failed("taking the grant", ks, taken);
  const queue = String(taken.request?.queue_id ?? request.queue_id ?? "");
  const privateKey = typeof taken.private_key === "string" ? taken.private_key : key.privateKey;
  if (typeof taken.token !== "string" || !queue) {
    if (taken.grant_taken) throw new SetupError("grant_taken", "an earlier take took this queue's service grant: make another on the Queues page");
    throw new SetupError("bad_response", "the take carried no grant");
  }
  const before = deps.env.read(opts.envFile);
  deps.env.write(opts.envFile, withVars(before, [[ENV_NAMES[0], queue], [ENV_NAMES[1], taken.token], [ENV_NAMES[2], privateKey]]), before === null);
  deps.print(`Wrote ${ENV_NAMES.join(", ")} to ${opts.envFile} (queue ${queue}). Keep that file out of git and put the same values in your deployment's secrets.`);
  const mcpUrl = typeof signedIn.mcp_url === "string" ? signedIn.mcp_url : undefined;
  if (mcpUrl) deps.print(`A fixing agent reaches the queue at ${mcpUrl} with an agent token of its own.`);
  return { queue, envFile: opts.envFile, mcpUrl };
}
