// Clients of a Clax in hosted mode (crates/clax-server/src/hosted.rs) for
// the demo scripts and the end-to-end test: a person's browser session over
// the JSON API, and an agent's session over the MCP endpoint with an agent
// token (crates/clax-server/src/routes/mcp_http.rs). Both use only what a
// browser and an MCP client use, so they work against the native daemon and
// the Worker alike.

/** An error the API or a tool answered: its code, message and details. */
export class ClaxApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly details: Record<string, unknown>;
  constructor(status: number, code: string, message: string, details: Record<string, unknown> = {}) {
    super(`${code}: ${message}`);
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

/** `base` without a trailing slash. */
export function trimBase(base: string): string {
  return base.replace(/\/+$/, "");
}

// ── People (the JSON API with a session cookie) ─────────────────────────────

/** A person as the sign-in routes answer them. */
export interface PersonInfo {
  public_id: string;
  display_name: string;
  owner?: boolean;
}

/** A person signed in through the API: their session cookie, sent as a
 * browser sends it, with the app's `Origin` on every change. The owner's
 * API requests also carry the shell token their browser holds (`GET
 * /api/token`), as the owner's shell sends it; the hosted routes
 * (`/api/hosted/…`) carry none. */
export class PersonSession {
  readonly base: string;
  readonly person: PersonInfo;
  /** `clax_session=<value>`, as a `Cookie` header carries it. */
  readonly cookie: string;
  shellToken: string | null = null;

  constructor(base: string, person: PersonInfo, cookie: string) {
    this.base = trimBase(base);
    this.person = person;
    this.cookie = cookie;
  }

  /** The cookie's name and value, for a browser context. */
  cookiePair(): { name: string; value: string } {
    const at = this.cookie.indexOf("=");
    return { name: this.cookie.slice(0, at), value: this.cookie.slice(at + 1) };
  }

  /** A JSON request as this person; answers the parsed body, throws ClaxApiError on an error answer. */
  async call<T>(method: string, path: string, body?: unknown): Promise<T> {
    const res = await this.fetch(method, path, body === undefined ? undefined : JSON.stringify(body), body === undefined ? {} : { "content-type": "application/json" });
    return (await parse(res, `${method} ${path}`)) as T;
  }

  /** A multipart request as this person (a thread, as the shell posts it). */
  async form<T>(path: string, fields: Record<string, string>): Promise<T> {
    const form = new FormData();
    for (const [k, v] of Object.entries(fields)) form.set(k, v);
    return (await parse(await this.fetch("POST", path, form, {}), `POST ${path}`)) as T;
  }

  private fetch(method: string, path: string, body: BodyInit | undefined, extra: Record<string, string>): Promise<Response> {
    const headers: Record<string, string> = { accept: "application/json", cookie: this.cookie, ...extra };
    if (method !== "GET" && method !== "HEAD") headers.origin = new URL(this.base).origin;
    // The hosted routes are the browser's, which refuse any `Authorization`.
    if (this.shellToken !== null && !path.startsWith("/api/hosted/")) headers.authorization = `Bearer ${this.shellToken}`;
    return fetch(`${this.base}${path}`, { method, headers, body });
  }
}

async function parse(res: Response, what: string): Promise<unknown> {
  if (res.status === 204) return undefined;
  const text = await res.text();
  let v: unknown;
  try {
    v = text === "" ? undefined : JSON.parse(text);
  } catch {
    throw new ClaxApiError(res.status, "bad_response", `${what} answered ${res.status}: ${text.slice(0, 200)}`);
  }
  if (!res.ok) {
    const e = (v as { error?: Record<string, unknown> } | undefined)?.error ?? {};
    const { code, message, ...details } = e;
    throw new ClaxApiError(res.status, typeof code === "string" ? code : "bad_response", typeof message === "string" ? message : `${what} answered ${res.status}`, details);
  }
  return v;
}

/** Signs in through a route that sets the session cookie (setup, an invite, a sign-in link). */
async function signIn(base: string, path: string, body: unknown): Promise<PersonSession> {
  const res = await fetch(`${trimBase(base)}${path}`, {
    method: "POST",
    headers: { accept: "application/json", "content-type": "application/json", origin: new URL(trimBase(base)).origin },
    body: JSON.stringify(body),
  });
  const v = (await parse(res, `POST ${path}`)) as { person: PersonInfo };
  const set = res.headers.getSetCookie().find((c) => /^(__Host-)?clax_session=/.test(c));
  if (set === undefined) throw new ClaxApiError(res.status, "bad_response", `POST ${path} set no session cookie`);
  return new PersonSession(base, v.person, set.split(";")[0]!);
}

/** Makes the instance's first person, the owner (`/api/hosted/setup`), with the shell token their browser holds. */
export async function setupFirstPerson(base: string, secret: string, name: string): Promise<PersonSession> {
  const owner = await signIn(base, "/api/hosted/setup", { secret, name });
  owner.shellToken = (await owner.call<{ token: string }>("GET", "/api/token")).token;
  return owner;
}

/** The code at the end of an invite or sign-in URL. */
export function linkCode(url: string): string {
  return new URL(url).pathname.split("/").pop()!;
}

/** Accepts an invite link: the invited person, signed in. */
export function acceptInvite(base: string, inviteUrl: string): Promise<PersonSession> {
  return signIn(base, `/api/hosted/invites/${linkCode(inviteUrl)}/accept`, {});
}

/** An invite the owner makes for `name`, with `rights` (an access array) if given: its URL and expiry. */
export async function invite(owner: PersonSession, name: string, rights?: unknown): Promise<{ url: string; expires_at: string }> {
  const r = await owner.call<{ invite: { url: string; expires_at: string } }>("POST", "/api/people/invites", { name, ...(rights === undefined ? {} : { rights }) });
  return r.invite;
}

/** A new agent token of `who`'s (`POST /api/hosted/agent-tokens`). */
export interface NewAgentToken {
  id: string;
  token: string;
  mcp_url: string;
  claude_command: string;
}

export function mintAgentToken(who: PersonSession, label: string): Promise<NewAgentToken> {
  return who.call<NewAgentToken>("POST", "/api/hosted/agent-tokens", { label });
}

/** A one-use sign-in link for `who`'s other browser, good for ten minutes. */
export async function signinLink(who: PersonSession): Promise<string> {
  return (await who.call<{ url: string }>("POST", "/api/hosted/signin-links")).url;
}

// ── Agents (MCP over Streamable HTTP, JSON responses) ───────────────────────

/** A tool's answer: its text for the model, the JSON it carries, and whether it is an error. */
export interface ToolAnswer {
  text: string;
  data: Record<string, unknown>;
  isError: boolean;
}

/** The error a tool result carries (`{code, message, …}`), or null. */
export function toolError(a: ToolAnswer): { code: string; message: string; [k: string]: unknown } | null {
  if (!a.isError) return null;
  return (a.data.error ?? { code: "unknown", message: a.text }) as { code: string; message: string };
}

const PROTOCOL = "2025-06-18";

/** The MCP endpoint's path: `/mcp/json` on the native daemon, `/mcp` on the Worker. */
export const NATIVE_MCP_PATH = "/mcp/json";

/** One agent's MCP session under an agent token: `initialize` registers a
 * harness session named after the client (`clientInfo.name`), whose ID the
 * endpoint answers as `Mcp-Session-Id`. */
export class McpAgent {
  readonly url: string;
  readonly token: string;
  /** The MCP client's name, which the agent is named after. */
  readonly name: string;
  session = "";
  private next = 1;

  constructor(base: string, mcpPath: string, token: string, name: string) {
    this.url = `${trimBase(base)}${mcpPath}`;
    this.token = token;
    this.name = name;
  }

  private async post(message: Record<string, unknown>, signal?: AbortSignal): Promise<{ status: number; headers: Headers; body: Record<string, unknown> | null; text: string }> {
    const headers: Record<string, string> = {
      "content-type": "application/json",
      accept: "application/json, text/event-stream",
      authorization: `Bearer ${this.token}`,
    };
    if (this.session !== "") {
      headers["mcp-session-id"] = this.session;
      headers["mcp-protocol-version"] = PROTOCOL;
    }
    const res = await fetch(this.url, { method: "POST", headers, body: JSON.stringify(message), signal });
    const text = await res.text();
    let body: Record<string, unknown> | null = null;
    if (text !== "") {
      try {
        body = JSON.parse(text) as Record<string, unknown>;
      } catch {
        body = null;
      }
    }
    return { status: res.status, headers: res.headers, body, text };
  }

  /** Initializes the session. */
  async connect(): Promise<this> {
    const init = await this.post({
      jsonrpc: "2.0",
      id: this.next++,
      method: "initialize",
      params: { protocolVersion: PROTOCOL, capabilities: {}, clientInfo: { name: this.name, version: "1.0.0" } },
    });
    if (init.status !== 200) throw new ClaxApiError(init.status, init.status === 401 ? "unauthorized" : "mcp_error", `initialize answered ${init.status}: ${init.text.slice(0, 200)}`);
    this.session = init.headers.get("mcp-session-id") ?? "";
    if (this.session === "") throw new ClaxApiError(init.status, "bad_response", "initialize answered no Mcp-Session-Id: is this token an agent token?");
    await this.post({ jsonrpc: "2.0", method: "notifications/initialized" });
    return this;
  }

  /** Calls a tool; a refusal is an answer with `isError`, not a throw. `signal` abandons the call (an abort throws). */
  async call(tool: string, args: Record<string, unknown> = {}, signal?: AbortSignal): Promise<ToolAnswer> {
    const r = await this.post({ jsonrpc: "2.0", id: this.next++, method: "tools/call", params: { name: tool, arguments: args } }, signal);
    const error = r.body?.error as { code: number; message: string } | undefined;
    if (r.status !== 200 || error !== undefined || r.body === null) {
      throw new ClaxApiError(r.status, "mcp_error", `${tool}: ${error?.message ?? r.text.slice(0, 200)}`);
    }
    const result = r.body.result as { content?: { type: string; text?: string }[]; structuredContent?: Record<string, unknown>; isError?: boolean };
    const text = (result.content ?? []).filter((c) => c.type === "text").map((c) => c.text ?? "").join("\n");
    let data = result.structuredContent;
    if (data === undefined) {
      try {
        data = JSON.parse(result.content?.[0]?.text ?? "{}") as Record<string, unknown>;
      } catch {
        data = {};
      }
    }
    return { text, data, isError: result.isError === true };
  }

  /** Calls a tool that must succeed; throws ClaxApiError with the tool's code otherwise. */
  async ok(tool: string, args: Record<string, unknown> = {}, signal?: AbortSignal): Promise<ToolAnswer> {
    const a = await this.call(tool, args, signal);
    const e = toolError(a);
    if (e !== null) {
      const { code, message, ...details } = e;
      throw new ClaxApiError(0, code, `${tool}: ${message}`, details);
    }
    return a;
  }

  /** Ends the session (`DELETE` on the endpoint). */
  async close(): Promise<void> {
    if (this.session === "") return;
    await fetch(this.url, { method: "DELETE", headers: { authorization: `Bearer ${this.token}`, "mcp-session-id": this.session } }).catch(() => undefined);
    this.session = "";
  }
}
