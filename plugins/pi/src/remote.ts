// Remote mode (spec 2026-10-08 §16, M26): a machine connected to another
// person's Clax with `clax connect` holds `remote.json` and `remote.key`;
// every request then goes to that daemon, signed with the machine's Ed25519
// key (`clax-ed25519 v2`, spec §6.6), and a request naming the session
// carries its secret (§6.8), which the extension keeps in memory and in the
// session's credential file under `<home>/remote-sessions/`.
import { createHash, createPrivateKey, randomBytes, sign as edSign, type KeyObject } from "node:crypto";
import { closeSync, constants, fstatSync, lstatSync, mkdirSync, openSync, readFileSync, readSync, renameSync, unlinkSync, chmodSync, writeSync, fsyncSync, futimesSync } from "node:fs";
import { isIP } from "node:net";
import { join } from "node:path";

/** The variable that, set to `0`, keeps a connected home local. */
export const REMOTE_ENV = "CLAX_REMOTE";
/** `remote.json`'s version this extension reads. */
export const FILE_VERSION = 1;
/** First line of the canonical string. */
export const VERSION_LINE = "clax-ed25519 v2";
/** The agent-side headers a signature covers, in order (`clax_core::sig::AGENT_HEADERS`). */
export const AGENT_HEADERS = ["x-clax-session", "x-clax-channel", "x-clax-via", "x-clax-git", "x-clax-call"] as const;
/** The header that carries a session's secret. */
export const SESSION_SECRET_HEADER = "x-clax-session-secret";
/** The prefix of a session secret. */
export const SESSION_PREFIX = "cxs_";
/** Seconds the daemon's clock may be learned off by (`clax_core::remote::MAX_OFFSET_S`). */
export const MAX_OFFSET_S = 600;
/** Backoffs, in seconds, for `limit_reached` and `busy` (`clax_core::remote::BACKOFF_S`). */
export const BACKOFF_S = [1, 2, 4];

/** `remote.json`: the daemon this machine is connected to and its grant. */
export interface RemoteConfig {
  /** `http://<IP>:<port>`. */
  url: string;
  /** `<IP>:<port>`, the `Host` every request names and signs. */
  host: string;
  grantId: string;
  accessToken: string;
  personName: string | null;
}

/** A connected machine: its configuration, signing key, and Clax home. */
export interface Remote {
  config: RemoteConfig;
  key: KeyObject;
  home: string;
  /** Seconds the daemon's clock is ahead of this machine's, as learned. */
  offset: number;
  /** Waits out a retry's backoff; `setTimeout` when absent (tests pass
   * their own). */
  sleep?: (ms: number) => Promise<void>;
}

/** Why the connection in `remote.json` cannot be used. */
export class RemoteBroken extends Error {
  constructor(message: string) {
    super(message);
    this.name = "RemoteBroken";
  }
}

/** The remote this home is connected to; `undefined` without
 * `remote.json` or with `CLAX_REMOTE=0`. Throws [`RemoteBroken`] when
 * `remote.json` exists but cannot be used: nothing then falls back to a
 * local daemon. */
