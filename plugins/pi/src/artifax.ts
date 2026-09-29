// Artifax for Pi: registers the Pi session with the local Artifax daemon and
// adds the nine Artifax tools (`artifax_publish`, `artifax_read`, ...) and the
// `/artifax` command. Pi has no MCP support in its extension API, so the tools
// call the daemon's REST API directly and return the same JSON as the MCP tools.
import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { extname, isAbsolute, join, basename } from "node:path";
import type { ExtensionAPI, ExtensionContext } from "@mariozechner/pi-coding-agent";
import { Type, type Static, type TSchema } from "typebox";
import { ClientError, DaemonClient, type Registration } from "./client.ts";
import { artifaxHome, discover, endpointOf, ensure, logPath } from "./daemon.ts";

/** Default cap on the bytes `read` returns. */
export const DEFAULT_READ_MAX_BYTES = 200_000;

/** The published path of an artifact's page. */
const INDEX = "index.html";

/** This package's version, which is the Artifax version it is released with
 * (`scripts/test-plugins.sh` keeps them equal); `status` reports
 * `daemon_version` when the daemon's version differs. */
const VERSION: string = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")).version;

/** File extensions published as UTF-8 text when they decode as UTF-8; any other
 * file is sent as base64. */
const TEXT_EXT = new Set(["html", "htm", "css", "js", "mjs", "json", "svg", "md", "txt", "csv", "xml", "map"]);

// ---- Tool schemas ----------------------------------------------------------

const strict = { additionalProperties: false } as const;
const opt = <T extends TSchema>(t: T) => Type.Optional(t);
const str = (description: string) => Type.String({ description });
const version = (description: string) => Type.Integer({ minimum: 0, description });
const urlOrId = str("Artifact URL or ID.");

const FileArg = Type.Object({
  path: opt(str("Local file to read; text files are sent as UTF-8, others as base64.")),
  content: opt(str("The file's content, encoded as `encoding` says.")),
  encoding: opt(Type.Unsafe<"utf8" | "base64">({ type: "string", enum: ["utf8", "base64"], description: "Encoding of `content`: `utf8` (default) or `base64`." })),
  content_type: opt(str("Content type to serve the file with; inferred from the extension when absent.")),
}, { ...strict, description: "A supporting file: exactly one of `path` (a local file) and `content`." });

const PublishArgs = Type.Object({
  file_path: opt(str("Local HTML file to publish as index.html. Exactly one of `file_path` and `html`.")),
  html: opt(str("HTML to publish as index.html. Exactly one of `file_path` and `html`.")),
  files: opt(Type.Record(Type.String(), Type.Union([FileArg, Type.Null()]), {
    description: "Supporting files by published path; `null` removes a file carried forward from the previous version.",
  })),
  url: opt(str("URL of an artifact to update (instead of creating one).")),
  id: opt(str("ID of an artifact to update (instead of creating one).")),
  if_version: opt(version("The version this update is based on; a stale value fails with the current version. Defaults to the artifact's current version.")),
  title: opt(str("Artifact title.")),
  description: opt(str("One-line description.")),
  icon: opt(str("One generic word for the icon, such as chart or map.")),
  label: opt(str("Short label for this version.")),
  capabilities: opt(Type.Record(Type.String(), Type.Unknown(), { description: "Runtime capabilities the page declares." })),
}, strict);

const ReadArgs = Type.Object({
  url_or_id: str("Artifact URL or ID. A URL naming a version (`/a/<id>/v/<n>`) selects that version unless `version` is given."),
  path: opt(str("Published path to read; defaults to index.html.")),
  version: opt(version("Version to read; defaults to the URL's version, else the current version.")),
  max_bytes: opt(Type.Integer({ minimum: 0, description: "Most bytes of content to return (default 200000); longer files are cut and flagged `truncated`." })),
}, strict);