export function loadRemote(home: string, env: NodeJS.ProcessEnv = process.env): Remote | undefined {
  if ((env[REMOTE_ENV] ?? "").trim() === "0") return undefined;
  const path = join(home, "remote.json");
  let raw: string;
  try {
    raw = readFileSync(path, "utf8");
  } catch (e) {
    if ((e as NodeJS.ErrnoException).code === "ENOENT") return undefined;
    throw new RemoteBroken(`${path}: ${(e as Error).message}`);
  }
  let s: any;
  try {
    s = JSON.parse(raw);
  } catch (e) {
    throw new RemoteBroken(`${path}: ${(e as Error).message}`);
  }
  if (s?.version !== FILE_VERSION) throw new RemoteBroken(`${path}: version ${s?.version} (this extension reads ${FILE_VERSION})`);
  const host = typeof s.url === "string" ? daemonHost(s.url) : null;
  if (host === null) throw new RemoteBroken(`${path}: the daemon URL is not http://<IP address>:<port>`);
  if (typeof s.access_token !== "string" || typeof s.grant_id !== "string") throw new RemoteBroken(`${path}: no grant`);
  const keyPath = join(home, "remote.key");
  let key: KeyObject;
  try {
    key = createPrivateKey({ key: readFileSync(keyPath), format: "der", type: "pkcs8" });
  } catch (e) {
    throw new RemoteBroken(`${keyPath}: ${(e as Error).message}`);
  }
  if (key.asymmetricKeyType !== "ed25519") throw new RemoteBroken(`${keyPath} is not an Ed25519 key`);
  return {
    config: { url: `http://${host}`, host, grantId: s.grant_id, accessToken: s.access_token, personName: s.person?.display_name ?? null },
    key,
    home,
    offset: 0,
  };
}

/** The daemon's `host[:port]` in `url`, in WHATWG URL's canonical form: an
 * IP literal host, the port left out when it is 80; null unless `url` is
 * `http://<IP>[:<port>]` with no path, query or credentials. That is the form
 * the Rust clients make (`clax_core::remote::DaemonUrl::parse`) except for an
 * IPv4-mapped IPv6 host, which Rust writes `[::ffff:1.2.3.4]` and WHATWG
 * `[::ffff:102:304]`. */
export function daemonHost(url: string): string | null {
  let u: URL;
  try {
    u = new URL(url);
  } catch {
    return null;
  }
  if (u.protocol !== "http:" || u.username || u.password || u.pathname !== "/" || url.includes("?") || url.includes("#")) return null;
  const ip = u.hostname.startsWith("[") ? u.hostname.slice(1, -1) : u.hostname;
  if (!isIP(ip)) return null;
  return u.host;
}

const sha256 = (b: Buffer | string) => createHash("sha256").update(b).digest();

/** `value` without its leading and trailing spaces and tabs, as the daemon
 * reads a header (`clax_core::sig::trim_header_value`). */
export function trimHeaderValue(value: string): string {
  return value.replace(/^[ \t]+|[ \t]+$/g, "");
}

/** The fixed serialization of the agent headers' values
 * (`clax_core::sig::encode_agent_headers`): per header in
 * [`AGENT_HEADERS`] order, `-\n` when absent, else `+<byte length>:<value>\n`. */
export function encodeAgentHeaders(values: (string | undefined)[]): Buffer {
  const parts: Buffer[] = [];
  for (const v of values) {
    if (v === undefined) parts.push(Buffer.from("-\n"));
    else {
      const b = Buffer.from(v, "utf8");
      parts.push(Buffer.from(`+${b.length}:`), b, Buffer.from("\n"));
    }
  }
  return Buffer.concat(parts);
}

/** What a request signs (`clax_core::sig::SignInput`). */
export interface SignInput {
  method: string;
  /** The path and query exactly as sent. */
  pathAndQuery: string;
  host: string;
  t: number;
  nonce: string;
  body: Buffer;
  /** The access token; none for the grant request. */
  token: string | undefined;
  /** The [`AGENT_HEADERS`] values the request sends, trimmed. */
  agent: (string | undefined)[];
}

/** The signed message (`clax_core::sig::SignInput::canonical`). */
export function canonical(i: SignInput): string {
  const token = i.token === undefined ? "" : sha256(i.token).toString("hex");
  return [
    VERSION_LINE, i.method, i.pathAndQuery, i.host, String(i.t), i.nonce,
    sha256(i.body).toString("hex"), token, sha256(encodeAgentHeaders(i.agent)).toString("hex"),
  ].join("\n");
}

/** A fresh nonce: 16 random bytes in unpadded base64url (22 characters). */
export function newNonce(): string {
  return randomBytes(16).toString("base64url");
}

/** The `Clax-Signature` value for `i`, signed with `key`. */
export function signature(key: KeyObject, i: SignInput): string {
  // eslint-disable-next-line no-control-regex
  if (/[\x00-\x1f\x7f]/.test(i.method + i.pathAndQuery + i.host + i.nonce)) throw new Error("a request field holds a control character");
  const s = edSign(null, Buffer.from(canonical(i), "utf8"), key);
  return `t=${i.t}, n=${i.nonce}, s=${s.toString("base64url")}`;
}

/** The headers of a request to `r` for `method` `pathAndQuery` with
 * `body`, from `headers` (any `authorization` replaced by the grant's,
 * `host` set, the agent headers trimmed), with its `clax-signature`
 * signed now. */
export function signedHeaders(r: Remote, method: string, pathAndQuery: string, body: Buffer, headers: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(headers)) {
    const name = k.toLowerCase();
    if (name === "authorization" || name === "host" || name === "clax-signature") continue;
    out[name] = (AGENT_HEADERS as readonly string[]).includes(name) ? trimHeaderValue(v) : v;
  }
  out.host = r.config.host;
  out.authorization = `Clax-Grant ${r.config.accessToken}`;
  const t = Math.floor(Date.now() / 1000) + r.offset;
  out["clax-signature"] = signature(r.key, {
    method, pathAndQuery, host: r.config.host, t, nonce: newNonce(), body,
    token: r.config.accessToken, agent: AGENT_HEADERS.map(h => out[h]),
  });
  return out;
}

/** The most bytes of a remote daemon's answer a client reads
 * (`clax_core::remote::MAX_RESPONSE_BYTES`); a longer one fails
 * `bad_response`. */
export const MAX_RESPONSE_BYTES = 128 * 1024 * 1024;

/** The daemon's time in a response's `Date` less this machine's, in
 * seconds; `undefined` without a readable `Date`. */
export function daemonOffset(date: string | undefined): number | undefined {
  const at = date ? Date.parse(date) : NaN;
  if (Number.isNaN(at)) return undefined;
  return Math.round(at / 1000) - Math.floor(Date.now() / 1000);
}

/** Learns the daemon's clock from a response's `Date`: true when the
 * offset is within [`MAX_OFFSET_S`]. */
export function learnOffset(r: Remote, date: string | undefined): boolean {
  const o = daemonOffset(date);
  if (o === undefined || Math.abs(o) > MAX_OFFSET_S) return false;
  r.offset = o;
  return true;
}

/** Why a request was not retried with a learned clock, as the Rust clients
 * say it (`clax_core::remote::clock_skew_message`). */
export function clockSkewMessage(offset: number): string {
  const signed = offset >= 0 ? `+${offset}` : `${offset}`;
  return `the daemon's clock is ${signed} s from this machine's, more than the ${MAX_OFFSET_S} s a signed request may be moved by: set this machine's clock (nothing was signed for that time)`;
}

// ---- Session credential files (spec 2026-10-08 §6.8) ----------------------

/** One agent session's credential: its ID and secret. */
export interface SessionCredential {
  session: string;
  secret: string;
}

/** The file name of harness session `id`'s credential: `pi-s-<id>.json`
 * for an ID of 1 to 128 characters of `[A-Za-z0-9._-]`, else
 * `pi-h-<the first 32 hex characters of its SHA-256>.json`
 * (`clax_core::remote::SessionKey::file_name`). */
export function credentialFileName(harness: string, id: string): string | undefined {
  if (!/^[a-z]{1,32}$/.test(harness) || id === "") return undefined;
  return /^[A-Za-z0-9._-]{1,128}$/.test(id) ? `${harness}-s-${id}.json` : `${harness}-h-${sha256(id).toString("hex").slice(0, 32)}.json`;
}