const ListArgs = Type.Object({
  limit: opt(Type.Integer({ minimum: 0, description: "Most artifacts to return." })),
  scope: opt(Type.Unsafe<"mine" | "all">({ type: "string", enum: ["mine", "all"], description: "`all` (default) lists every artifact; `mine` only those this session created." })),
}, strict);

const TargetArgs = Type.Object({ url_or_id: urlOrId }, strict);

const AssetUploadArgs = Type.Object({
  url_or_id: str("Artifact URL or ID the assets belong to."),
  file_path: opt(str("One local file to upload.")),
  file_paths: opt(Type.Array(Type.String(), { description: "Several local files to upload." })),
}, strict);

const StatusArgs = Type.Object({}, strict);

type Json = Record<string, any>;

// ---- Results ---------------------------------------------------------------

/** A finished error result, thrown out of a tool so that Pi flags the result
 * `isError`; its message is the result's JSON text. */
export class ToolError extends Error {
  constructor(readonly error: Json) {
    super(render({ error }));
    this.name = "ToolError";
  }
}

/** The text of a result: pretty-printed JSON carrying a `feedback` array
 * (empty until comments exist). */
function render(obj: Json): string {
  return JSON.stringify({ ...obj, feedback: [] }, null, 2);
}

function toolError(code: string, message: string, extra: Json = {}): ToolError {
  return new ToolError({ code, message, ...extra });
}

const invalid = (message: string) => toolError("invalid_args", message);

/** An error result for an unexpected exception, so every error stays JSON. */
function internal(e: unknown): ToolError {
  return e instanceof ToolError ? e : toolError("internal", e instanceof Error ? e.message : String(e));
}
const notFound = (message: string) => toolError("not_found", message);

/** The error result for a failed daemon call. An unreachable daemon is
 * `daemon_unreachable` naming `log`; a request past its deadline is `timeout`
 * (also naming `log`); an unparseable success body is `bad_response`; an API
 * error passes the daemon's `error` object through unchanged (so a conflict
 * keeps its `current`). Anything else is `internal`. */
function clientError(e: unknown, log: string): ToolError {
  if (e instanceof ToolError) return e;
  if (!(e instanceof ClientError)) return internal(e);
  switch (e.kind) {
    case "unreachable":
      return toolError("daemon_unreachable", `the artifax daemon did not respond (${e.message}); see its log`, { log });
    case "timeout":
      return toolError("timeout", "the daemon did not respond in time; for a publish, read the artifact before retrying", { detail: e.message, log });
    case "bad_response":
      return toolError("bad_response", `the daemon's response could not be read: ${e.message}`);
    case "api":
      return new ToolError(e.error);
  }
}

// ---- Helpers ---------------------------------------------------------------

const ID_RE = /^[0-9abcdefghjkmnpqrstvwxyz]{12}$/;

/** The artifact ID, and the version when the reference names one, in a bare ID
 * or a URL of one of these forms (query and fragment ignored): `.../a/<id>`,
 * `.../a/<id>/v/<n>`, `.../c/<id>/v/<n>/...`, and the per-artifact origin
 * `http://<id>.localhost:<port>/v/<n>/...`. */