/** The credential files of a connected home, `<home>/remote-sessions/`:
 * the directory 0700, each file 0600, written whole and renamed into
 * place. Neither the directory nor a file is followed through a symbolic
 * link, and a file that is not a regular file is refused. Node has no
 * `openat`, so the directory is checked with `lstat` and each file opened
 * with `O_NOFOLLOW`. A Pi row is written under its harness session ID
 * only: the extension cannot read its parent's start time, so it writes
 * no parent-PID file. */
export class SessionFiles {
  constructor(readonly dir: string) {}

  static of(home: string): SessionFiles {
    return new SessionFiles(join(home, "remote-sessions"));
  }

  /** The directory, refused when it is a symbolic link or not a directory;
   * false when it does not exist. */
  private checkDir(): boolean {
    try {
      const st = lstatSync(this.dir);
      if (!st.isDirectory()) throw new Error(`${this.dir} is a symbolic link or not a directory, which is never followed`);
      return true;
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code === "ENOENT") return false;
      throw e;
    }
  }

  /** The credential in file `name`; `undefined` when there is none. Throws
   * naming the file when it exists but cannot be used. */
  read(name: string): SessionCredential | undefined {
    const path = join(this.dir, name);
    if (!this.checkDir()) return undefined;
    let fd: number;
    try {
      fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
    } catch (e) {
      const code = (e as NodeJS.ErrnoException).code;
      if (code === "ENOENT") return undefined;
      if (code === "ELOOP") throw new Error(`session_credential_unreadable: ${path}: a symbolic link, which is never followed`);
      throw new Error(`session_credential_unreadable: ${path}: ${(e as Error).message}`);
    }
    try {
      if (!fstatSync(fd).isFile()) throw new Error(`session_credential_unreadable: ${path}: not a regular file`);
      const buf = Buffer.alloc(64 * 1024);
      const n = readSync(fd, buf, 0, buf.length, 0);
      const f = JSON.parse(buf.subarray(0, n).toString("utf8"));
      if (typeof f?.session !== "string" || !f.session || typeof f.secret !== "string" || !f.secret.startsWith(SESSION_PREFIX)) {
        throw new Error("not a session credential");
      }
      return { session: f.session, secret: f.secret };
    } catch (e) {
      const m = (e as Error).message;
      throw new Error(m.startsWith("session_credential_unreadable") ? m : `session_credential_unreadable: ${path}: not a session credential`);
    } finally {
      closeSync(fd);
    }
  }

  /** Writes `cred`, the credential of a row whose harness session ID is
   * `harnessSessionId`, to file `name`, 0600, renamed into place; the
   * directory made 0700, or set so. */
  write(name: string, harnessSessionId: string | null, cred: SessionCredential): void {
    if (!this.checkDir()) mkdirSync(this.dir, { recursive: true, mode: 0o700 });
    if ((lstatSync(this.dir).mode & 0o777) !== 0o700) chmodSync(this.dir, 0o700);
    const body: Record<string, string> = { session: cred.session, secret: cred.secret };
    if (harnessSessionId) body.harness_session_id = harnessSessionId;
    const tmp = join(this.dir, `.${name}.${randomBytes(8).toString("hex")}.tmp`);
    const fd = openSync(tmp, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
    try {
      writeSync(fd, `${JSON.stringify(body)}\n`);
      fsyncSync(fd);
    } catch (e) {
      closeSync(fd);
      try { unlinkSync(tmp); } catch { /* gone */ }
      throw e;
    }
    closeSync(fd);
    try {
      renameSync(tmp, join(this.dir, name));
    } catch (e) {
      try { unlinkSync(tmp); } catch { /* gone */ }
      throw e;
    }
  }

  /** Marks file `name` in use now (its modification time) when it holds
   * session `session`, so the Rust clients' pruning of credential files
   * unused for a week (`clax_core::remote::STALE_FILE_AFTER`) keeps it. */
  touch(name: string, session: string): void {
    try {
      if (this.read(name)?.session !== session) return;
      const fd = openSync(join(this.dir, name), constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
      try {
        const now = new Date();
        futimesSync(fd, now, now);
      } finally {
        closeSync(fd);
      }
    } catch { /* another session's, or unreadable: left alone */ }
  }

  /** Removes file `name` when it holds session `session`. */
  remove(name: string, session: string): void {
    try {
      if (this.read(name)?.session === session) unlinkSync(join(this.dir, name));
    } catch { /* another session's, or unreadable: left alone */ }
  }
}

// ---- Clips (spec 2026-10-08 §16) -------------------------------------------

/** The scheme a remote session's payload names its clips by. */
export const CLIP_SCHEME = "clax-clip:";
const ARTIFACT_ID = /^[0-9abcdefghjkmnpqrstvwxyz]{12}$/;
const THREAD_ULID = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/;

/** The artifact and thread a clip reference names (`clax_core::clips::parse_reference`). */
export function parseClipReference(s: string): [string, string] | undefined {
  if (!s.startsWith(CLIP_SCHEME)) return undefined;
  const rest = s.slice(CLIP_SCHEME.length);
  const slash = rest.indexOf("/");
  if (slash < 0) return undefined;
  const [aid, tid] = [rest.slice(0, slash), rest.slice(slash + 1)];
  return ARTIFACT_ID.test(aid) && THREAD_ULID.test(tid) ? [aid, tid] : undefined;
}

/** `s` with control characters, U+2028 and U+2029 written as `\uXXXX`. */
function oneLine(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/[\x00-\x1f\x7f-\x9f\u2028\u2029]/g, c => `\\u${c.charCodeAt(0).toString(16).padStart(4, "0")}`);
}