export function artifactRef(urlOrId: string): { id: string; version?: number } {
  const s = urlOrId.trim().split(/[?#]/)[0] ?? "";
  if (ID_RE.test(s)) return { id: s };
  const versionIn = (segs: string[]) => (segs[0] === "v" && /^\d+$/.test(segs[1] ?? "") ? Number(segs[1]) : undefined);
  const scheme = s.indexOf("://");
  let host = "";
  let path = s;
  if (scheme >= 0) {
    const rest = s.slice(scheme + 3);
    const slash = rest.indexOf("/");
    host = slash >= 0 ? rest.slice(0, slash) : rest;
    path = slash >= 0 ? rest.slice(slash + 1) : "";
  }
  const segs = path.split("/").filter(seg => seg !== "");
  const colon = host.lastIndexOf(":");
  const hostname = colon >= 0 ? host.slice(0, colon) : host;
  if (hostname.endsWith(".localhost")) {
    const id = hostname.slice(0, -".localhost".length);
    if (ID_RE.test(id)) return { id, version: versionIn(segs) };
  }
  for (let i = 0; i < segs.length; i++) {
    const id = segs[i + 1];
    if ((segs[i] === "a" || segs[i] === "c") && id !== undefined && ID_RE.test(id)) {
      return { id, version: versionIn(segs.slice(i + 2)) };
    }
  }
  throw toolError("invalid_id", `'${urlOrId}' is not an artifact ID or URL`);
}

function readLocal(path: string): Buffer {
  try {
    return readFileSync(path);
  } catch (e) {
    throw toolError("file_unreadable", `cannot read ${path}: ${e instanceof Error ? e.message : String(e)}`, { path });
  }
}

function decodesAsUtf8(bytes: Buffer): string | undefined {
  try {
    return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
  } catch {
    return undefined;
  }
}

/** A publish file entry for a local file: UTF-8 text for text extensions whose
 * bytes decode, base64 otherwise. */
function fileEntry(path: string): Json {
  const bytes = readLocal(path);
  const ext = extname(path).slice(1).toLowerCase();
  const text = TEXT_EXT.has(ext) ? decodesAsUtf8(bytes) : undefined;
  return text !== undefined ? { content: text, encoding: "utf8" } : { content: bytes.toString("base64"), encoding: "base64" };
}

/** True for content types `read` returns as text. */
export function isText(contentType: string): boolean {
  const base = (contentType.split(";")[0] ?? "").trim().toLowerCase();
  return base.startsWith("text/") || ["application/json", "application/javascript", "image/svg+xml"].includes(base);
}

/** The first `max` bytes of `bytes` as text. A cut through a multi-byte
 * character drops that character; other invalid UTF-8 is replaced with U+FFFD. */
export function textPrefix(bytes: Buffer, max: number): string {
  const decoder = new TextDecoder("utf-8", { ignoreBOM: true });
  return max < bytes.length ? decoder.decode(bytes.subarray(0, max), { stream: true }) : decoder.decode(bytes);
}

/** Longest title [`htmlTitle`] returns, in characters. */
export const MAX_DERIVED_TITLE_CHARS = 200;

/** Whitespace as Rust's `char::is_whitespace` (Unicode White_Space); unlike
 * `\s` it includes U+0085 and excludes U+FEFF. */
const WHITESPACE = /[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]+/;
const TRAILING_WHITESPACE = new RegExp(`${WHITESPACE.source}$`);
const BASIC_ENTITIES: Record<string, string> = { "&amp;": "&", "&lt;": "<", "&gt;": ">", "&quot;": "\"", "&apos;": "'" };

/** The text of the first `<title>` element of `html`, for use as an artifact
 * title: the tag name matches in any case and may carry attributes; the five
 * entities `&amp;` `&lt;` `&gt;` `&quot;` `&apos;` are decoded once (any other
 * entity is kept as written); runs of whitespace become one space; the result
 * is trimmed and cut to [`MAX_DERIVED_TITLE_CHARS`] characters. `undefined`
 * when there is no closed `<title>` element or its text is empty. The same
 * rule as `artifax_core::html_title`. */
export function htmlTitle(html: string): string | undefined {
  // ASCII-only lowercasing keeps every index valid in `html`.
  const lower = html.replace(/[A-Z]/g, c => c.toLowerCase());
  let from = 0;
  let bodyStart: number;
  for (;;) {
    const at = lower.indexOf("<title", from);
    if (at < 0) return undefined;
    const after = at + "<title".length;
    const next = lower[after];
    if (next === ">") {
      bodyStart = after + 1;
      break;
    }
    if (next !== undefined && /[\t\n\f\r /]/.test(next)) {
      const gt = lower.indexOf(">", after);
      if (gt < 0) return undefined;
      bodyStart = gt + 1;
      break;
    }
    from = after;
  }
  const bodyEnd = lower.indexOf("</title", bodyStart);
  if (bodyEnd < 0) return undefined;
  const text = html.slice(bodyStart, bodyEnd).replace(/&(?:amp|lt|gt|quot|apos);/g, e => BASIC_ENTITIES[e] ?? e);
  const joined = text.split(WHITESPACE).filter(w => w !== "").join(" ");
  const title = Array.from(joined).slice(0, MAX_DERIVED_TITLE_CHARS).join("").replace(TRAILING_WHITESPACE, "");
  return title === "" ? undefined : title;
}

/** How long [`openInBrowser`] waits for the opener to exit. */
export const OPEN_WAIT_MS = 1_500;

/** Runs the platform opener (`open` on macOS, `xdg-open` elsewhere, looked up
 * on `env`'s `PATH`) on `url` with stdio detached and waits up to
 * [`OPEN_WAIT_MS`] for it. True when it exits successfully in time, or is
 * still running then (best effort: some openers hand off and linger); false
 * when it cannot start or exits unsuccessfully. */
function openInBrowser(url: string, env: NodeJS.ProcessEnv): Promise<boolean> {
  const opener = process.platform === "darwin" ? "open" : "xdg-open";
  return new Promise(resolve => {
    let timer: NodeJS.Timeout | undefined;
    const done = (opened: boolean) => {
      clearTimeout(timer);
      resolve(opened);
    };
    try {
      const child = spawn(opener, [url], { stdio: "ignore", detached: true, env });
      child.once("error", () => done(false));
      child.once("exit", code => done(code === 0));
      timer = setTimeout(() => { child.unref(); done(true); }, OPEN_WAIT_MS);
    } catch {
      done(false);
    }
  });
}

// ---- The tool set ----------------------------------------------------------

export interface ArtifaxOptions {
  /** The Artifax home; `$ARTIFAX_HOME`, else `~/.artifax`, when absent. */
  home?: string;
  /** Environment for locating the `artifax` binary (`ARTIFAX_BIN`, `PATH`)
   * and for `ARTIFAX_NO_OPEN`; this process's when absent. */
  env?: NodeJS.ProcessEnv;
  /** Port for a daemon the extension starts; the CLI's default when absent. */
  port?: number;
}

/** The Artifax tools for one Pi session. */
class Tools {
  private client: DaemonClient | undefined;

  constructor(private readonly home: string, private readonly opts: ArtifaxOptions) {}

  private get env(): NodeJS.ProcessEnv {
    return this.opts.env ?? process.env;
  }

  /** The client for this session, created on first use from `ctx`. */
  clientFor(ctx: ExtensionContext): DaemonClient {
    if (!this.client) {
      const registration: Registration = {
        harness: "pi",
        harness_session_id: ctx.sessionManager.getSessionId() || null,
        cwd: ctx.cwd,
        pid: process.pid,
        parent_pid: process.ppid,
      };
      const refresh = async () => endpointOf(await ensure(this.home, { env: this.env, port: this.opts.port }));
      const find = async () => {
        const info = await discover(this.home);
        if (!info) throw new Error("no artifax daemon is running");
        return endpointOf(info);
      };
      this.client = new DaemonClient(refresh, find, registration);
    }
    return this.client;
  }

  /** The client of an existing session, if one was created. */
  existingClient(): DaemonClient | undefined {
    return this.client;
  }

  get log(): string {
    return logPath(this.home);
  }

  private browserBase(c: DaemonClient): string {
    return (c.browserBase() ?? "").replace(/\/+$/, "");
  }

  private artifactUrl(c: DaemonClient, id: string): string {
    return `${this.browserBase(c)}/a/${id}`;
  }

  /** Runs `f`, turning daemon failures into error results. */
  private async call<T>(f: () => Promise<T>): Promise<T> {
    try {
      return await f();
    } catch (e) {
      throw clientError(e, this.log);
    }
  }

  /** `p` as given when absolute (a leading `@` dropped), else joined to the
   * session's working directory. A relative path when Pi reports no working
   * directory is an `invalid_args` error. */
  private localPath(ctx: ExtensionContext, p: string): string {
    const path = p.startsWith("@") ? p.slice(1) : p;
    if (isAbsolute(path)) return path;
    if (!ctx.cwd) throw invalid(`file paths must be absolute: there is no session working directory to resolve '${p}' against`);
    return join(ctx.cwd, path);
  }

  private fileArg(ctx: ExtensionContext, name: string, f: Static<typeof FileArg>): Json {
    let entry: Json;
    if (f.path !== undefined && f.content === undefined) {
      if (f.encoding !== undefined) throw invalid(`files.${name}: encoding applies only to content`);
      entry = fileEntry(this.localPath(ctx, f.path));
    } else if (f.path === undefined && f.content !== undefined) {
      entry = { content: f.content, encoding: f.encoding ?? "utf8" };
    } else {
      throw invalid(`files.${name}: pass exactly one of path and content`);
    }
    if (f.content_type !== undefined) entry.content_type = f.content_type;
    return entry;
  }

  async publish(ctx: ExtensionContext, a: Static<typeof PublishArgs>): Promise<Json> {
    let page: Json;
    if (a.file_path !== undefined && a.html === undefined) page = fileEntry(this.localPath(ctx, a.file_path));
    else if (a.file_path === undefined && a.html !== undefined) page = { content: a.html, encoding: "utf8" };
    else throw invalid("pass exactly one of file_path and html");
    if (a.id !== undefined && a.url !== undefined) throw invalid("pass at most one of id and url");
    const ref = a.id ?? a.url;
    const target = ref === undefined ? undefined : artifactRef(ref).id;
    const files: Json = {};
    for (const [name, f] of Object.entries(a.files ?? {}).sort(([x], [y]) => (x < y ? -1 : x > y ? 1 : 0))) {
      if (name === INDEX) throw invalid("index.html comes from file_path or html, not files");
      files[name] = f === null ? null : this.fileArg(ctx, name, f);
    }
    // A new artifact needs a title: the given one, else the page's <title>.
    let title = a.title;
    if (target === undefined && title === undefined) {
      title = page.encoding === "utf8" ? htmlTitle(page.content) : undefined;
      if (title === undefined) throw invalid("a new artifact needs a title: pass `title`, or give the page a non-empty <title>");
    }
    files[INDEX] = page;
    const body: Json = { files };
    if (title !== undefined) body.title = title;
    for (const k of ["description", "icon", "label"] as const) if (a[k] !== undefined) body[k] = a[k];
    if (a.capabilities !== undefined) body.capabilities = a.capabilities;
    const c = this.clientFor(ctx);
    const res = await this.call(async () => {
      if (target === undefined) return c.create(body);
      body.if_version = a.if_version ?? Number((await c.get(target)).artifact?.current_version ?? 0);
      try {
        return await c.publishVersion(target, body);
      } catch (e) {
        if (e instanceof ClientError && e.kind === "api" && e.status === 409) throw await this.conflict(c, target, e);
        throw e;
      }
    });
    const id: string = res.artifact?.id ?? "";
    return {
      artifact_id: id,
      url: this.artifactUrl(c, id),
      version: res.version?.n ?? null,
      title: res.artifact?.title ?? null,
      files: Object.keys(res.version?.files ?? {}),
    };
  }

  /** The error result for a publish conflict: the daemon's error plus a summary
   * of the current version and how to merge. When the summary cannot be
   * fetched, the daemon's error alone. */
  private async conflict(c: DaemonClient, id: string, e: ClientError): Promise<ToolError> {
    const error: Json = { ...e.error };
    try {
      const got = await c.get(id);
      const n = Number(got.artifact?.current_version ?? 0);
      const v = (got.versions ?? []).find((v: Json) => v.n === n);
      if (v) {
        error.current_version = {
          n,
          label: v.label ?? null,
          created_at: v.created_at ?? null,
          files: Object.keys(v.files ?? {}),
          url: `${this.artifactUrl(c, id)}/v/${n}`,
        };
        error.hint = `read the current version, merge your change, and retry with if_version = ${n}`;
      }
    } catch { /* the daemon's error alone */ }
    return new ToolError(error);
  }

  async read(ctx: ExtensionContext, a: Static<typeof ReadArgs>): Promise<Json> {
    const { id, version: urlVersion } = artifactRef(a.url_or_id);
    const c = this.clientFor(ctx);
    const got = await this.call(() => c.get(id));
    const n = a.version ?? urlVersion ?? Number(got.artifact?.current_version ?? 0);
    const version = (got.versions ?? []).find((v: Json) => v.n === n);
    if (!version) throw notFound(`artifact ${id} has no version ${n}`);
    const path = a.path ?? INDEX;
    const meta = version.files?.[path];
    if (!meta) throw notFound(`version ${n} of ${id} has no file '${path}'`);
    const contentType: string = meta.content_type ?? "";
    const bytes = await this.call(() => c.fileBytes(id, n, path));
    const max = a.max_bytes ?? DEFAULT_READ_MAX_BYTES;
    const out: Json = { artifact_id: id, version: n, path, content_type: contentType, truncated: bytes.length > max, size: bytes.length };
    if (isText(contentType)) out.content = textPrefix(bytes, max);
    else if (bytes.length <= max) out.content_base64 = bytes.toString("base64");
    return out;
  }

  async list(ctx: ExtensionContext, a: Static<typeof ListArgs>): Promise<Json> {
    const c = this.clientFor(ctx);
    const res = await this.call(() => c.list());
    const sessionId = c.session()?.id;
    const mine = (x: Json) => a.scope !== "mine" || (sessionId !== undefined && x.owner_session_id === sessionId);
    const artifacts = (res.artifacts ?? []).filter(mine).slice(0, a.limit ?? Infinity).map((x: Json) => ({
      id: x.id,
      url: this.artifactUrl(c, x.id ?? ""),
      title: x.title ?? null,
      version: x.current_version ?? null,
      pinned: x.pinned ?? null,
      updated_at: x.updated_at ?? null,
      owner_session_id: x.owner_session_id ?? null,
    }));
    return { artifacts };
  }

  async delete(ctx: ExtensionContext, a: Static<typeof TargetArgs>): Promise<Json> {
    const { id } = artifactRef(a.url_or_id);
    const c = this.clientFor(ctx);
    await this.call(() => c.delete(id));
    return { artifact_id: id, deleted: true };
  }

  async setPinned(ctx: ExtensionContext, a: Static<typeof TargetArgs>, pinned: boolean): Promise<Json> {
    const { id } = artifactRef(a.url_or_id);
    const c = this.clientFor(ctx);
    const res = await this.call(() => c.patch(id, { pinned }));
    return { artifact_id: id, pinned: res.artifact?.pinned ?? null };
  }

  async open(ctx: ExtensionContext, a: Static<typeof TargetArgs>): Promise<Json> {
    const { id } = artifactRef(a.url_or_id);
    const c = this.clientFor(ctx);
    await this.call(() => c.get(id));
    const url = this.artifactUrl(c, id);
    const opened = this.env.ARTIFAX_NO_OPEN === undefined && (await openInBrowser(url, this.env));
    return { url, opened };
  }

  /** Opens the gallery in the person's browser. */
  async openGallery(ctx: ExtensionContext): Promise<Json> {
    const c = this.clientFor(ctx);
    await this.call(() => c.healthz());
    const url = `${this.browserBase(c)}/`;
    const opened = this.env.ARTIFAX_NO_OPEN === undefined && (await openInBrowser(url, this.env));
    return { url, opened };
  }

  async assetUpload(ctx: ExtensionContext, a: Static<typeof AssetUploadArgs>): Promise<Json> {
    const { id } = artifactRef(a.url_or_id);
    const paths = [...(a.file_path === undefined ? [] : [a.file_path]), ...(a.file_paths ?? [])];
    if (paths.length === 0) throw invalid("pass file_path or file_paths");
    const files = paths.map(p => {
      const path = this.localPath(ctx, p);
      return { name: basename(path) || "file", bytes: readLocal(path) };
    });
    const c = this.clientFor(ctx);
    const assets: Json[] = [];
    for (const { name, bytes } of files) {
      const res = await this.call(() => c.uploadAsset(id, name, undefined, bytes));
      const asset = res.asset ?? {};
      assets.push({
        id: asset.id ?? null,
        url: `${this.browserBase(c)}${res.url ?? ""}`,
        content_type: asset.content_type ?? null,
        size: asset.size ?? null,
      });
    }
    return { assets };
  }

  async status(ctx: ExtensionContext): Promise<Json> {
    const c = this.clientFor(ctx);
    const h = await this.call(() => c.healthz());
    const session = c.session() ?? null;
    const out: Json = {
      daemon_url: this.browserBase(c),
      version: h.version ?? null,
      harness: session?.harness ?? null,
      session,
      watches: [],
    };
    // Version skew between these tools and the daemon they call.
    if (h.version !== VERSION) out.daemon_version = h.version ?? null;
    return out;
  }
}

/** How long session_start may spend finding the daemon and registering. */
const START_BUDGET_MS = 3_000;

/** The `/artifax` command's usage line. */
const USAGE = "usage: /artifax open [ID] | list | status";

/** The Artifax extension, with `opts` overriding where it finds the daemon. */
export function artifaxExtension(opts: ArtifaxOptions = {}): (pi: ExtensionAPI) => void {
  return pi => {
    const tools = new Tools(opts.home ?? artifaxHome(opts.env ?? process.env), opts);

    // Pi awaits this handler before it continues, so registration gets at most
    // START_BUDGET_MS; the first tool call registers when this did not.
    pi.on("session_start", async (_event, ctx) => {
      let timer: NodeJS.Timeout | undefined;
      const budget = new Promise<"timeout">(r => { timer = setTimeout(() => r("timeout"), START_BUDGET_MS); });
      try {
        const registered = tools.clientFor(ctx).ensureSession(START_BUDGET_MS);
        // A registration still running when the budget ends may fail later, unobserved.
        registered.catch(() => undefined);
        await Promise.race([registered, budget]);
      } catch (e) {
        if (ctx.hasUI) ctx.ui.notify(`artifax: no daemon yet (${e instanceof Error ? e.message : String(e)})`, "warning");
      } finally {
        clearTimeout(timer);
      }
    });

    pi.on("session_shutdown", async () => {
      await tools.existingClient()?.endSession().catch(() => undefined);
    });

    const define = <P extends TSchema>(
      name: string,
      label: string,
      description: string,
      promptSnippet: string,
      parameters: P,
      run: (ctx: ExtensionContext, params: Static<P>) => Promise<Json>,
    ) => {
      pi.registerTool({
        name: `artifax_${name}`,
        label,
        description,
        promptSnippet,
        parameters,
        async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
          let result: Json;
          try {
            result = await run(ctx, params as Static<P>);
          } catch (e) {
            throw internal(e);
          }
          return { content: [{ type: "text", text: render(result) }], details: {} };
        },
      });
    };

    define("publish", "Artifax publish",
      "Publish an HTML page as a new artifact, or as a new version of an existing one (pass `id` or `url`, with `if_version`). Give the page as `html` or `file_path`, plus optional supporting `files`. Returns the artifact ID, its URL for the person, and the new version number.",
      "Publish an HTML page (artifact) to the local Artifax server, or a new version of one",
      PublishArgs, (ctx, a) => tools.publish(ctx, a));
    define("read", "Artifax read",
      "Read a published file (index.html by default) of an artifact's current or given version, as stored, before serve-time wrapping. Text is cut at `max_bytes` (default 200000) with `truncated: true`; binary files come back as `content_base64` when under the cap.",
      "Read a published file of an Artifax artifact version",
      ReadArgs, (ctx, a) => tools.read(ctx, a));
    define("list", "Artifax list",
      "List artifacts, pinned first and then most recently updated, with their URLs and current versions. `scope: mine` lists only those this session created.",
      "List Artifax artifacts with their URLs and current versions",
      ListArgs, (ctx, a) => tools.list(ctx, a));
    define("delete", "Artifax delete", "Delete an artifact and all its versions.",
      "Delete an Artifax artifact and all its versions",
      TargetArgs, (ctx, a) => tools.delete(ctx, a));
    define("open", "Artifax open", "Open an artifact in the person's browser on this machine.",
      "Open an Artifax artifact in the person's browser",
      TargetArgs, (ctx, a) => tools.open(ctx, a));
    define("pin", "Artifax pin", "Pin an artifact to the top of the gallery.",
      "Pin an Artifax artifact to the top of the gallery",
      TargetArgs, (ctx, a) => tools.setPinned(ctx, a, true));
    define("unpin", "Artifax unpin", "Unpin an artifact.",
      "Unpin an Artifax artifact",
      TargetArgs, (ctx, a) => tools.setPinned(ctx, a, false));
    define("asset_upload", "Artifax asset upload",
      "Upload local files (images, video, fonts, data) as assets of an artifact. Returns each asset's URL for the page to reference.",
      "Upload local files as assets of an Artifax artifact and get URLs for the page",
      AssetUploadArgs, (ctx, a) => tools.assetUpload(ctx, a));
    define("status", "Artifax status",
      "Report the Artifax daemon's URL and version and the session publishes are attributed to.",
      "Report the Artifax daemon's URL and version and this session",
      StatusArgs, ctx => tools.status(ctx));

    pi.registerCommand("artifax", {
      description: "Artifax: open [ID] (the gallery, or an artifact), list, status",
      handler: async (args, ctx) => {
        const [sub = "", target] = args.trim().split(/\s+/);
        try {
          switch (sub) {
            case "open": {
              const r = target ? await tools.open(ctx, { url_or_id: target }) : await tools.openGallery(ctx);
              ctx.ui.notify(r.opened ? `opened ${r.url}` : r.url, "info");
              return;
            }
            case "list": {
              const { artifacts } = await tools.list(ctx, {});
              const lines = artifacts.map((a: Json) => `${a.pinned ? "* " : ""}${a.title ?? "(untitled)"}  v${a.version}  ${a.url}`);
              ctx.ui.notify(lines.length ? lines.join("\n") : "no artifacts yet", "info");
              return;
            }
            case "status": {
              const s = await tools.status(ctx);
              ctx.ui.notify(`artifax daemon at ${s.daemon_url} (v${s.version}), session ${s.session?.id ?? "not registered"}`, "info");
              return;
            }
            default:
              ctx.ui.notify(USAGE, "error");
          }
        } catch (e) {
          const message = e instanceof ToolError ? `${e.error.code}: ${e.error.message}` : e instanceof Error ? e.message : String(e);
          ctx.ui.notify(`artifax: ${message}`, "error");
        }
      },
    });
  };
}

/** The Artifax extension for the daemon in `$ARTIFAX_HOME` (else `~/.artifax`). */
export default artifaxExtension();