/** A fetch of one clip: its cached file, or why there is none. One is made
 * per payload, and bounds every clip of it from its first fetch, as the
 * shim's `CLIP_FETCH_TIME` does, so a clip waiting for a turn behind slow
 * ones never holds the payload longer. */
export type ClipFetch = (aid: string, tid: string) => Promise<{ path: string } | { why: string }>;

/** The most clip fetches one payload runs at once
 * (`clax_mcp::client::CLIP_FETCHES_AT_ONCE`). */
export const CLIP_FETCHES_AT_ONCE = 4;

/** Fetches each distinct clip `refs` names once, together, with at most
 * [`CLIP_FETCHES_AT_ONCE`] in flight; `fetch` bounds each, its wait for a
 * turn included. */
async function fetchAll(refs: [string, string][], fetch: ClipFetch): Promise<Map<string, { path: string } | { why: string }>> {
  const keys = [...new Set(refs.map(([aid, tid]) => `${aid}/${tid}`))];
  const got: ({ path: string } | { why: string })[] = new Array(keys.length);
  let next = 0;
  const turn = async () => {
    while (next < keys.length) {
      const i = next++;
      const [aid, tid] = keys[i]!.split("/") as [string, string];
      got[i] = await fetch(aid, tid);
    }
  };
  await Promise.all(Array.from({ length: Math.min(CLIP_FETCHES_AT_ONCE, keys.length) }, turn));
  return new Map(keys.map((k, i) => [k, got[i]!]));
}

/** The most bytes a clip may hold (`clax_core::clips::MAX_BYTES`). */
export const CLIP_MAX_BYTES = 5 * 1024 * 1024;
const PNG_SIGNATURE = Buffer.from("89504e470d0a1a0a", "hex");

/** Why a downloaded clip is not one (`clax_core::clips::check`): not
 * `image/png`, over [`CLIP_MAX_BYTES`], or without a PNG's signature;
 * `undefined` when it is. */
export function checkClip(contentType: string | undefined, bytes: Buffer): string | undefined {
  if ((contentType ?? "").split(";")[0]!.trim().toLowerCase() !== "image/png") return "not a PNG";
  if (bytes.length > CLIP_MAX_BYTES) return `over ${CLIP_MAX_BYTES} bytes`;
  if (!bytes.subarray(0, 8).equals(PNG_SIGNATURE)) return "not a PNG";
  return undefined;
}

/** A remote daemon's payload `text` with every `Clip:` line rewritten
 * (`clax_core::clips::rewrite_text`): a clip reference to the file `fetch`
 * cached, or `unavailable (<why>)`; `Clip: withheld` and `Clip: none (…)`
 * kept without a fetch; any other value `unavailable`, so a remote daemon
 * never hands the agent a path of its own. A `Snapshot:` line becomes
 * `Snapshot: unavailable (…)`. */
export async function rewriteClipText(text: string, fetch: ClipFetch): Promise<string> {
  const lines = text.split("\n");
  const refs = lines.map(l => (l.startsWith("Clip: ") ? parseClipReference(l.slice(6)) : undefined)).filter((r): r is [string, string] => r !== undefined);
  const got = await fetchAll(refs, fetch);
  return lines.map(l => {
    if (l.startsWith("Snapshot: ")) return "Snapshot: unavailable (on the remote daemon's machine)";
    if (!l.startsWith("Clip: ")) return l;
    const v = l.slice(6);
    if (v === "withheld" || v.startsWith("none (")) return l;
    const ref = parseClipReference(v);
    if (!ref) return "Clip: unavailable (the remote daemon named no clip)";
    const r = got.get(`${ref[0]}/${ref[1]}`)!;
    return "path" in r ? `Clip: ${r.path}` : `Clip: unavailable (${oneLine(r.why)})`;
  }).join("\n");
}

/** Remote feedback items or thread views with their `clip_path` rewritten
 * (`clax_core::clips::rewrite_json`): the cached file, else null (with
 * `clip_unavailable` saying why); any `snapshot_path` made null. */
export async function rewriteClipItems(items: unknown[], fetch: ClipFetch): Promise<void> {
  const objs = items.filter((v): v is Record<string, any> => typeof v === "object" && v !== null);
  const refs = objs.map(o => (typeof o.clip_path === "string" ? parseClipReference(o.clip_path) : undefined)).filter((r): r is [string, string] => r !== undefined);
  const got = await fetchAll(refs, fetch);
  for (const o of objs) {
    if (typeof o.clip_path === "string") {
      const ref = parseClipReference(o.clip_path);
      const r = ref ? got.get(`${ref[0]}/${ref[1]}`) : undefined;
      if (r && "path" in r) o.clip_path = r.path;
      else {
        o.clip_path = null;
        if (r) o.clip_unavailable = r.why;
      }
    }
    if (o.snapshot_path !== undefined && o.snapshot_path !== null) o.snapshot_path = null;
  }
}

/** Writes clip `bytes` of `(aid, tid)` to `<home>/remote-cache/clips/`
 * (0700; the file 0600, written whole and renamed into place); its path. */
export function storeClip(home: string, aid: string, tid: string, bytes: Buffer): string {
  const dir = join(home, "remote-cache", "clips");
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  const st = lstatSync(dir);
  if (!st.isDirectory()) throw new Error(`${dir} is not a directory`);
  if ((st.mode & 0o777) !== 0o700) chmodSync(dir, 0o700);
  const path = join(dir, `${aid}-${tid}.png`);
  const tmp = join(dir, `.${aid}-${tid}.${randomBytes(8).toString("hex")}.tmp`);
  const fd = openSync(tmp, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
  try {
    writeSync(fd, bytes);
    fsyncSync(fd);
  } catch (e) {
    closeSync(fd);
    try { unlinkSync(tmp); } catch { /* gone */ }
    throw e;
  }
  closeSync(fd);
  try {
    renameSync(tmp, path);
  } catch (e) {
    try { unlinkSync(tmp); } catch { /* gone */ }
    throw e;
  }
  return path;
}
